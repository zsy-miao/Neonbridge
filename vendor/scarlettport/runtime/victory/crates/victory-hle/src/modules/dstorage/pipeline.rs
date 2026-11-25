//! The pipelined request path: one I/O thread and a pool of decode threads
//! instead of workers that each read, decode and write one request.
//!
//! The I/O thread takes requests from the scheduler (same order, same
//! staging limit), groups the ones that read nearby bytes of one file into
//! larger reads (`plan`), and keeps many of them in flight through an I/O
//! completion port on a second, overlapped handle of each file. A finished
//! read is cut back into each request's source bytes, which go to the
//! decode threads; they inflate / unpack / copy into the destination and
//! finish the request in the scheduler exactly as a worker did, so status
//! entries keep their order and their first-failure results. A request
//! whose bytes a read did not deliver (an error, the end of the file) is
//! read again on its own, synchronously, so it fails exactly as before.
//!
//! Switches (environment, read once):
//! - `VICTORY_DS_PIPELINE` 0: the old workers (default 1).
//! - `VICTORY_DS_QD` reads in flight in all (default 64), `VICTORY_DS_QD_FILE`
//!   per file (default 16).
//! - `VICTORY_DS_COALESCE_KB` largest merged read (default 2048; 0: no merging),
//!   `VICTORY_DS_GAP_KB` largest hole read through to merge (default 256).
//! - `VICTORY_DS_DECODE_THREADS` (default: logical CPUs - 4, at least 2).
//! - `VICTORY_DS_UNBUFFERED` 1: reads bypass the system file cache (default 1:
//!   no second copy in the file cache, and ReadFile never blocks the I/O thread).
//! - `VICTORY_DS_BUFFER_MB` read buffers alive at once, holes included (default 192).
//! - `VICTORY_DS_STAGING_MB` the staging limit, raised over the title's 32 MiB
//!   (default 128; 0: the title's).
//! - `VICTORY_DS_CACHE_MB` a cache of recently read 256 KiB blocks, reads
//!   widened to whole blocks (default 0: off).
//! - `VICTORY_DS_MAX_JOBS` requests held by the pipeline at once (default 4096).

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{ReOpenFile, ReadFile};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatusEx, OVERLAPPED, OVERLAPPED_ENTRY,
    PostQueuedCompletionStatus,
};

use super::{File, Instance, Job, MEMORY_SOURCE, Scheduler, execute};

// Switches.

pub(super) struct Config {
    pub enabled: bool,
    qd: usize,
    qd_file: usize,
    pub coalesce: u64,
    gap: u64,
    decode_threads: usize,
    unbuffered: bool,
    cache_bytes: u64,
    max_jobs: usize,
    /// Source bytes the pipeline may hold (reads and decode queue).
    max_held: u64,
    max_buffers: u64,
    /// The staging limit we use (bytes; 0: the title's).
    pub staging: u64,
}

const CACHE_BLOCK: u64 = 256 << 10;
const SECTOR: u64 = 4096;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

pub(super) fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
        let config = Config {
            enabled: env_u64("VICTORY_DS_PIPELINE", 1) != 0,
            qd: env_u64("VICTORY_DS_QD", 64).max(1) as usize,
            qd_file: env_u64("VICTORY_DS_QD_FILE", 16).max(1) as usize,
            coalesce: env_u64("VICTORY_DS_COALESCE_KB", 2048).min(16 << 10) << 10,
            gap: env_u64("VICTORY_DS_GAP_KB", 256) << 10,
            decode_threads: env_u64("VICTORY_DS_DECODE_THREADS", cpus.saturating_sub(4).max(2) as u64)
                .clamp(1, 64) as usize,
            unbuffered: env_u64("VICTORY_DS_UNBUFFERED", 1) != 0,
            cache_bytes: env_u64("VICTORY_DS_CACHE_MB", 0) << 20,
            max_jobs: env_u64("VICTORY_DS_MAX_JOBS", 4096).max(1) as usize,
            max_held: 512 << 20,
            max_buffers: env_u64("VICTORY_DS_BUFFER_MB", 192).max(8) << 20,
            staging: env_u64("VICTORY_DS_STAGING_MB", 128) << 20,
        };
        tracing::info!(
            pipeline = config.enabled,
            qd = config.qd,
            qd_file = config.qd_file,
            coalesce_kb = config.coalesce >> 10,
            gap_kb = config.gap >> 10,
            decode_threads = config.decode_threads,
            unbuffered = config.unbuffered,
            cache_mb = config.cache_bytes >> 20,
            max_jobs = config.max_jobs,
            buffer_mb = config.max_buffers >> 20,
            staging_mb = config.staging >> 20,
            "dstorage: io switches"
        );
        config
    })
}

impl Config {
    /// What reads are aligned to: sectors when unbuffered, cache blocks when
    /// caching, else nothing.
    fn align(&self) -> u64 {
        let mut align = 1;
        if self.unbuffered {
            align = SECTOR;
        }
        if self.cache_bytes > 0 {
            align = align.max(CACHE_BLOCK);
        }
        align
    }
}

// Coalescing.

/// Bytes a request wants from its file.
#[derive(Clone, Copy, Debug)]
pub(super) struct Want {
    pub offset: u64,
    pub len: u64,
}

/// One read: `[start, start + len)`, serving the `members` (indices into
/// the wants), in offset order.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Extent {
    pub start: u64,
    pub len: u64,
    pub members: Vec<usize>,
}

/// Groups wants into reads: sorted by offset, a want joins the read before
/// it if it starts at most `gap` bytes after that read's end and the read
/// stays within `max` bytes (a want larger than `max` reads alone). Reads
/// start and end on multiples of `align`. `max` 0 merges nothing.
pub(super) fn plan(wants: &[Want], max: u64, gap: u64, align: u64) -> Vec<Extent> {
    let align = align.max(1);
    let down = |v: u64| v / align * align;
    let up = |v: u64| v.div_ceil(align) * align;
    let mut order: Vec<usize> = (0..wants.len()).collect();
    order.sort_by_key(|&i| (wants[i].offset, wants[i].len));
    let mut out: Vec<Extent> = Vec::new();
    let mut end = 0;
    for i in order {
        let w = wants[i];
        let (start, stop) = (down(w.offset), up(w.offset + w.len));
        if let Some(last) = out.last_mut() {
            let merged = stop.max(end) - last.start;
            if max > 0 && start <= end.saturating_add(gap) && merged <= max {
                end = end.max(stop);
                last.len = end - last.start;
                last.members.push(i);
                continue;
            }
        }
        end = stop;
        out.push(Extent {
            start,
            len: stop - start,
            members: vec![i],
        });
    }
    out
}

/// Where a want's bytes are in a read of `transferred` bytes from `start`,
/// or `None` if the read did not deliver all of them.
pub(super) fn member_bytes(start: u64, transferred: u64, want: Want) -> Option<Range<usize>> {
    let at = want.offset.checked_sub(start)?;
    let end = at.checked_add(want.len)?;
    (end <= transferred).then(|| at as usize..end as usize)
}

// Buffers.

/// A read's bytes, aligned for unbuffered reads.
pub(super) struct Buffer {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

// SAFETY: plain memory, written by one read, then only read.
unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}

impl Buffer {
    fn new(len: usize) -> Option<Buffer> {
        let layout = std::alloc::Layout::from_size_align(len.max(1), SECTOR as usize).ok()?;
        // SAFETY: a non-zero size.
        let ptr = unsafe { std::alloc::alloc(layout) };
        if ptr.is_null() {
            return None;
        }
        BUFFERS.fetch_add(layout.size() as u64, Ordering::Relaxed);
        Some(Buffer { ptr, layout })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: our allocation; the read finished before anyone looks.
        unsafe { std::slice::from_raw_parts(self.ptr, self.layout.size()) }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: allocated with this layout.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) }
        BUFFERS.fetch_sub(self.layout.size() as u64, Ordering::Relaxed);
    }
}

/// A request's source bytes as the decode threads get them.
pub(super) enum Source {
    /// Not read ahead: the request reads its source itself (memory, or a
    /// synchronous read).
    Itself,
    Slice(Arc<Buffer>, usize),
    Owned(Vec<u8>),
}

impl Source {
    fn bytes(&self, len: usize) -> Option<&[u8]> {
        match self {
            Source::Itself => None,
            Source::Slice(buffer, at) => buffer.bytes().get(*at..*at + len),
            Source::Owned(bytes) => Some(&bytes[..len]),
        }
    }
}

// The read-ahead cache.

/// Recently read blocks of `block` bytes, by (file, block number).
pub(super) struct BlockCache {
    block: u64,
    capacity: usize,
    blocks: HashMap<(u64, u64), (Arc<Buffer>, usize, u64)>,
    stamp: u64,
}

impl BlockCache {
    pub(super) fn new(block: u64, bytes: u64) -> BlockCache {
        BlockCache {
            block,
            capacity: (bytes / block) as usize,
            blocks: HashMap::new(),
            stamp: 0,
        }
    }

    /// Keeps the whole blocks of a read of `transferred` bytes that started
    /// at `start` (a block boundary).
    pub(super) fn insert(&mut self, file: u64, start: u64, buffer: &Arc<Buffer>, transferred: u64) {
        if self.capacity == 0 || start % self.block != 0 {
            return;
        }
        for i in 0..transferred / self.block {
            self.stamp += 1;
            let at = (i * self.block) as usize;
            self.blocks.insert(
                (file, start / self.block + i),
                (Arc::clone(buffer), at, self.stamp),
            );
        }
        if self.blocks.len() > self.capacity {
            // Drop the oldest eighth (or what is over).
            let over = self.blocks.len() - self.capacity + self.capacity / 8;
            let mut stamps: Vec<u64> = self.blocks.values().map(|v| v.2).collect();
            stamps.sort_unstable();
            let cut = stamps[over.min(stamps.len()) - 1];
            self.blocks.retain(|_, v| v.2 > cut);
        }
    }

    /// A want's bytes if every block of it is held.
    pub(super) fn lookup(&mut self, file: u64, want: Want) -> Option<Source> {
        if self.blocks.is_empty() || want.len == 0 {
            return None;
        }
        let (first, last) = (want.offset / self.block, (want.offset + want.len - 1) / self.block);
        let mut parts = Vec::new();
        for b in first..=last {
            parts.push(self.blocks.get(&(file, b))?);
        }
        let within = (want.offset - first * self.block) as usize;
        // One buffer, blocks in a row: no copy.
        let (buffer, at, _) = parts[0];
        let contiguous = parts.iter().enumerate().all(|(i, (b, a, _))| {
            Arc::ptr_eq(b, buffer) && *a == at + i * self.block as usize
        });
        let source = if contiguous {
            Source::Slice(Arc::clone(buffer), at + within)
        } else {
            let mut bytes = Vec::with_capacity(want.len as usize);
            for (b, a, _) in &parts {
                bytes.extend_from_slice(&b.bytes()[*a..*a + self.block as usize]);
            }
            bytes.drain(..within);
            bytes.truncate(want.len as usize);
            Source::Owned(bytes)
        };
        self.stamp += 1;
        for b in first..=last {
            if let Some(v) = self.blocks.get_mut(&(file, b)) {
                v.2 = self.stamp;
            }
        }
        Some(source)
    }
}

// Counters, summed up with the scheduler's 5 s line.

#[derive(Default)]
struct Stats {
    disk_bytes: AtomicU64,
    reads: AtomicU64,
    requests: AtomicU64,
    coalesced: AtomicU64,
    sync_reads: AtomicU64,
    instant_reads: AtomicU64,
    cache_hits: AtomicU64,
    /// Î£ reads in flight Ã— ns, for the average queue depth.
    qd_ns: AtomicU64,
    max_qd: AtomicU64,
    /// ns the I/O thread spent inside ReadFile (should be ~0 when async).
    issue_ns: AtomicU64,
    decode_ns: AtomicU64,
}

static STATS: OnceLock<Stats> = OnceLock::new();

fn stats() -> &'static Stats {
    STATS.get_or_init(Stats::default)
}

/// Coalesced requests so far (tests).
#[cfg(test)]
pub(super) fn coalesced_so_far() -> u64 {
    stats().coalesced.load(Ordering::Relaxed)
}

/// Logs the pipeline's counters for the last `secs` and resets them.
pub(super) fn log_summary(secs: f64) {
    if !running() {
        return;
    }
    let s = stats();
    let take = |a: &AtomicU64| a.swap(0, Ordering::Relaxed);
    let disk = take(&s.disk_bytes);
    let reads = take(&s.reads);
    let requests = take(&s.requests);
    tracing::info!(
        disk_mb_per_s = format_args!("{:.1}", disk as f64 / secs / 1e6),
        reads_per_s = format_args!("{:.0}", reads as f64 / secs),
        requests_per_s = format_args!("{:.0}", requests as f64 / secs),
        avg_read_kb = if reads > 0 { disk / reads / 1024 } else { 0 },
        coalesced = take(&s.coalesced),
        qd_avg = format_args!("{:.1}", take(&s.qd_ns) as f64 / (secs * 1e9)),
        qd_max = take(&s.max_qd),
        decode_busy = format_args!("{:.2}", take(&s.decode_ns) as f64 / (secs * 1e9)),
        decode_threads = config().decode_threads,
        decode_queue = DECODE.get().map_or(0, |d| d.lock().len()),
        held = HELD_JOBS.load(Ordering::Relaxed),
        sync_reads = take(&s.sync_reads),
        instant_reads = take(&s.instant_reads),
        issue_ms = take(&s.issue_ns) / 1_000_000,
        cache_hits = take(&s.cache_hits),
        buffer_mb = BUFFERS.load(Ordering::Relaxed) >> 20,
        "dstorage io: last {secs:.1} s"
    );
}

// The threads.

/// The completion port, once the pipeline runs.
static PORT: OnceLock<usize> = OnceLock::new();
static HELD_JOBS: AtomicUsize = AtomicUsize::new(0);
static HELD_BYTES: AtomicU64 = AtomicU64::new(0);
/// Bytes of read buffers alive (in flight, or held by requests not yet decoded).
static BUFFERS: AtomicU64 = AtomicU64::new(0);

fn running() -> bool {
    PORT.get().is_some()
}

/// Wakes the I/O thread (new requests, or room freed).
pub(super) fn wake() {
    if let Some(&port) = PORT.get() {
        // SAFETY: our port; a null overlapped is our wake packet.
        unsafe { PostQueuedCompletionStatus(port as HANDLE, 0, 0, std::ptr::null()) };
    }
}

/// A file's second handle, for overlapped reads through our port.
pub(super) struct AsyncHandle(HANDLE);

// SAFETY: a kernel handle.
unsafe impl Send for AsyncHandle {}
unsafe impl Sync for AsyncHandle {}

impl Drop for AsyncHandle {
    fn drop(&mut self) {
        // SAFETY: ours.
        unsafe { CloseHandle(self.0) };
    }
}

impl File {
    /// The overlapped handle, opened on first use; `None` if the file
    /// cannot have one (its requests then read synchronously).
    fn overlapped(&self) -> Option<HANDLE> {
        const GENERIC_READ: u32 = 0x8000_0000;
        const FILE_SHARE_READ: u32 = 1;
        const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
        const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
        let &port = PORT.get()?;
        self.overlapped
            .get_or_init(|| {
                let mut flags = FILE_FLAG_OVERLAPPED;
                if config().unbuffered {
                    flags |= FILE_FLAG_NO_BUFFERING;
                }
                // SAFETY: the file's own handle, open while `self` lives.
                let handle = unsafe {
                    ReOpenFile(self.file.as_raw_handle() as HANDLE, GENERIC_READ, FILE_SHARE_READ, flags)
                };
                if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                    let error = std::io::Error::last_os_error();
                    tracing::warn!(%error, "dstorage: no overlapped handle; this file reads synchronously");
                    return None;
                }
                let handle = AsyncHandle(handle);
                // SAFETY: both handles are ours.
                let joined = unsafe { CreateIoCompletionPort(handle.0, port as HANDLE, 1, 0) };
                if joined.is_null() {
                    let error = std::io::Error::last_os_error();
                    tracing::warn!(%error, "dstorage: the completion port refused a file");
                    return None;
                }
                Some(handle)
            })
            .as_ref()
            .map(|h| h.0)
    }
}

struct Work {
    job: Job,
    source: Source,
}

struct DecodeQueue {
    work: Mutex<VecDeque<Work>>,
    ready: Condvar,
}

impl DecodeQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Work>> {
        self.work.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(&self, items: impl IntoIterator<Item = Work>) {
        let mut queue = self.lock();
        let before = queue.len();
        queue.extend(items);
        let added = queue.len() - before;
        drop(queue);
        if added == 1 {
            self.ready.notify_one();
        } else if added > 1 {
            self.ready.notify_all();
        }
    }
}

static DECODE: OnceLock<DecodeQueue> = OnceLock::new();

/// Starts the I/O thread and the decode threads; false if they could not
/// start (the caller then starts the old workers).
pub(super) fn start(scheduler: &'static Scheduler) -> bool {
    let config = config();
    // SAFETY: a new port, no file yet.
    let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1) };
    if port.is_null() {
        tracing::warn!(error = %std::io::Error::last_os_error(), "dstorage: no completion port");
        return false;
    }
    let decode = DECODE.get_or_init(|| DecodeQueue {
        work: Mutex::new(VecDeque::new()),
        ready: Condvar::new(),
    });
    let decoders = (0..config.decode_threads)
        .filter(|i| {
            std::thread::Builder::new()
                .name(format!("dstorage decode {i}"))
                .spawn(move || decode_thread(scheduler, decode))
                .is_ok()
        })
        .count();
    if decoders == 0 {
        return false;
    }
    let port = port as usize;
    let _ = PORT.set(port);
    let io = std::thread::Builder::new()
        .name("dstorage io".into())
        .spawn(move || io_thread(scheduler, port, decode));
    if io.is_err() {
        // Decoders idle forever; nothing queued reaches them. Unreachable in
        // practice (a thread just started), said aloud anyway.
        tracing::error!("dstorage: the io thread did not start");
        return false;
    }
    tracing::info!(decoders, "dstorage: io pipeline");
    true
}

fn decode_thread(scheduler: &'static Scheduler, decode: &'static DecodeQueue) {
    loop {
        let work = {
            let mut queue = decode.lock();
            loop {
                if let Some(work) = queue.pop_front() {
                    break work;
                }
                queue = decode.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
            }
        };
        let started = Instant::now();
        let request = &work.job.request;
        let result = execute(request, work.source.bytes(request.source_size as usize));
        stats()
            .decode_ns
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let held = u64::from(request.source_size);
        drop(work.source);
        let mut lanes = scheduler.lock();
        lanes.finish(&work.job, result);
        lanes.count(&work.job, started);
        lanes.starved = false;
        drop(lanes);
        scheduler.finished.notify_all();
        HELD_JOBS.fetch_sub(1, Ordering::AcqRel);
        HELD_BYTES.fetch_sub(held, Ordering::AcqRel);
        wake();
    }
}

/// A read in flight; the overlapped structure comes first so the
/// completion's pointer is the read.
#[repr(C)]
struct ReadOp {
    overlapped: OVERLAPPED,
    file: u64,
    start: u64,
    buffer: Arc<Buffer>,
    jobs: Vec<Job>,
}

/// A file's requests waiting to be read: admission order, the request.
struct Waiting {
    handle: HANDLE,
    jobs: Vec<(u64, Job)>,
    in_flight: usize,
}

fn want(job: &Job) -> Want {
    Want {
        offset: job.request.offset,
        len: u64::from(job.request.source_size),
    }
}

struct Io {
    config: &'static Config,
    port: HANDLE,
    decode: &'static DecodeQueue,
    files: HashMap<u64, Waiting>,
    in_flight: usize,
    changed: Instant,
    order: u64,
    cache: Option<BlockCache>,
}

fn io_thread(scheduler: &'static Scheduler, port: usize, decode: &'static DecodeQueue) {
    let config = config();
    let mut io = Io {
        config,
        port: port as HANDLE,
        decode,
        files: HashMap::new(),
        in_flight: 0,
        changed: Instant::now(),
        order: 0,
        cache: (config.cache_bytes > 0).then(|| BlockCache::new(CACHE_BLOCK, config.cache_bytes)),
    };
    // SAFETY: plain data.
    let mut entries: [OVERLAPPED_ENTRY; 64] = unsafe { std::mem::zeroed() };
    loop {
        io.admit(scheduler);
        io.issue();
        let mut n = 0;
        // SAFETY: our port and array.
        let ok = unsafe {
            GetQueuedCompletionStatusEx(io.port, entries.as_mut_ptr(), 64, &mut n, u32::MAX, 0)
        };
        if ok == 0 {
            continue;
        }
        let mut done = Vec::new();
        for entry in &entries[..n as usize] {
            if entry.lpOverlapped.is_null() {
                continue; // a wake
            }
            // SAFETY: every overlapped we pass is a leaked `ReadOp`.
            let op = unsafe { Box::from_raw(entry.lpOverlapped.cast::<ReadOp>()) };
            let transferred = if entry.Internal == 0 {
                u64::from(entry.dwNumberOfBytesTransferred)
            } else {
                0
            };
            io.completed(*op, transferred, &mut done);
        }
        io.decode.push(done);
    }
}

impl Io {
    /// Takes every request the scheduler lets start, up to the holding
    /// limits; ones that read no file go straight to decode.
    fn admit(&mut self, scheduler: &'static Scheduler) {
        let mut direct = Vec::new();
        let mut lanes = scheduler.lock();
        while HELD_JOBS.load(Ordering::Acquire) < self.config.max_jobs
            && HELD_BYTES.load(Ordering::Acquire) < self.config.max_held
        {
            let Some(job) = lanes.start() else {
                break;
            };
            HELD_JOBS.fetch_add(1, Ordering::AcqRel);
            HELD_BYTES.fetch_add(u64::from(job.request.source_size), Ordering::AcqRel);
            stats().requests.fetch_add(1, Ordering::Relaxed);
            let request = &job.request;
            if request.options & MEMORY_SOURCE != 0 || request.source.is_null() || request.source_size == 0 {
                direct.push(Work { job, source: Source::Itself });
                continue;
            }
            // SAFETY: a file object we created; the title keeps it open.
            let file = &unsafe { &*request.source.cast::<Instance<File>>() }.state;
            let Some(handle) = file.overlapped() else {
                stats().sync_reads.fetch_add(1, Ordering::Relaxed);
                direct.push(Work { job, source: Source::Itself });
                continue;
            };
            if let Some(cache) = &mut self.cache
                && let Some(source) = cache.lookup(file.id, want(&job))
            {
                stats().cache_hits.fetch_add(1, Ordering::Relaxed);
                direct.push(Work { job, source });
                continue;
            }
            self.order += 1;
            self.files
                .entry(file.id)
                .or_insert_with(|| Waiting {
                    handle,
                    jobs: Vec::new(),
                    in_flight: 0,
                })
                .jobs
                .push((self.order, job));
        }
        drop(lanes);
        self.decode.push(direct);
    }

    fn note_depth(&mut self) {
        let now = Instant::now();
        let ns = now.duration_since(self.changed).as_nanos() as u64;
        stats()
            .qd_ns
            .fetch_add(ns * self.in_flight as u64, Ordering::Relaxed);
        stats().max_qd.fetch_max(self.in_flight as u64, Ordering::Relaxed);
        self.changed = now;
    }

    /// Starts reads while the queue depths allow, oldest requests first.
    fn issue(&mut self) {
        let config = self.config;
        let align = config.align();
        let mut failed = Vec::new();
        let ids: Vec<u64> = self.files.keys().copied().collect();
        for id in ids {
            // Read buffers are capped (the cache's pinned blocks aside); with
            // nothing in flight one read always starts, so this never stalls.
            let buffers = BUFFERS.load(Ordering::Relaxed);
            let cap = config.max_buffers + 2 * config.cache_bytes;
            if self.in_flight >= config.qd || (self.in_flight > 0 && buffers >= cap) {
                break;
            }
            let Some(file) = self.files.get_mut(&id) else {
                continue;
            };
            if file.jobs.is_empty() || file.in_flight >= config.qd_file {
                continue;
            }
            let wants: Vec<Want> = file.jobs.iter().map(|(_, j)| want(j)).collect();
            let mut extents = plan(&wants, config.coalesce, config.gap, align);
            extents.sort_by_key(|e| e.members.iter().map(|&m| file.jobs[m].0).min());
            let room = (config.qd - self.in_flight).min(config.qd_file - file.in_flight);
            extents.truncate(room);
            let mut taken: Vec<Option<(u64, Job)>> =
                std::mem::take(&mut file.jobs).into_iter().map(Some).collect();
            let mut reads = Vec::new();
            for extent in extents {
                let jobs: Vec<Job> = extent
                    .members
                    .iter()
                    .filter_map(|&m| taken[m].take().map(|(_, j)| j))
                    .collect();
                if jobs.len() > 1 {
                    stats().coalesced.fetch_add(jobs.len() as u64, Ordering::Relaxed);
                }
                reads.push((extent.start, extent.len, jobs));
            }
            file.jobs = taken.into_iter().flatten().collect();
            let handle = file.handle;
            for (start, len, jobs) in reads {
                match self.read(id, handle, start, len, jobs) {
                    Ok(()) => {}
                    Err(jobs) => failed.extend(jobs),
                }
            }
        }
        self.files.retain(|_, f| !f.jobs.is_empty() || f.in_flight > 0);
        // A read that could not start: its requests read on their own.
        stats().sync_reads.fetch_add(failed.len() as u64, Ordering::Relaxed);
        self.decode.push(failed.into_iter().map(|job| Work { job, source: Source::Itself }));
    }

    fn read(&mut self, file: u64, handle: HANDLE, start: u64, len: u64, jobs: Vec<Job>) -> Result<(), Vec<Job>> {
        let Some(buffer) = u32::try_from(len).ok().and_then(|_| Buffer::new(len as usize)) else {
            return Err(jobs);
        };
        // SAFETY: plain data.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.Anonymous.Anonymous.Offset = start as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (start >> 32) as u32;
        let buffer = Arc::new(buffer);
        let ptr = buffer.ptr;
        let op = Box::into_raw(Box::new(ReadOp {
            overlapped,
            file,
            start,
            buffer,
            jobs,
        }));
        self.note_depth();
        let issued = Instant::now();
        // SAFETY: the buffer and the op live until the completion comes back
        // (the op owns the buffer, and we only free it then).
        let ok = unsafe { ReadFile(handle, ptr, len as u32, std::ptr::null_mut(), op.cast()) };
        stats()
            .issue_ns
            .fetch_add(issued.elapsed().as_nanos() as u64, Ordering::Relaxed);
        // SAFETY: GetLastError right after the call.
        if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            // No completion will come.
            // SAFETY: the op we leaked above; nobody else has it.
            let op = unsafe { Box::from_raw(op) };
            return Err(op.jobs);
        }
        if ok != 0 {
            stats().instant_reads.fetch_add(1, Ordering::Relaxed);
        }
        self.in_flight += 1;
        if let Some(f) = self.files.get_mut(&file) {
            f.in_flight += 1;
        }
        stats().reads.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Cuts a finished read into its requests' sources.
    fn completed(&mut self, op: ReadOp, transferred: u64, done: &mut Vec<Work>) {
        self.note_depth();
        self.in_flight -= 1;
        if let Some(f) = self.files.get_mut(&op.file) {
            f.in_flight -= 1;
        }
        let transferred = transferred.min(op.buffer.layout.size() as u64);
        stats().disk_bytes.fetch_add(transferred, Ordering::Relaxed);
        if let Some(cache) = &mut self.cache {
            cache.insert(op.file, op.start, &op.buffer, transferred);
        }
        for job in op.jobs {
            let source = match member_bytes(op.start, transferred, want(&job)) {
                Some(range) => Source::Slice(Arc::clone(&op.buffer), range.start),
                None => {
                    stats().sync_reads.fetch_add(1, Ordering::Relaxed);
                    Source::Itself
                }
            };
            done.push(Work { job, source });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(offset: u64, len: u64) -> Want {
        Want { offset, len }
    }

    fn spans(extents: &[Extent]) -> Vec<(u64, u64, Vec<usize>)> {
        extents.iter().map(|e| (e.start, e.len, e.members.clone())).collect()
    }

    #[test]
    fn adjacent_and_near_requests_merge() {
        let wants = [w(100, 50), w(0, 100), w(150, 10), w(300, 20)];
        // Gap 100: 160 -> 300 is 140 away, stays apart.
        assert_eq!(
            spans(&plan(&wants, 1 << 20, 100, 1)),
            [(0, 160, vec![1, 0, 2]), (300, 20, vec![3])]
        );
        // Gap 200: one read through the hole.
        assert_eq!(spans(&plan(&wants, 1 << 20, 200, 1)), [(0, 320, vec![1, 0, 2, 3])]);
    }

    #[test]
    fn merging_stops_at_the_size_limit() {
        let wants: Vec<Want> = (0..10).map(|i| w(i * 100, 100)).collect();
        let extents = plan(&wants, 350, 0, 1);
        assert_eq!(
            spans(&extents),
            [(0, 300, vec![0, 1, 2]), (300, 300, vec![3, 4, 5]), (600, 300, vec![6, 7, 8]), (900, 100, vec![9])]
        );
        // A request larger than the limit reads alone.
        assert_eq!(spans(&plan(&[w(0, 10), w(10, 1000), w(1010, 10)], 100, 0, 1)), [
            (0, 10, vec![0]),
            (10, 1000, vec![1]),
            (1010, 10, vec![2])
        ]);
    }

    #[test]
    fn no_merging_when_switched_off_and_overlaps_merge() {
        let wants = [w(0, 10), w(10, 10)];
        assert_eq!(spans(&plan(&wants, 0, 1 << 20, 1)), [(0, 10, vec![0]), (10, 10, vec![1])]);
        // Overlapping and identical requests share a read.
        let wants = [w(0, 100), w(50, 100), w(50, 100), w(10, 5)];
        assert_eq!(spans(&plan(&wants, 1 << 20, 0, 1)), [(0, 150, vec![0, 3, 1, 2])]);
    }

    #[test]
    fn reads_are_aligned_and_cover_every_member() {
        let wants = [w(5000, 100), w(9000, 3000), w(70_000, 1)];
        let extents = plan(&wants, 1 << 20, 0, 4096);
        assert_eq!(spans(&extents), [(4096, 8192, vec![0, 1]), (69_632, 4096, vec![2])]);
        for e in &extents {
            assert_eq!((e.start % 4096, e.len % 4096), (0, 0));
            for &m in &e.members {
                let r = member_bytes(e.start, e.len, wants[m]).unwrap();
                assert_eq!(r.len() as u64, wants[m].len);
            }
        }
    }

    #[test]
    fn a_short_read_serves_only_what_it_delivered() {
        assert_eq!(member_bytes(1000, 500, w(1100, 100)), Some(100..200));
        assert_eq!(member_bytes(1000, 500, w(1400, 100)), Some(400..500));
        assert_eq!(member_bytes(1000, 500, w(1450, 100)), None, "past the end of the file");
        assert_eq!(member_bytes(1000, 0, w(1000, 1)), None, "a failed read");
        assert_eq!(member_bytes(1000, 500, w(900, 10)), None);
    }

    fn buffer_of(bytes: &[u8]) -> Arc<Buffer> {
        let b = Buffer::new(bytes.len()).unwrap();
        // SAFETY: our fresh allocation of this size.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.ptr, bytes.len()) };
        Arc::new(b)
    }

    #[test]
    fn the_block_cache_serves_whole_blocks_only() {
        let data: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let mut cache = BlockCache::new(16, 1024);
        // A read of 40 bytes from 16: blocks 1 and 2 whole, the rest partial.
        cache.insert(7, 16, &buffer_of(&data[16..56]), 40);
        let got = |c: &mut BlockCache, o, l| c.lookup(7, w(o, l)).map(|s| s.bytes(l as usize).unwrap().to_vec());
        assert_eq!(got(&mut cache, 20, 8), Some(data[20..28].to_vec()));
        assert_eq!(got(&mut cache, 16, 32), Some(data[16..48].to_vec()));
        assert_eq!(got(&mut cache, 40, 10), None, "block 3 was partial");
        assert_eq!(cache.lookup(8, w(20, 8)).map(|_| ()), None, "another file");
        // Blocks from two reads are stitched together.
        cache.insert(7, 48, &buffer_of(&data[48..64]), 16);
        assert_eq!(got(&mut cache, 40, 20), Some(data[40..60].to_vec()));
    }

    #[test]
    fn the_block_cache_forgets_the_oldest() {
        let data = vec![1u8; 16];
        let mut cache = BlockCache::new(16, 64); // four blocks
        for i in 0..4 {
            cache.insert(1, i * 16, &buffer_of(&data), 16);
        }
        assert!(cache.lookup(1, w(0, 1)).is_some()); // block 0 used again
        cache.insert(1, 64, &buffer_of(&data), 16);
        assert!(cache.blocks.len() <= 4);
        assert!(cache.lookup(1, w(0, 1)).is_some(), "recently used stays");
        assert!(cache.lookup(1, w(16, 1)).is_none(), "the oldest went");
    }
}
