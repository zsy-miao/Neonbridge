//! The title's placeholder memory as the runtime mapped it
//! ([`victory_core::memmap`]), and the copies of title memory that use it.
//!
//! The runtime maps every page the title streams in (`victory-hle` xmem:
//! placeholders, then views of physical pages) and tells this crate
//! ([`title_memory_placeholder`], [`crate::title_memory_remapped`],
//! [`crate::title_memory_view`]). With that record a refresh of a host copy
//! reads the mapped runs of title memory with plain copies, writes zeros for
//! the holes, and asks Windows nothing; before it, a read that met a hole
//! went through the kernel region by region, asking `VirtualQuery` about
//! each (milliseconds a question in the title's big placeholders): 1-20 s
//! frames while the title streamed (thousands of mapping changes a frame).
//! Memory the record does not hold (the title's heaps) is read as before.
//!
//! `VICTORY_COPY_FAST=0` copies, guards and asks as before (for A/B).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use victory_core::memmap::{Kind, RegionMap};

static MAP: Mutex<RegionMap> = Mutex::new(RegionMap::new());

fn map() -> std::sync::MutexGuard<'static, RegionMap> {
    MAP.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether copies and guards use the record (`VICTORY_COPY_FAST=0`: no).
pub(crate) fn fast() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_COPY_FAST").is_none_or(|v| v != "0");
        tracing::info!("title memory copies: {} (VICTORY_COPY_FAST)", if on { "by the runtime's mapping record" } else { "asking Windows, as before" });
        on
    })
}

/// The runtime reserved `[at, at + len)` as a placeholder for physical
/// pages: holes until it maps views there.
pub fn title_memory_placeholder(at: usize, len: usize) {
    map().reserve(at, len);
}

/// The runtime reserved or released `[at, at + len)` as something other than
/// a placeholder: what the record held there is gone.
pub fn title_memory_not_placeholder(at: usize, len: usize) {
    let mut m = map();
    if !m.is_empty() {
        m.release(at, len);
    }
}

/// `[at, at + len)` shows nothing now (`crate::title_memory_remapped`).
pub(crate) fn unmapped(at: usize, len: usize) {
    map().unmap(at, len);
}

/// `[at, at + len)` shows a view now (`crate::title_memory_view`).
pub(crate) fn mapped(at: usize, len: usize) {
    map().map(at, len);
    prefetch(at, len);
}

/// Whether every byte of `[at, at + len)` is placeholder memory (mapped or
/// not): memory that is not write-watched, so a write-watch asking about it
/// can only fail.
pub(crate) fn all_placeholder(at: usize, len: usize) -> bool {
    if !fast() || len == 0 {
        return false;
    }
    let m = map();
    !m.is_empty() && m.knows_all(at, len)
}

/// Whether all of `[at, at + len)` is mapped, some of it is a hole, or the
/// record cannot say (`None`, also with the switch off).
pub(crate) fn all_mapped(at: usize, len: usize) -> Option<bool> {
    if !fast() || len == 0 {
        return None;
    }
    let m = map();
    if m.is_empty() {
        return None;
    }
    let all = m.all_mapped(at, len);
    // Holes shown through the translation table
    // (`VICTORY_MEM2_GPU_ONLY_NOVIEW`) count as mapped: they show bytes,
    // read through the table or in place once made views. (The table is
    // never asked with the record's lock held: its fault handler takes the
    // record's lock with the table's held.)
    let runs = (all == Some(false)).then(|| m.runs(at, len, false));
    drop(m);
    if let Some(runs) = runs
        && crate::translate::any(at, len)
    {
        if runs.iter().any(|r| r.kind == Kind::Hole && crate::translate::bytes_in(r.start, r.len()) != r.len()) {
            return Some(false);
        }
        return (!runs.iter().any(|r| r.kind == Kind::Unknown)).then_some(true);
    }
    all
}

/// How many bytes of `[at, at + len)` are mapped, if the record knows all
/// of it (`None` otherwise, and with the switch off).
pub(crate) fn mapped_bytes(at: usize, len: usize) -> Option<u64> {
    if !fast() || len == 0 {
        return None;
    }
    let m = map();
    if m.is_empty() {
        return None;
    }
    let runs = m.runs(at, len, false);
    drop(m);
    let mut bytes = 0u64;
    for run in runs {
        match run.kind {
            Kind::Mapped => bytes += run.len() as u64,
            // Shown through the translation table: there.
            Kind::Hole => bytes += crate::translate::bytes_in(run.start, run.len()) as u64,
            Kind::Unknown => return None,
        }
    }
    Some(bytes)
}

/// What the record says of `[at, at + len)` for the guards: `None` if it
/// knows none of it; else its runs, each view on its own.
pub(crate) fn runs_by_view(at: usize, len: usize) -> Option<Vec<victory_core::memmap::Run>> {
    if !fast() || len == 0 {
        return None;
    }
    let m = map();
    if m.is_empty() || !m.knows_any(at, len) {
        return None;
    }
    Some(m.runs(at, len, true))
}

/// The run holding `at`, by the record (`None` with the switch off).
pub(crate) fn around(at: usize) -> Option<victory_core::memmap::Run> {
    if !fast() {
        return None;
    }
    let m = map();
    if m.is_empty() {
        return None;
    }
    Some(m.around(at))
}

/// The most read in one piece while the bytes are held still
/// ([`crate::watch::title_memory_reading_now`]): the title's mapping of
/// them waits that long at most (some milliseconds).
const CHUNK: usize = 4 << 20;

/// Copies title memory `[src, src + len)` into host memory at `dst`: the
/// mapped runs as they are, zeros for the holes, the rest through `os`
/// (the kernel's guarded read). False if the record knows nothing of the
/// range (the caller copies as before).
pub(crate) fn copy(dst: *mut u8, src: *const u8, len: usize, os: impl Fn(*mut u8, *const u8, usize)) -> bool {
    if len == 0 || !fast() {
        return false;
    }
    let start = src as usize;
    {
        let m = map();
        if m.is_empty() || !m.knows_any(start, len) {
            return false;
        }
    }
    let begun = std::time::Instant::now();
    add(S::Copies, 1);
    for (from, to) in victory_core::memmap::chunks(start, len, CHUNK) {
        let at = |a: usize| dst.wrapping_add(a - start);
        // The runtime changes no mapping of these bytes while they are
        // read (it waits); one under way or waiting: read through the
        // kernel, which tells a hole from a page itself.
        let Some(_still) = crate::watch::title_memory_reading_now(from, to - from) else {
            add(S::Busy, 1);
            os(at(from), from as *const u8, to - from);
            add(S::OsBytes, (to - from) as u64);
            continue;
        };
        let runs = map().runs(from, to - from, false);
        for run in runs {
            add(S::Runs, 1);
            let n = run.len();
            match run.kind {
                Kind::Mapped => {
                    // SAFETY: the record says these bytes show a mapped,
                    // committed view (the runtime maps views read-write and
                    // commits them before telling), and no change of their
                    // mapping can begin while `_still` is held. The
                    // destination is the caller's host memory of `len` bytes.
                    unsafe { std::ptr::copy_nonoverlapping(run.start as *const u8, at(run.start), n) };
                    add(S::MappedBytes, n as u64);
                }
                Kind::Hole if crate::translate::any(run.start, n) => {
                    // Pages shown through the translation table
                    // (`VICTORY_MEM2_GPU_ONLY_NOVIEW`): their bytes; the
                    // rest of the hole as below, piece by piece.
                    crate::translate::read(run.start, at(run.start), n, |a, d, k| {
                        add(S::Probes, 1);
                        let mut byte = [0u8];
                        if crate::host::read_into(a as *const u8, &mut byte) {
                            add(S::Stale, 1);
                            os(d, a as *const u8, k);
                            add(S::OsBytes, k as u64);
                        } else {
                            // SAFETY: inside the caller's host destination.
                            unsafe { d.write_bytes(0, k) };
                            add(S::HoleBytes, k as u64);
                        }
                        true
                    });
                    add(S::MappedBytes, crate::translate::bytes_in(run.start, n) as u64);
                }
                Kind::Hole => {
                    // One look through the kernel at the hole's first byte:
                    // memory the title reserved anew without the runtime
                    // (the record keeps placeholders it never saw released)
                    // reads, and is then read as before and forgotten.
                    add(S::Probes, 1);
                    let mut byte = [0u8];
                    if crate::host::read_into(run.start as *const u8, &mut byte) {
                        add(S::Stale, 1);
                        map().release(run.start, n);
                        os(at(run.start), run.start as *const u8, n);
                        add(S::OsBytes, n as u64);
                    } else {
                        // SAFETY: inside the caller's host destination.
                        unsafe { at(run.start).write_bytes(0, n) };
                        add(S::HoleBytes, n as u64);
                    }
                }
                Kind::Unknown => {
                    os(at(run.start), run.start as *const u8, n);
                    add(S::OsBytes, n as u64);
                }
            }
        }
    }
    add(S::FastUs, begun.elapsed().as_micros() as u64);
    true
}

/// Counters of the copy path, since the last stats line.
#[derive(Clone, Copy)]
pub(crate) enum S {
    /// Copies the record handled; runs they were cut into.
    Copies,
    Runs,
    /// Bytes copied plainly from mapped runs, written as zeros for holes,
    /// and read through the kernel (unknown memory, busy chunks).
    MappedBytes,
    HoleBytes,
    OsBytes,
    /// Holes looked at through the kernel, and found mapped after all.
    Probes,
    Stale,
    /// Chunks whose mapping was being changed (read through the kernel).
    Busy,
    /// Microseconds in copies the record handled.
    FastUs,
    /// Copies as before (`host::copy_mapped`), their bytes and microseconds.
    OsCopies,
    OsCopyBytes,
    OsCopyUs,
    /// System calls by kind: kernel reads, `VirtualQuery`, `VirtualProtect`,
    /// write-watch askings.
    Reads,
    Queries,
    Protects,
    Watches,
    /// Microseconds in `VirtualQuery` and `VirtualProtect` calls.
    QueryUs,
    ProtectUs,
    /// Submission flushes (`host::Batch::flush`), their microseconds, the
    /// longest, and the page faults the process took during them.
    Flushes,
    FlushUs,
    FlushMaxUs,
    FlushFaults,
    /// Write-watch askings left out: the range is all placeholder memory.
    WatchesSaved,
    /// Ranges given to the prefetch thread, and their bytes.
    Prefetched,
    PrefetchBytes,
}

const COUNTERS: usize = S::PrefetchBytes as usize + 1;
static COUNT: [AtomicU64; COUNTERS] = [const { AtomicU64::new(0) }; COUNTERS];

pub(crate) fn add(s: S, n: u64) {
    COUNT[s as usize].fetch_add(n, Relaxed);
}

pub(crate) fn max(s: S, n: u64) {
    COUNT[s as usize].fetch_max(n, Relaxed);
}

/// Times a system call of kind `s` (with its microseconds in `us`).
pub(crate) fn timed<T>(s: S, us: Option<S>, f: impl FnOnce() -> T) -> T {
    add(s, 1);
    let Some(us) = us else { return f() };
    let begun = std::time::Instant::now();
    let r = f();
    add(us, begun.elapsed().as_micros() as u64);
    r
}

/// The process's page faults so far.
#[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
pub(crate) fn page_faults() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    u64::from(c.PageFaultCount)
}

/// One line a second at most (called after flushes and at presents, so a
/// long frame still logs): what the copies did since the last line.
#[cfg(feature = "diagnostics")]
pub(crate) fn tick() {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::host::now_ms();
    let last = LAST.load(Relaxed);
    if now < last + 1000 || LAST.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    let c: Vec<u64> = COUNT.iter().map(|a| a.swap(0, Relaxed)).collect();
    let g = |s: S| c[s as usize];
    let secs = (now - last).max(1) as f64 / 1e3;
    let mb = |v: u64| v as f64 / 1e6;
    let ms = |v: u64| v as f64 / 1e3;
    let copied = g(S::MappedBytes) + g(S::OsCopyBytes);
    let copy_us = g(S::FastUs) + g(S::OsCopyUs);
    let regions = {
        let m = map();
        m.generation()
    };
    tracing::info!(
        "copy path over {secs:.1} s ({}): {} copies by the record in {} runs ({:.1} MB plain, {:.1} MB holes, {:.1} MB through \
         the kernel; {} holes looked at, {} found mapped; {} chunks busy) {:.1} ms; {} copies as before {:.1} MB {:.1} ms; \
         {:.0} MB/s; system calls {} (reads {}, queries {} {:.1} ms, protects {} {:.1} ms, write-watch {}, {} left out); \
         {} flushes {:.1} ms (longest {:.1} ms), {} page faults in them; {} prefetched {:.1} MB; mapping record changes so far {regions}",
        if fast() { "fast" } else { "as before" },
        g(S::Copies),
        g(S::Runs),
        mb(g(S::MappedBytes)),
        mb(g(S::HoleBytes)),
        mb(g(S::OsBytes)),
        g(S::Probes),
        g(S::Stale),
        g(S::Busy),
        ms(g(S::FastUs)),
        g(S::OsCopies),
        mb(g(S::OsCopyBytes)),
        ms(g(S::OsCopyUs)),
        if copy_us > 0 { copied as f64 / copy_us as f64 } else { 0.0 },
        g(S::Reads) + g(S::Queries) + g(S::Protects) + g(S::Watches),
        g(S::Reads),
        g(S::Queries),
        ms(g(S::QueryUs)),
        g(S::Protects),
        ms(g(S::ProtectUs)),
        g(S::Watches),
        g(S::WatchesSaved),
        g(S::Flushes),
        ms(g(S::FlushUs)),
        ms(g(S::FlushMaxUs)),
        g(S::FlushFaults),
        g(S::Prefetched),
        mb(g(S::PrefetchBytes)),
    );
}

/// Whether views the runtime maps are brought into memory ahead, off the
/// submitting thread (`VICTORY_COPY_PREFETCH=1`; off by default): if the
/// stats line shows the copies' time in page faults (memory paged out, or
/// pages first touched by the copy), the refresh no longer takes them.
fn prefetching() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COPY_PREFETCH").is_some_and(|v| v == "1"))
}

static PREFETCH: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
static PREFETCH_READY: std::sync::Condvar = std::sync::Condvar::new();

fn prefetch(at: usize, len: usize) {
    if !prefetching() || len == 0 {
        return;
    }
    static THREAD: std::sync::Once = std::sync::Once::new();
    THREAD.call_once(|| {
        let _ = std::thread::Builder::new().name("victory-prefetch".into()).spawn(prefetch_thread);
    });
    let mut q = PREFETCH.lock().unwrap_or_else(|e| e.into_inner());
    // Pages mapped one by one: one range for the run.
    if let Some(last) = q.last_mut()
        && last.0 + last.1 == at
    {
        last.1 += len;
    } else {
        q.push((at, len));
    }
    PREFETCH_READY.notify_one();
}

fn prefetch_thread() {
    use windows_sys::Win32::System::Memory::{PrefetchVirtualMemory, WIN32_MEMORY_RANGE_ENTRY};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    loop {
        let ranges = {
            let mut q = PREFETCH.lock().unwrap_or_else(|e| e.into_inner());
            while q.is_empty() {
                q = PREFETCH_READY.wait(q).unwrap_or_else(|e| e.into_inner());
            }
            std::mem::take(&mut *q)
        };
        // A short wait gathers the next pages of a burst into one call.
        std::thread::sleep(std::time::Duration::from_millis(2));
        for part in ranges.chunks(512) {
            let entries: Vec<WIN32_MEMORY_RANGE_ENTRY> =
                part.iter().map(|&(at, len)| WIN32_MEMORY_RANGE_ENTRY { VirtualAddress: at as *mut _, NumberOfBytes: len }).collect();
            add(S::Prefetched, part.len() as u64);
            add(S::PrefetchBytes, part.iter().map(|r| r.1 as u64).sum());
            // SAFETY: our own process; the call only reads the ranges in
            // (a range unmapped meanwhile is skipped by the system).
            unsafe { PrefetchVirtualMemory(GetCurrentProcess(), entries.len(), entries.as_ptr(), 0) };
        }
    }
}
