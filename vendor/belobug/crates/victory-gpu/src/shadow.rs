//! Host copies of title buffers (decision 0007, step 3 and "Copies of title
//! memory"). Title buffers are title memory, which the host GPU cannot
//! read, so a buffer a draw uses gets a persistent host buffer of its size
//! on first use (system memory shaders may also write), and the ranges each
//! submission reads (vertex, index and constant data, buffer views) are
//! brought up to date from title memory before it runs: the 64 KiB pages
//! the title wrote since they were last copied ([`crate::watch`]: write
//! watching, or read-only guards on physical-page views), all of it where
//! neither works. A title GPU address inside the buffer is the same offset
//! in its copy. Ranges shaders write go back to title memory after the
//! submission ([`Shadow::gpu_writes`]).
//!
//! shortcut: only the ranges draws are known to read are brought up to
//! date (vertex and index buffer views, constant buffers in tables, root
//! descriptors, buffer views in tables); buffers read through directly
//! indexed heaps keep what their first use copied.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

use crate::host::{HostList, host};
use crate::watch::PAGE;

/// A title buffer's host copy.
pub(crate) struct Shadow {
    pub(crate) buffer: ID3D12Resource,
    data: *mut u8,
    gpu: u64,
    /// Where the title buffer starts and how long it is.
    start: u64,
    len: u64,
    /// Per page of title memory from `start`'s on: the write-watch asking
    /// after which it was last copied (0: never).
    copied: Mutex<Vec<u64>>,
    /// Whether the memory is write-watched: unknown, yes, no.
    watched: AtomicU8,
    /// Ranges shaders of a submission not yet copied back may be writing:
    /// title memory there is older than the copy.
    pending: Mutex<Vec<(usize, usize)>>,
    /// The copy's bytes before the submission ran, from an offset on,
    /// covering the ranges its shaders write: what the GPU wrote differs.
    before: Mutex<Before>,
    /// The buffer is the title's memory itself ([`direct`]): nothing is
    /// copied either way.
    direct: Option<Direct>,
    /// Ranges that cannot be watched, with the frame they were copied in.
    unwatched: Mutex<std::collections::HashMap<(usize, usize), u64>>,
    /// Ranges already guarded and copied at `region_epoch(from, to)` (O(1) fast
    /// path when no guard fault/remap/rewrite occurred in those 64 MiB regions).
    clean_spans: Mutex<std::collections::HashMap<(usize, usize), u64>>,
    /// The buffer has memory only where it was brought up to date.
    sparse: Option<Sparse>,
    /// Device-local VRAM mirror (`DEFAULT`) + Wave64 GPU hash state (`card.rs`).
    card: Option<Arc<crate::card::CardState>>,
}

struct Direct {
    heap: windows::Win32::Graphics::Direct3D12::ID3D12Heap,
    /// The same memory from other 64 KiB starts, for views whose elements
    /// do not fall on multiples of their size from this one's.
    aliases: Mutex<Vec<(u64, Arc<Shadow>)>>,
    /// The title mapped other memory there since.
    dead: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct Before {
    snapshot: Option<(usize, Vec<u8>)>,
    unchanged: UnchangedPages,
}

/// Pages already compared in full against this submission's snapshot.
#[derive(Default)]
struct UnchangedPages {
    start: usize,
    end: usize,
    pages: Vec<bool>,
}

impl UnchangedPages {
    fn begin(&mut self, start: usize, end: usize) {
        if self.pages.is_empty() {
            self.start = start;
            self.end = end;
            self.pages.resize(end.div_ceil(PAGE) - start / PAGE, false);
        }
    }

    fn differs(&mut self, address: usize, current: &[u8], before: &[u8]) -> bool {
        let page = address / PAGE;
        let unchanged = &mut self.pages[page - self.start / PAGE];
        if *unchanged {
            return false;
        }
        if current != before {
            return true;
        }
        // A later range can touch bytes a partial comparison missed. At
        // the edges, only bytes covered by the snapshot can be compared.
        *unchanged = address == (page * PAGE).max(self.start)
            && address + current.len() == ((page + 1) * PAGE).min(self.end);
        false
    }

    fn clear(&mut self) {
        self.pages.clear();
    }
}

/// Snapshot memory kept for reuse (already touched, so no page faults).
static POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// An empty buffer with room for `n` bytes, from the pool if it has one.
fn pooled(n: usize) -> Vec<u8> {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let fit = pool
        .iter()
        .enumerate()
        .filter(|(_, v)| v.capacity() >= n)
        .min_by_key(|(_, v)| v.capacity())
        .map(|(i, _)| i);
    let mut v = match fit {
        Some(i) => pool.swap_remove(i),
        None => Vec::with_capacity(n),
    };
    v.clear();
    v
}

/// Snapshot memory of one tile each, kept for reuse: a world frame takes
/// thousands, and the pool above keeps 8 (the rest was allocated and freed
/// each time, a tenth of the submitting thread's time).
static TILE_POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// Tile snapshots kept at most (512 MiB).
const TILE_POOL_MOST: usize = 8192;

/// `n` empty buffers with room for a tile, under one lock of the pool.
fn pooled_tiles(n: usize) -> Vec<Vec<u8>> {
    let mut pool = TILE_POOL.lock().unwrap_or_else(|e| e.into_inner());
    (0..n)
        .map(|_| {
            let mut v = pool.pop().unwrap_or_else(|| Vec::with_capacity(PAGE));
            v.clear();
            v
        })
        .collect()
}

/// Returns tile snapshots to their pool.
fn give_back_tiles(all: impl Iterator<Item = Vec<u8>>) {
    let mut pool = TILE_POOL.lock().unwrap_or_else(|e| e.into_inner());
    for v in all {
        if pool.len() < TILE_POOL_MOST && v.capacity() >= PAGE {
            pool.push(v);
        }
    }
}

/// Of sparse copies shaders write: bytes declared, bytes of snapshots taken,
/// bytes compared after and bytes of those that differed.
#[cfg(feature = "diagnostics")]
static SPARSE_WRITES: [std::sync::atomic::AtomicU64; 4] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// Logs and resets the counts above.
#[cfg(feature = "diagnostics")]
pub(crate) fn sparse_writes_text() -> String {
    let [declared, taken, compared, differed] =
        [0, 1, 2, 3].map(|i| SPARSE_WRITES[i].swap(0, Ordering::Relaxed) as f64 / 1e6);
    format!("declared {declared:.1} MB, snapshots {taken:.1} MB, compared {compared:.1} MB, differed {differed:.1} MB")
}

/// Bytes of the snapshot memory kept for reuse.
pub(crate) fn pool_bytes() -> u64 {
    let pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    pool.iter().map(|v| v.capacity() as u64).sum()
}

/// Bytes the pool keeps at most: snapshots of the world's 174 MB heap
/// would otherwise keep 8 of that size (1.4 GB of host memory) for good.
const POOL_BYTES: usize = 512 << 20;

/// Returns a snapshot buffer to the pool (at most 8 kept, the largest,
/// and no more than [`POOL_BYTES`] in all).
fn give_back(v: Vec<u8>) {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    pool.push(v);
    pool.sort_by_key(|v| std::cmp::Reverse(v.capacity()));
    pool.truncate(8);
    let mut total = 0;
    pool.retain(|v| {
        total += v.capacity();
        total <= POOL_BYTES
    });
}

const NOT_WATCHED: u8 = 2;
const WATCHED_GUARDED: u8 = 3;

// SAFETY: D3D12 resources are free-threaded; `data` is a persistent mapping
// written only on the submitting thread.
unsafe impl Send for Shadow {}
// SAFETY: as above.
unsafe impl Sync for Shadow {}

impl Drop for Shadow {
    fn drop(&mut self) {
        if self.direct.is_some() || self.sparse.is_some() {
            return;
        }
        // SAFETY: a live resource.
        let bytes = unsafe { self.buffer.GetDesc() }.Width;
        crate::memstat::sub(&crate::memstat::SHADOWS, bytes);
        crate::memstat::sub(&crate::memstat::SHADOW_COUNT, 1);
    }
}

impl Shadow {
    /// The host GPU address of a title address inside the buffer.
    pub(crate) fn gpu(&self, address: u64) -> u64 {
        self.gpu + (address - self.start)
    }

    /// Bytes from a title address to the buffer's end.
    pub(crate) fn left(&self, address: u64) -> u64 {
        (self.start + self.len).saturating_sub(address)
    }

    /// Offset of a title address from the buffer's start.
    pub(crate) fn offset(&self, address: u64) -> u64 {
        address - self.start
    }

    /// The copy's bytes at `[address, address + len)` (inside the buffer).
    #[cfg(test)]
    pub(crate) fn host_bytes(&self, address: u64, len: usize) -> Vec<u8> {
        // SAFETY: inside the mapping; no submission runs while tests read.
        unsafe { std::slice::from_raw_parts(self.data.add(self.offset(address) as usize), len) }.to_vec()
    }

    /// Brings `len` bytes at `address` up to date when `list` is submitted,
    /// and keeps the copy alive until the list is reset.
    pub(crate) fn refresh(self: &Arc<Self>, list: &HostList, address: u64, len: u64) {
        if self.direct.is_some() {

            return;
        }
        let len = len.min(self.start + self.len - address);
        list.defer_refresh(self, address as usize, len as usize);
    }

    fn new(
        buffer: ID3D12Resource,
        data: *mut u8,
        gpu: u64,
        start: u64,
        len: u64,
        card: Option<Arc<crate::card::CardState>>,
    ) -> Shadow {
        let pages = (start + len).div_ceil(PAGE as u64) - start / PAGE as u64;
        // SAFETY: a live resource.
        let bytes = unsafe { buffer.GetDesc() }.Width;
        crate::memstat::add(&crate::memstat::SHADOWS, bytes);
        crate::memstat::add(&crate::memstat::SHADOW_COUNT, 1);
        Shadow {
            buffer,
            data,
            gpu,
            start,
            len,
            copied: Mutex::new(vec![0; pages as usize]),
            watched: AtomicU8::new(0),
            pending: Mutex::default(),
            before: Mutex::default(),
            direct: None,
            unwatched: Mutex::default(),
            clean_spans: Mutex::default(),
            sparse: None,
            card,
        }
    }

    pub(crate) fn is_card(&self) -> bool {
        self.card.is_some()
    }

    pub(crate) fn card_state(&self) -> Option<Arc<crate::card::CardState>> {
        self.card.clone()
    }

    pub(crate) fn inside_offsets(&self, address: usize, len: usize) -> (usize, usize) {
        self.inside(address, len)
    }

    /// Merges GPU-modified 64 KiB pieces (fetched from VRAM into `runs`:
    /// `(first_piece, count, readback_ptr)`) into title memory and `card.staging`.
    /// Returns bytes written to title memory.
    pub(crate) fn merge_card_pieces(
        &self,
        card: &Arc<crate::card::CardState>,
        runs: &[(u32, u32, usize)],
    ) -> usize {
        let start = self.start as usize;
        let shadow_len = self.len as usize;
        let mode = self.watched.load(Ordering::Relaxed);
        let mut written = 0usize;
        let mut title_buf = pooled(PAGE);
        title_buf.resize(PAGE, 0);
        let mut synced_pages: Vec<usize> = Vec::new();
        let mut min_addr = usize::MAX;
        let mut max_addr = 0usize;

        for &(first_piece, count, rb_ptr) in runs {
            for k in 0..count {
                let piece = (first_piece + k) as usize;
                let off = piece * PAGE;
                if off >= shadow_len {
                    break;
                }
                let n = PAGE.min(shadow_len - off);
                let title_addr = start + off;
                // SAFETY: `rb_ptr` holds `count * PAGE` bytes fetched from VRAM;
                // `self.data` maps `card.staging` of at least `shadow_len` bytes.
                let gpu_new = unsafe {
                    std::slice::from_raw_parts((rb_ptr + (k as usize) * PAGE) as *const u8, n)
                };
                let staging_old =
                    unsafe { std::slice::from_raw_parts_mut(self.data.add(off), n) };
                if gpu_new == staging_old {
                    continue;
                }
                let title_slice = &mut title_buf[..n];
                if !crate::host::read_into(title_addr as *const u8, title_slice) {
                    // Unmapped in title memory: still sync staging to VRAM so
                    // future diffs compare against the latest GPU bytes.
                    staging_old.copy_from_slice(gpu_new);
                    continue;
                }
                let (dirty_span, cpu_modified) =
                    merge_gpu_writes_card(title_slice, staging_old, gpu_new);
                if let Some((dirty_start, dirty_end)) = dirty_span {
                    let dirty_len = dirty_end - dirty_start;
                    if mode != 1 {
                        crate::watch::writing(title_addr + dirty_start, dirty_len);
                    }
                    crate::host::write_mapped(
                        (title_addr + dirty_start) as *mut u8,
                        title_slice[dirty_start..].as_ptr(),
                        dirty_len,
                    );
                    written += dirty_len;
                }
                if cpu_modified {
                    // Concurrent CPU write on the same page: upload merged
                    // `staging` back to VRAM before the next submission.
                    card.mark_dirty(off, off + n);
                }
                let page_lo = title_addr.div_ceil(PAGE);
                let page_hi = (title_addr + n) / PAGE;
                for p in page_lo..page_hi {
                    synced_pages.push(p);
                }
                min_addr = min_addr.min(title_addr);
                max_addr = max_addr.max(title_addr + n);
            }
        }
        give_back(title_buf);

        if !synced_pages.is_empty() && mode != NOT_WATCHED && min_addr < max_addr {
            let (first, last) = (min_addr.div_ceil(PAGE), max_addr / PAGE);
            if first < last {
                let res = if mode == WATCHED_GUARDED {
                    crate::watch::writes_guarded(first * PAGE, last * PAGE)
                } else {
                    crate::watch::writes(first * PAGE, last * PAGE, true)
                };
                if let Some((now, _)) = res {
                    let base = start / PAGE;
                    let mut copied = self.copied.lock().unwrap_or_else(|e| e.into_inner());
                    for page in synced_pages {
                        if let Some(slot) = copied.get_mut(page.wrapping_sub(base)) {
                            *slot = now;
                        }
                    }
                }
            }
        }
        written
    }

    pub(crate) fn finish_card_pending(&self, declared: &[(usize, usize)]) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for &(address, len) in declared {
            pending.retain(|&r| r != (address, address + len));
        }
    }

    #[cfg(test)]
    pub(crate) fn new_card_test(
        card: Arc<crate::card::CardState>,
        data: *mut u8,
        start: u64,
        len: u64,
    ) -> Arc<Self> {
        let buffer = card.vram.clone();
        // SAFETY: live VRAM resource.
        let gpu = unsafe { buffer.GetGPUVirtualAddress() };
        let shadow = Shadow::new(buffer, data, gpu, start, len, Some(card));
        shadow.watched.store(NOT_WATCHED, Ordering::Relaxed);
        Arc::new(shadow)
    }

    #[cfg(test)]
    pub(crate) fn copy_for_test(&self, from: usize, to: usize) -> usize {
        self.copy(from, to)
    }

    /// Whether the buffer is the title's memory itself.
    pub(crate) fn is_direct(&self) -> bool {
        self.direct.is_some()
    }

    pub(crate) fn is_sparse(&self) -> bool {
        self.sparse.is_some()
    }

    /// The same for copies that share their memory.
    pub(crate) fn root_key(&self) -> usize {
        self.sparse_root()
            .map_or(std::ptr::from_ref(self) as usize, |(r, _)| std::ptr::from_ref(r) as usize)
    }

    /// Whether the title mapped other memory there since.
    pub(crate) fn is_dead(&self) -> bool {
        self.direct
            .as_ref()
            .is_some_and(|d| d.dead.load(Ordering::Acquire))
    }

    /// This memory from a start `address` is a multiple of `element` bytes
    /// after.
    fn on_element(self: &Arc<Self>, address: u64, element: u64) -> Option<Arc<Shadow>> {
        if element == 0 || (address - self.start) % element == 0 {
            return Some(self.clone());
        }
        let direct = self.direct.as_ref()?;
        let (page, first) = (PAGE as u64, self.start);
        let mut base = first + page;
        while base <= address && base - first <= 4096 * page {
            if (address - base) % element == 0 {
                let mut aliases = direct.aliases.lock().unwrap_or_else(|e| e.into_inner());
                if let Some((_, s)) = aliases.iter().find(|(b, _)| *b == base) {
                    return Some(s.clone());
                }
                let made = Arc::new(placed(&direct.heap, base - first, base, first + self.len - base)?);
                aliases.push((base, made.clone()));
                return Some(made);
            }
            base += page;
        }
        None
    }

    /// `[address, address + len)` cut to the buffer, as offsets in the copy.
    fn inside(&self, address: usize, len: usize) -> (usize, usize) {
        let (start, end) = (self.start as usize, (self.start + self.len) as usize);
        let from = address.clamp(start, end);
        (from - start, (address + len).clamp(from, end) - start)
    }

    /// Shaders of `list` may write `[address, address + len)` of the copy
    /// (unordered access): after the list's submission has run, the range
    /// is copied back into title memory, where the title reads it; until
    /// then no refresh copies title memory over it. Call after the
    /// [`Shadow::refresh`] that gives the shaders the title's data.
    ///
    /// shortcut: what shaders wrote is found by keeping the range's bytes
    /// before the submission (a copy per declared range and submission);
    /// a GPU-side dirty mask would save that if big ranges make it slow.
    pub(crate) fn gpu_writes(self: &Arc<Self>, list: &HostList, address: u64, len: u64) {
        if self.direct.is_some() {
            return;
        }
        let len = len.min(self.left(address));
        list.defer_gpu_write(self, address as usize, len as usize);
    }

    /// At submission, after the range's refresh: a submitted list's shaders
    /// write this range. Its bytes are kept to tell later what the GPU wrote:
    /// one snapshot per copy and submission, grown to cover every range the
    /// submission declares (the world declares overlapping views of its
    /// 174 MB heap dozens of times), in reused memory (fresh snapshots cost
    /// ~2.3 million page faults per world frame).
    pub(crate) fn writing(&self, address: usize, len: usize) {
        if self.direct.is_some() {
            return;
        }
        if self.card.is_some() {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((address, address + len));
            return;
        }
        if let Some((root, s)) = self.sparse_root() {
            return root.sparse_writing(s, address, len);
        }
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((address, address + len));
        let (from, to) = self.inside(address, len);
        // SAFETY: inside the mapping.
        let host =
            |a: usize, b: usize| unsafe { std::slice::from_raw_parts(self.data.add(a), b - a) };
        let mut before = self.before.lock().unwrap_or_else(|e| e.into_inner());
        let before = &mut before.snapshot;
        let (start, end) = match &*before {
            Some((at, bytes)) => ((*at).min(from), (at + bytes.len()).max(to)),
            None => (from, to),
        };
        if let Some((at, bytes)) = &*before
            && (*at, at + bytes.len()) == (start, end)
        {
            return;
        }
        // Parts newly covered from the copy, the rest as it was taken.
        let mut grown = pooled(end - start);
        match before.take() {
            Some((at, bytes)) => {
                grown.extend_from_slice(host(start, at));
                grown.extend_from_slice(&bytes);
                grown.extend_from_slice(host(at + bytes.len(), end));
                give_back(bytes);
            }
            None => grown.extend_from_slice(host(start, end)),
        }
        *before = Some((start, grown));
    }

    /// After the submission ran: copies the bytes its shaders wrote back
    /// into title memory (unmapped pages left alone), and only those: the
    /// title's threads write the same buffers while the GPU runs (its
    /// command memory shares the big GPU heap with what shaders write), and
    /// putting back the copy's older bytes there broke the title's own
    /// records (a fence list read as -1, title+0x4e9b54a). Returns the bytes
    /// written.
    pub(crate) fn copy_back(&self, address: usize, len: usize) -> usize {
        if self.direct.is_some() || self.card.is_some() {
            return 0;
        }
        if let Some((root, s)) = self.sparse_root() {
            return root.sparse_back(s, address, len);
        }
        let mut before = self.before.lock().unwrap_or_else(|e| e.into_inner());
        let Before {
            snapshot,
            unchanged,
        } = &mut *before;
        let mut written = 0;
        let (from, to) = self.inside(address, len);
        let start = self.start as usize;
        if let Some((taken, before)) = &*snapshot
            && *taken <= from
            && to <= taken + before.len()
            && from < to
        {
            unchanged.begin(start + taken, start + taken + before.len());
            let before = &before[from - taken..to - taken];
            // By 64 KiB piece: the copy compared with its bytes before the
            // submission, title memory read only where the GPU wrote.
            // An unchanged page is never modified by copy-back, so later
            // overlapping ranges can skip it for the rest of the submission.
            let mode = self.watched.load(Ordering::Relaxed);
            let mut title: Option<Vec<u8>> = None;
            let mut synced = Vec::new();
            let mut at = from;
            while at < to {
                let n = (PAGE - (start + at) % PAGE).min(to - at);
                // SAFETY: inside the mapping; only the submitting thread
                // writes it.
                let host = unsafe { std::slice::from_raw_parts_mut(self.data.add(at), n) };
                let old = &before[at - from..at - from + n];
                if unchanged.differs(start + at, host, old) {
                    let buf = title.get_or_insert_with(|| {
                        let mut v = pooled(PAGE);
                        v.resize(PAGE, 0);
                        v
                    });
                    let title_slice = &mut buf[..n];
                    if crate::host::read_into((start + at) as *const u8, title_slice) {
                        // Title memory with the GPU's bytes put in; the copy
                        // takes the title's bytes in turn.
                        if let Some((dirty_start, dirty_end)) =
                            merge_gpu_writes(title_slice, host, old)
                        {
                            let dirty_len = dirty_end - dirty_start;
                            if mode != 1 {
                                crate::watch::writing(start + at + dirty_start, dirty_len);
                            }
                            crate::host::write_mapped(
                                (start + at + dirty_start) as *mut u8,
                                title_slice[dirty_start..].as_ptr(),
                                dirty_len,
                            );
                            written += dirty_len;
                        }
                        synced.push((start + at) / PAGE);
                    }
                }
                at += n;
            }
            if let Some(buf) = title {
                give_back(buf);
            }
            // Pages made equal to title memory are current in the copy, so
            // our own write is not taken for the title's; others keep what
            // they had (a title write there shows at the next refresh).
            let (first, last) = ((start + from).div_ceil(PAGE), (start + to) / PAGE);
            if first < last && !synced.is_empty() && mode != NOT_WATCHED {
                let res = if mode == WATCHED_GUARDED {
                    crate::watch::writes_guarded(first * PAGE, last * PAGE)
                } else {
                    crate::watch::writes(first * PAGE, last * PAGE, true)
                };
                if let Some((now, _)) = res {
                    let base = start / PAGE;
                    let mut copied = self.copied.lock().unwrap_or_else(|e| e.into_inner());
                    for page in synced.into_iter().filter(|p| (first..last).contains(p)) {
                        copied[page - base] = now;
                    }
                }
            }
        }
        // Every submitted step that declared the range is done with it (one
        // submission runs at a time), and the snapshot with the last range.
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|&r| r != (address, address + len));
        if pending.is_empty()
            && let Some((_, bytes)) = snapshot.take()
        {
            unchanged.clear();
            give_back(bytes);
        }
        written
    }

    /// At submission: copies what changed of `[address, address + len)`
    /// since it was last copied (all of it if the memory is not
    /// write-watched). Returns the bytes copied.
    pub(crate) fn update(&self, address: usize, len: usize) -> usize {
        self.update_lazily(address, len, false)
    }

    /// [`Shadow::update`]; `lazy`: the tiles of a sparse copy that did not
    /// change the last times are looked at less often.
    pub(crate) fn update_lazily(&self, address: usize, len: usize, lazy: bool) -> usize {
        if self.direct.is_some() {
            return 0;
        }
        if let Some(root) = self.sparse.as_ref().and_then(|s| s.of.as_ref()) {
            return root.update_lazily(address, len, lazy);
        }
        let (start, end) = (self.start as usize, (self.start + self.len) as usize);
        let (from, to) = (address.max(start), (address + len).min(end));
        if from >= to {
            return 0;
        }
        let mut busy: Vec<(usize, usize)> = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .filter(|&(a, e)| a < to && e > from)
            .collect();
        if !busy.is_empty() {
            // Around ranges shaders are writing: the gaps, each as below
            // (in the world most submissions have such ranges; copying the
            // gaps whole cost ~1 s per frame).
            busy.sort_unstable();
            let (mut at, mut bytes) = (from, 0);
            for (a, e) in busy {
                if a > at {
                    bytes += self.update_free(at, a, lazy);
                }
                at = at.max(e);
            }
            if at < to {
                bytes += self.update_free(at, to, lazy);
            }
            return bytes;
        }
        self.update_free(from, to, lazy)
    }

    /// [`Shadow::update`] of `[from, to)` (inside the buffer) where no
    /// shader writes.
    fn update_free(&self, from: usize, to: usize, lazy: bool) -> usize {
        // Diagnostics (`VICTORY_ALWAYS_COPY`): 1 no write watch and no
        // guard, every copy taken again once a frame, every tile looked at
        // (slow); 2 only every tile looked at (no waiting 2 to 16 frames for
        // one that had not changed); 3 only guarded memory taken again each
        // frame (a guard does not see a write through another view of the
        // page). They tell which of these leaves a copy stale.
        static ALWAYS: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
        let always = *ALWAYS
            .get_or_init(|| std::env::var("VICTORY_ALWAYS_COPY").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
        let mut mode = self.watched.load(Ordering::Relaxed);
        let forced = always == 1 || always == 3 && mode == WATCHED_GUARDED;
        if forced {
            mode = NOT_WATCHED;
        }
        let lazy = lazy && !forced && always != 2;
        if mode == WATCHED_GUARDED {
            let ge = crate::watch::region_epoch(from, to);
            {
                let spans = self.clean_spans.lock().unwrap_or_else(|e| e.into_inner());
                if spans.get(&(from, to)) == Some(&ge) {
                    #[cfg(feature = "diagnostics")]
                    {
                        use crate::copystat::{C, add};
                        add(C::Checked, (to - from) as u64);
                        add(C::Unchanged, (to - from) as u64);
                    }
                    return 0;
                }
            }
            if let Some((now, written)) = crate::watch::writes_guarded(from, to) {
                let copied = self.copy_stale(from, to, now, &written);
                if crate::watch::region_epoch(from, to) == ge {
                    let mut spans = self.clean_spans.lock().unwrap_or_else(|e| e.into_inner());
                    if spans.len() >= 8192 {
                        spans.clear();
                    }
                    spans.insert((from, to), ge);
                }
                #[cfg(feature = "diagnostics")]
                {
                    use crate::copystat::{C, add};
                    add(C::Checked, (to - from) as u64);
                    add(C::Unchanged, (to - from).saturating_sub(copied) as u64);
                }
                return copied;
            }
        } else if mode != NOT_WATCHED {
            // Probe write-watch first; if that fails and guard succeeds, mark
            // this shadow as guarded so subsequent calls skip GetWriteWatch.
            if let Some((now, written)) = crate::watch::writes(from, to, false) {
                self.watched.store(1, Ordering::Relaxed);
                let copied = self.copy_stale(from, to, now, &written);
                #[cfg(feature = "diagnostics")]
                {
                    use crate::copystat::{C, add};
                    add(C::Checked, (to - from) as u64);
                    add(C::Unchanged, (to - from).saturating_sub(copied) as u64);
                }
                return copied;
            }
            let ge = crate::watch::region_epoch(from, to);
            if let Some((now, written)) = crate::watch::writes_guarded(from, to) {
                self.watched.store(WATCHED_GUARDED, Ordering::Relaxed);
                let copied = self.copy_stale(from, to, now, &written);
                if crate::watch::region_epoch(from, to) == ge {
                    let mut spans = self.clean_spans.lock().unwrap_or_else(|e| e.into_inner());
                    if spans.len() >= 8192 {
                        spans.clear();
                    }
                    spans.insert((from, to), ge);
                }
                #[cfg(feature = "diagnostics")]
                {
                    use crate::copystat::{C, add};
                    add(C::Checked, (to - from) as u64);
                    add(C::Unchanged, (to - from).saturating_sub(copied) as u64);
                }
                return copied;
            }
            // A sparse shadow spans a 2 GiB chunk that may contain both
            // watchable mapped/private regions and unmapped holes: one
            // unwatchable sub-range must not permanently disable watching for
            // the rest of the chunk.
            if self.sparse.is_none() {
                self.watched.store(NOT_WATCHED, Ordering::Relaxed);
            }
        }
        // shortcut: memory that cannot be watched (unmapped, mostly) is copied
        // once a frame.
        let frame = crate::queue::presented() + 1;
        {
            let mut seen = self.unwatched.lock().unwrap_or_else(|e| e.into_inner());
            if !cfg!(test) && seen.insert((from, to), frame) == Some(frame) {
                return 0;
            }
        }
        if let Some(s) = self.sparse.as_ref().filter(|_| !cfg!(test)) {
            return self.copy_tiles(s, from, to, frame, lazy);
        }
        self.copy(from, to);
        to - from
    }

    /// Copies title memory `[from, to)` into the tiles of a sparse copy where
    /// the title changed it since the last time: what shaders wrote into the
    /// copy only is left as it is. `lazy`: not the tiles whose turn it is not.
    ///
    /// shortcut: a tile that did not change when looked at twice is next
    /// looked at after 2, 4 and at most 16 frames, unless something new reads
    /// it: the title fills memory, then makes views of it.
    fn copy_tiles(&self, s: &Sparse, from: usize, to: usize, frame: u64, lazy: bool) -> usize {
        thread_local! {
            static READ: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0; PAGE]);
        }
        let mut bytes = 0;
        // The title's unmapped memory found last: tiles of it have no memory
        // here either (reads give zeros). Tens of thousands of tiles a frame
        // were zeros copied, 1.4 GB of them.
        let mut hole = 0..0;
        let mut at = from;
        while at < to {
            let n = (PAGE - at % PAGE).min(to - at);
            let next = at + n;
            let tile = ((at - self.start as usize) / PAGE) as u32;
            let whole = at - at % PAGE..at - at % PAGE + PAGE;
            let look = {
                let mut looks = s.looks.lock().unwrap_or_else(|e| e.into_inner());
                looks.entry(tile).or_default().clone()
            };
            let mut look = look.lock().unwrap_or_else(|e| e.into_inner());
            if lazy && n == PAGE && frame < look.next || look.hole && frame < look.next {
                at = next;
                continue;
            }
            if !(hole.contains(&whole.start) && whole.end <= hole.end) {
                hole = match unmapped(whole.start) {
                    Some(end) => whole.start..end,
                    None => 0..0,
                };
            }
            if whole.start >= hole.start && whole.end <= hole.end {
                if !look.hole {
                    look.hole = true;
                    look.blocks = [0; PAGE / BLOCK];
                    let tiles = s.tiles.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(t) = tiles.get(&tile) {
                        // SAFETY: the tile's mapping.
                        unsafe { std::ptr::write_bytes(t.data as *mut u8, 0, PAGE) };
                    }
                }
                // Memory committed later is copied by the next frame: zeros
                // read for what the title filled can hang the GPU.
                look.next = frame + 1;
                at = next;
                continue;
            }
            look.hole = false;
            let Some(data) = self.tile(s, at) else {
                at = next;
                continue;
            };
            let data = (data + at % PAGE) as *mut u8;
            bytes += n;
            let mut same = true;
            READ.with_borrow_mut(|read| {
                if !crate::host::read_into(at as *const u8, &mut read[..n]) {
                    crate::host::copy_mapped(read.as_mut_ptr(), at as *const u8, n, false);
                }
                let first = at % PAGE;
                let mut done = 0;
                while done < n {
                    let block = (first + done) / BLOCK;
                    let len = (BLOCK - (first + done) % BLOCK).min(n - done);
                    let part = &read[done..done + len];
                    let known = &mut look.blocks[block];
                    let changed = if len == BLOCK {
                        let now = mixed(part);
                        std::mem::replace(known, now) != now
                    } else {
                        // Part of a block: not known any more.
                        *known = 0;
                        true
                    };
                    if changed {
                        same = false;
                        // SAFETY: the tile's mapping; only submissions write it.
                        unsafe { std::ptr::copy_nonoverlapping(part.as_ptr(), data.add(done), len) };
                    }
                    done += len;
                }
            });
            if n == PAGE {
                look.still = if same { (look.still + 1).min(5) } else { 0 };
                // Not all in the same frame.
                let wait = match look.still {
                    0 | 1 => 1,
                    k => (1 << (k - 1)) - u64::from(tile) % 2,
                };
                look.next = frame + wait;
            }
            at = next;
        }
        bytes
    }

    /// Copies the pages of `[from, to)` written after their last copy
    /// (`written`: last writes by page, in page order; `now`: this
    /// asking), whole pages within the buffer.
    fn copy_stale(&self, from: usize, to: usize, now: u64, written: &[(usize, u64)]) -> usize {
        let base = self.start as usize / PAGE;
        let mut copied = self.copied.lock().unwrap_or_else(|e| e.into_inner());
        let mut writes = written.iter().peekable();
        let (mut bytes, mut run) = (0, None);
        let last = (to - 1) / PAGE;
        for page in from / PAGE..=last {
            let write = match writes.peek() {
                Some(&&(p, w)) if p == page => {
                    writes.next();
                    w
                }
                _ => 0,
            };
            let at = &mut copied[page - base];
            if *at == 0 || write > *at {
                // A page only partly in the range is copied in part: it
                // stays stale for the rest.
                let (start, end) = (self.start as usize, (self.start + self.len) as usize);
                if (page * PAGE).max(start) >= from && ((page + 1) * PAGE).min(end) <= to {
                    *at = now;
                }
                run.get_or_insert(page);
            } else if let Some(first) = run.take() {
                bytes += self.copy((first * PAGE).max(from), (page * PAGE).min(to));
            }
        }
        // Cut to the range: next to it shaders may be writing the copy.
        if let Some(first) = run {
            bytes += self.copy((first * PAGE).max(from), ((last + 1) * PAGE).min(to));
        }
        bytes
    }

    /// Copies title memory `[from, to)`, cut to the buffer, into the copy.
    fn copy(&self, from: usize, to: usize) -> usize {
        let (start, end) = (self.start as usize, (self.start + self.len) as usize);
        let (from, to) = (from.max(start), to.min(end));
        if let Some(s) = &self.sparse {
            self.each_tile(s, from, to, |at, data, n| {
                crate::host::copy_mapped(data, at as *const u8, n, false);
            });
            return to - from;
        }
        // SAFETY: inside the mapping (`from` and `to` are within the buffer).
        let at = unsafe { self.data.add(from - start) };
        crate::host::copy_mapped(at, from as *const u8, to - from, false);
        if let Some(card) = &self.card {
            card.mark_dirty(from - start, to - start);
        }
        to - from
    }
}

/// Windows of title memory the host GPU reads and writes in place, by start:
/// each a run of committed pages of one allocation.
static DIRECT: Mutex<BTreeMap<u64, Arc<Shadow>>> = Mutex::new(BTreeMap::new());

fn windows_of_memory() -> std::sync::MutexGuard<'static, BTreeMap<u64, Arc<Shadow>>> {
    DIRECT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Buffers of windows gone since (by interface address): views of them are
/// made again.
static GONE: Mutex<std::collections::BTreeSet<usize>> = Mutex::new(std::collections::BTreeSet::new());

/// Whether a host buffer is one of a window gone since.
pub(crate) fn gone(buffer: &ID3D12Resource) -> bool {
    use windows::core::Interface;
    let gone = GONE.lock().unwrap_or_else(|e| e.into_inner());
    !gone.is_empty() && gone.contains(&(buffer.as_raw() as usize))
}

/// A buffer over part of a heap.
fn placed(
    heap: &windows::Win32::Graphics::Direct3D12::ID3D12Heap,
    offset: u64,
    start: u64,
    len: u64,
) -> Option<Shadow> {
    use windows::Win32::Graphics::Direct3D12::*;
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: len,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: a live heap that holds the range.
    let made = unsafe {
        host()?.device.CreatePlacedResource(
            heap,
            offset,
            &desc,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut buffer,
        )
    };
    if let Err(error) = made {
        tracing::warn!(%error, start = format_args!("{start:#x}"), len, "no buffer over title memory");
        return None;
    }
    let buffer = buffer?;
    crate::host::name(&buffer, || format!("title memory {start:#x}+{len:#x} in place"));
    // SAFETY: a live buffer.
    let gpu = unsafe { buffer.GetGPUVirtualAddress() };
    Some(Shadow {
        buffer,
        data: start as *mut u8,
        gpu,
        start,
        len,
        copied: Mutex::default(),
        watched: AtomicU8::new(NOT_WATCHED),
        pending: Mutex::default(),
        before: Mutex::default(),
        direct: Some(Direct {
            heap: heap.clone(),
            aliases: Mutex::default(),
            dead: std::sync::atomic::AtomicBool::new(false),
        }),
        unwatched: Mutex::default(),
        clean_spans: Mutex::default(),
        sparse: None,
        card: None,
    })
}

/// How far a window reaches at most (host heaps end below 4 GiB).
const WINDOW_REACH: u64 = 1 << 30;

/// The run of committed pages of one allocation that holds `[from, to)`, in
/// whole 64 KiB pages.
fn run_of(from: u64, to: u64) -> Option<(u64, u64)> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_READONLY, PAGE_READWRITE, VirtualQuery,
    };
    let query = |at: u64| {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: a query of our own address space.
        let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
        (n != 0).then_some(info)
    };
    let usable = |i: &MEMORY_BASIC_INFORMATION| {
        i.State == MEM_COMMIT
            && i.Type != MEM_IMAGE
            && matches!(i.Protect, PAGE_READWRITE | PAGE_READONLY)
    };
    let first = query(from).filter(usable)?;
    let allocation = first.AllocationBase as u64;
    let low = (from & !(WINDOW_REACH - 1)).max(allocation);
    let high = to.checked_next_multiple_of(WINDOW_REACH)?;
    // A query gives the region from the address up, not down: from the
    // bottom on.
    let (mut start, mut end) = (low, low);
    while end < high
        && let Some(i) = query(end)
        && i.AllocationBase as u64 == allocation
    {
        let next = i.BaseAddress as u64 + i.RegionSize as u64;
        if !usable(&i) {
            if end > from {
                break;
            }
            start = next;
        }
        end = next;
    }
    let page = PAGE as u64;
    let (start, end) = (start.max(low).next_multiple_of(page), end.min(high) & !(page - 1));
    // Not short views: the title maps and unmaps those (streaming), which
    // the host refuses while the GPU uses them.
    if first.Type == windows_sys::Win32::System::Memory::MEM_MAPPED && end - start < 16 << 20 {
        return None;
    }
    (start <= from && end >= to).then_some((start, end))
}

/// Whether `[address, end)` lies in a window made already.
pub(crate) fn in_place(address: u64, end: u64) -> bool {
    windows_of_memory()
        .range(..=address)
        .rev()
        .take(8)
        .any(|(_, s)| s.start + s.len >= end)
}

/// The window of title memory holding `[address, end)`, made on first use.
/// `None` where the host GPU cannot use the memory in place: parts of it
/// not committed, or of several allocations (views of physical pages).
fn direct(address: u64, end: u64) -> Option<Arc<Shadow>> {
    use windows::Win32::Graphics::Direct3D12::{ID3D12Device13, ID3D12Heap};
    use windows::core::Interface;
    static DEVICE: std::sync::OnceLock<Option<ID3D12Device13>> = std::sync::OnceLock::new();
    if cfg!(test) {
        return None;
    }
    let device = DEVICE.get_or_init(|| {
        // Diagnostics: copies, as without this.
        let on = std::env::var_os("VICTORY_COPY_MEMORY").is_none();
        let device: Option<ID3D12Device13> = if on {
            host().and_then(|h| h.device.cast().ok())
        } else {
            None
        };
        tracing::info!(on = device.is_some(), "title memory used in place by the host GPU");
        device
    });
    let device = device.as_ref()?;
    let mut map = windows_of_memory();
    if let Some(s) = map
        .range(..=address)
        .rev()
        .take(8)
        .map(|(_, s)| s)
        .find(|s| s.start + s.len >= end)
    {
        return Some(s.clone());
    }
    // Asked over and over for the same memory: not looked at each time.
    static REFUSED: Mutex<BTreeMap<u64, (u64, u64)>> = Mutex::new(BTreeMap::new());
    let frame = crate::queue::presented();
    let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(e, f)) = refused.get(&address)
        && e == end
        && f + 30 > frame
    {
        return None;
    }
    let page = PAGE as u64;
    let window = run_of(address & !(page - 1), end.next_multiple_of(page)).and_then(|(start, stop)| {
        // Read-only guards would refuse the GPU's writes.
        crate::watch::writing(start as usize, (stop - start) as usize);
        // SAFETY: committed memory; the heap keeps its pages.
        let heap: Result<ID3D12Heap, _> = unsafe {
            device.OpenExistingHeapFromAddress1(start as *const _, (stop - start) as usize)
        };
        match heap {
            Ok(heap) => placed(&heap, 0, start, stop - start),
            Err(error) => {
                tracing::warn!(%error, start = format_args!("{start:#x}"), len = stop - start, "title memory refused by the host GPU");
                None
            }
        }
    });
    let Some(window) = window else {
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count <= 16 || count.is_power_of_two() {
            tracing::info!(
                count,
                address = format_args!("{address:#x}"),
                len = end - address,
                "title memory not usable in place: copied"
            );
            if count <= 3 || address > 0xa_0000_0000 {
                use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
                let (mut at, mut text) = (address, String::new());
                while at < end && text.len() < 1500 {
                    let mut i = MEMORY_BASIC_INFORMATION::default();
                    // SAFETY: a query of our own address space.
                    if unsafe { VirtualQuery(at as *const _, &mut i, size_of_val(&i)) } == 0 {
                        break;
                    }
                    text.push_str(&format!(
                        " {:x}+{:x}@{:x}:{:x}/{:x}/{:x}",
                        i.BaseAddress as usize, i.RegionSize, i.AllocationBase as usize, i.State, i.Type, i.Protect
                    ));
                    at = i.BaseAddress as u64 + i.RegionSize as u64;
                }
                tracing::info!("its regions:{text}");
            }
        }
        refused.insert(address, (end, frame));
        return None;
    };
    let count = map.len() + 1;
    if count <= 16 || count.is_power_of_two() {
        tracing::info!(
            count,
            start = format_args!("{:#x}", window.start),
            len = window.len,
            "title memory in place"
        );
    }
    let window = Arc::new(window);
    map.insert(window.start, window.clone());
    Some(window)
}

/// Where the unmapped title memory `address` is in ends, if it is.
fn unmapped(address: usize) -> Option<usize> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, VirtualQuery,
    };
    // SAFETY: the query looks at an address and fills a local structure.
    unsafe {
        let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        let size = size_of_val(&info);
        if VirtualQuery(address as *const _, &mut info, size) != size {
            return Some((address | (CHUNK as usize - 1)) + 1);
        }
        (info.State != MEM_COMMIT || info.Protect == PAGE_NOACCESS)
            .then(|| info.BaseAddress as usize + info.RegionSize)
    }
}

/// Tiles of `[start, end)` known to have no title memory are looked at again.
pub(crate) fn filled(start: u64, end: u64) {
    let over: Vec<Arc<Shadow>> = SPARSE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(chunk, _)| *chunk * CHUNK < end && start < (*chunk + 1) * CHUNK)
        .filter_map(|(_, s)| s.clone())
        .collect();
    for root in over {
        let Some(s) = &root.sparse else { continue };
        let page = PAGE as u64;
        let first = (start.max(root.start) - root.start) / page;
        let last = (end.min(root.start + root.len) - root.start).div_ceil(page);
        for tile in first..last {
            s.kept
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&(tile as u32));
            let look = s
                .looks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(tile as u32))
                .cloned();
            if let Some(look) = look {
                // Being copied now: it is looked at again soon anyway.
                if let Ok(mut look) = look.try_lock() {
                    look.next = 0;
                }
            }
        }
    }
}

/// The title mapped or unmapped memory in `[start, end)`.
pub(crate) fn remapped(start: u64, end: u64) {
    use windows::core::Interface;
    filled(start, end);
    crate::memory::remapped(start, end);
    let mut map = windows_of_memory();
    if map.is_empty() {
        return;
    }
    let over: Vec<u64> = map
        .range(..end)
        .filter(|(_, s)| s.start + s.len > start)
        .map(|(k, _)| *k)
        .collect();
    for key in over {
        let Some(s) = map.remove(&key) else { continue };
        let mut gone = GONE.lock().unwrap_or_else(|e| e.into_inner());
        gone.insert(s.buffer.as_raw() as usize);
        if let Some(d) = &s.direct {
            d.dead.store(true, Ordering::Release);
            for (_, a) in d.aliases.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                gone.insert(a.buffer.as_raw() as usize);
                if let Some(d) = &a.direct {
                    d.dead.store(true, Ordering::Release);
                }
            }
        }
        // shortcut: kept for good, pages too (`GONE` names its buffers by
        // address); drop them with their last view if the title remaps much.
        std::mem::forget(s);
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_power_of_two() {
            tracing::info!(count, "title memory in place was mapped anew");
        }
    }
}

/// The memory behind a buffer view with elements of `element` bytes, of
/// `len` bytes at `address`.
pub(crate) fn for_view(address: u64, len: u64, element: u64) -> Option<Arc<Shadow>> {
    let shadow = find(address, len)?;
    if shadow.sparse.is_some() {
        return shadow.sparse_on_element(address, element);
    }
    if !shadow.is_direct() {
        return Some(shadow);
    }
    shadow.on_element(address, element).or_else(|| {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::warn!(address = format_args!("{address:#x}"), element, "a view off its elements: copied");
        });
        let copy = find_copy(address, len)?;
        if copy.sparse.is_some() {
            return copy.sparse_on_element(address, element);
        }
        Some(copy)
    })
}

/// Copies of title memory the host GPU cannot use in place: a buffer as long
/// as the address space chunk it stands for, with memory behind the 64 KiB
/// tiles that were brought up to date. Reads of the others give zeros, where
/// a copy ending too soon was a GPU page fault; and memory many buffers of
/// the title overlap in is copied once.
/// The first 32 words of `size` bytes of title memory, in hexadecimal.
#[cfg(feature = "diagnostics")]
pub(crate) fn words_text(address: u64, size: usize) -> String {
    let mut words = [0u32; 32];
    let n = (size / 4).min(32);
    // SAFETY: plain words.
    let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), n * 4) };
    let read = crate::host::read_into(address as *const u8, bytes);
    format!("{read} {:x?}", &words[..n])
}

/// The first dwords at `address`: as the title has them, as the copy has
/// them now, and what that is.
pub(crate) fn args_text(address: u64) -> String {
    let mut title = [0u32; 4];
    // SAFETY: plain words.
    let bytes = unsafe { std::slice::from_raw_parts_mut(title.as_mut_ptr().cast::<u8>(), 16) };
    let read = crate::host::read_into(address as *const u8, bytes);
    let Some(shadow) = find(address, 16) else {
        return format!("{title:x?} {read} no buffer");
    };
    if shadow.is_direct() {
        return format!("{title:x?} in place");
    }
    let mut copy = [0u32; 4];
    if let Some(s) = &shadow.sparse {
        if let Some(data) = shadow.tile(s, address as usize) {
            // SAFETY: the tile's mapping.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (data as *const u8).add(address as usize % PAGE),
                    copy.as_mut_ptr().cast(),
                    16,
                );
            }
        }
        return format!("{title:x?} {read} sparse {copy:x?}");
    }
    format!("{title:x?} {read} copy")
}

/// What was seen of a tile's title memory.
struct Look {
    /// The frame to look at it next in, and how often in a row it had not
    /// changed.
    next: u64,
    still: u32,
    /// The title has no memory there.
    hole: bool,
    /// A hash of each [`BLOCK`] as last copied (0: not known).
    blocks: [u64; PAGE / BLOCK],
}

impl Default for Look {
    fn default() -> Self {
        Look {
            next: 0,
            still: 0,
            hole: false,
            blocks: [0; PAGE / BLOCK],
        }
    }
}

/// Bytes of title memory copied as one.
const BLOCK: usize = 1 << 10;

/// A quick hash, never 0.
fn mixed(bytes: &[u8]) -> u64 {
    let mut h = [0x9e37_79b9_7f4a_7c15_u64, 0xc2b2_ae3d_27d4_eb4f, 0x1656_67b1_9e37_79f9, 0x27d4_eb2f_1656_67c5];
    for c in bytes.chunks_exact(32) {
        for (k, h) in h.iter_mut().enumerate() {
            let x = u64::from_le_bytes(c[8 * k..8 * k + 8].try_into().unwrap_or_default());
            *h = (*h ^ x).wrapping_mul(0x9fb2_1c65_1e98_df25).rotate_left(29);
        }
    }
    let x = h[0] ^ h[1].rotate_left(17) ^ h[2].rotate_left(31) ^ h[3].rotate_left(47);
    (x ^ x >> 32).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1
}

/// A quick hash for tile numbers.
#[derive(Clone, Copy, Default)]
struct Mix(u64);
impl std::hash::BuildHasher for Mix {
    type Hasher = Mix;
    fn build_hasher(&self) -> Mix {
        Mix(0)
    }
}
impl std::hash::Hasher for Mix {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u32(u32::from(b));
        }
    }
    fn write_u32(&mut self, x: u32) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(x)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

struct Sparse {
    /// By tile: where the CPU writes it, and where it is in which heap.
    tiles: Mutex<std::collections::HashMap<u32, Tile, Mix>>,
    /// What tiles shaders write held before the submission.
    before: Mutex<std::collections::HashMap<u32, Vec<u8>, Mix>>,
    /// By tile: the frame to look at it next in, and how often in a row it
    /// had not changed.
    looks: Mutex<std::collections::HashMap<u32, Arc<Mutex<Look>>, Mix>>,
    /// Tiles shaders reach by address: kept whatever asks about them.
    kept: Mutex<std::collections::HashSet<u32, Mix>>,
    /// The same from other 64 KiB starts ([`Direct::aliases`]).
    aliases: Mutex<Vec<Arc<Shadow>>>,
    /// This is an alias of that one.
    of: Option<Arc<Shadow>>,
}

#[derive(Clone, Copy)]
struct Tile {
    data: usize,
    heap: usize,
    offset: u32,
    /// The frame it was last brought up to date or written in.
    used: u64,
}

/// Address space a sparse copy stands for.
const CHUNK: u64 = 2 << 30;

/// Memory for tiles, in heaps the CPU maps.
struct TilePool {
    heaps: Vec<(windows::Win32::Graphics::Direct3D12::ID3D12Heap, ID3D12Resource, usize)>,
    /// Tiles used of the last one.
    used: u32,
    /// Tiles given back.
    free: Vec<Tile>,
}
// SAFETY: free-threaded objects and a persistent mapping.
unsafe impl Send for TilePool {}

static TILES: Mutex<TilePool> = Mutex::new(TilePool {
    heaps: Vec::new(),
    used: 0,
    free: Vec::new(),
});

const POOL_TILES: u32 = 512;

fn new_tile() -> Option<Tile> {
    use windows::Win32::Graphics::Direct3D12::*;
    let mut pool = TILES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tile) = pool.free.pop() {
        crate::memstat::add(&crate::memstat::SHADOWS, PAGE as u64);
        return Some(tile);
    }
    if pool.heaps.is_empty() || pool.used == POOL_TILES {
        let size = u64::from(POOL_TILES) * PAGE as u64;
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: size,
            Properties: D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_CUSTOM,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
                MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
                ..Default::default()
            },
            Alignment: 0,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS,
        };
        let mut heap: Option<ID3D12Heap> = None;
        let mut whole: Option<ID3D12Resource> = None;
        let mut data = std::ptr::null_mut();
        // SAFETY: plain creation; the mapping lives as long as the heap.
        let made = unsafe {
            let device = &host()?.device;
            device.CreateHeap(&desc, &mut heap).and_then(|()| {
                device.CreatePlacedResource(
                    heap.as_ref(),
                    0,
                    &buffer_desc(size, D3D12_RESOURCE_FLAG_NONE),
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut whole,
                )
            })
            .and_then(|()| match &whole {
                Some(w) => w.Map(0, None, Some(&mut data)),
                None => Err(windows::core::Error::empty()),
            })
        };
        if let Err(error) = made {
            tracing::warn!(%error, "no memory for copies of title memory");
            return None;
        }
        let heap = heap?;
        crate::memstat::track(&heap, 5, size);
        pool.heaps.push((heap, whole?, data as usize));
        pool.used = 0;
    }
    let heap = pool.heaps.len() - 1;
    let offset = pool.used;
    pool.used += 1;
    crate::memstat::add(&crate::memstat::SHADOWS, PAGE as u64);
    Some(Tile {
        data: pool.heaps[heap].2 + offset as usize * PAGE,
        heap,
        offset,
        used: 0,
    })
}

/// Which tiles are given back (`VICTORY_SWEEP_TILES`): 0 none, 1 those
/// nothing has asked about for 120 frames, 2 those nothing has asked about
/// for 600 frames AND whose title memory is no longer there.
// shortcut: 0 by default, so the copies' memory only grows (3.8 GB of tiles
// after a quarter of an hour's drive, 2026-11-25). With 1 the host GPU
// stopped about when the first tiles of the world were given back: what
// still uses a tile does not always say so. 2 gives back only what the
// title itself has unmapped, which nothing of its own may read any more.
fn sweeps() -> u8 {
    static ON: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        if cfg!(test) {
            return 1;
        }
        std::env::var("VICTORY_SWEEP_TILES").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
    })
}

/// Gives back the tiles nothing has asked about for some frames: what
/// viewed them is gone. At the present of `frame`.
pub(crate) fn sweep(frame: u64) {
    const EVERY: u64 = 32;
    let mode = sweeps();
    if !frame.is_multiple_of(EVERY) || mode == 0 {
        return;
    }
    // Once a frame, whoever asks.
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if LAST.swap(frame, Ordering::Relaxed) == frame {
        return;
    }
    let gone_only = mode >= 2;
    let keep: u64 = if gone_only { 600 } else { 120 };
    let mut given = 0u64;
    let started = std::time::Instant::now();
    let all: Vec<Arc<Shadow>> = SPARSE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .flatten()
        .cloned()
        .collect();
    for shadow in all {
        let Some(s) = &shadow.sparse else { continue };
        if !shadow.pending.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            continue;
        }
        // Those old enough, taken under the lock; whether their title memory
        // is gone is asked of the system outside it, a region at a time (one
        // question answers for every tile of an unmapped region); then those
        // still as they were are taken out.
        let mut candidates: Vec<(u32, u64)> = {
            let tiles = s.tiles.lock().unwrap_or_else(|e| e.into_inner());
            let kept = s.kept.lock().unwrap_or_else(|e| e.into_inner());
            tiles
                .iter()
                .filter(|(at, t)| t.used + keep < frame && !kept.contains(at))
                .map(|(&at, t)| (at, t.used))
                .collect()
        };
        if gone_only && !candidates.is_empty() {
            candidates.sort_unstable();
            let mut gone_until = 0usize;
            candidates.retain(|&(at, _)| {
                let address = shadow.start as usize + at as usize * PAGE;
                if address + PAGE <= gone_until {
                    return true;
                }
                match unmapped(address) {
                    Some(end) => {
                        gone_until = end;
                        address + PAGE <= end
                    }
                    None => false,
                }
            });
        }
        if candidates.is_empty() {
            continue;
        }
        let old: Vec<(u32, Tile)> = {
            let mut tiles = s.tiles.lock().unwrap_or_else(|e| e.into_inner());
            let kept = s.kept.lock().unwrap_or_else(|e| e.into_inner());
            let mut old = Vec::new();
            for (at, used) in candidates {
                if !kept.contains(&at)
                    && tiles.get(&at).is_some_and(|t| t.used == used)
                    && let Some(t) = tiles.remove(&at)
                {
                    old.push((at, t));
                }
            }
            old
        };
        if old.is_empty() {
            continue;
        }
        given += old.len() as u64;
        {
            let mut looks = s.looks.lock().unwrap_or_else(|e| e.into_inner());
            for (at, _) in &old {
                looks.remove(at);
            }
        }
        {
            let base = shadow.start as usize / PAGE;
            let mut copied = shadow.copied.lock().unwrap_or_else(|e| e.into_inner());
            for (at, _) in &old {
                let page = (shadow.start as usize / PAGE + *at as usize) - base;
                if let Some(c) = copied.get_mut(page) {
                    *c = 0;
                }
            }
        }
        shadow
            .unwatched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        unmap_tiles(&shadow.buffer, old.iter().map(|o| o.0));
        for alias in s.aliases.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            let shift = ((alias.start - shadow.start) / PAGE as u64) as u32;
            unmap_tiles(
                &alias.buffer,
                old.iter().filter(|o| o.0 >= shift).map(|o| o.0 - shift),
            );
        }
        crate::memstat::sub(&crate::memstat::SHADOWS, (old.len() * PAGE) as u64);
        TILES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .free
            .extend(old.into_iter().map(|o| o.1));
    }
    let took = started.elapsed();
    if given > 0 || took > std::time::Duration::from_millis(20) {
        tracing::info!(
            tiles = given,
            megabytes = given * PAGE as u64 >> 20,
            frame,
            ms = took.as_millis() as u64,
            "tiles given back"
        );
    }
}

/// Reads of these tiles give zeros again.
fn unmap_tiles(buffer: &ID3D12Resource, tiles: impl Iterator<Item = u32>) {
    use windows::Win32::Graphics::Direct3D12::*;
    let Some(host) = host() else { return };
    for at in tiles {
        let coordinate = D3D12_TILED_RESOURCE_COORDINATE {
            X: at,
            ..Default::default()
        };
        let region = D3D12_TILE_REGION_SIZE {
            NumTiles: 1,
            ..Default::default()
        };
        // SAFETY: a live sparse buffer, a tile inside it.
        unsafe {
            host.queue().UpdateTileMappings(
                buffer,
                1,
                Some(&coordinate),
                Some(&region),
                None,
                1,
                Some(&D3D12_TILE_RANGE_FLAG_NULL),
                None,
                Some(&1),
                D3D12_TILE_MAPPING_FLAG_NONE,
            );
        }
    }
}

fn buffer_desc(
    len: u64,
    flags: windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_FLAGS,
) -> windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DESC {
    use windows::Win32::Graphics::Direct3D12::*;
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: len,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    }
}

/// Sparse copies by chunk.
static SPARSE: Mutex<BTreeMap<u64, Option<Arc<Shadow>>>> = Mutex::new(BTreeMap::new());

fn sparse_buffer(start: u64, len: u64, of: Option<Arc<Shadow>>) -> Option<Shadow> {
    use windows::Win32::Graphics::Direct3D12::*;
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: plain creation of a buffer without memory.
    let made = unsafe {
        host()?.device.CreateReservedResource(
            &buffer_desc(len, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS),
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut buffer,
        )
    };
    if let Err(error) = made {
        tracing::warn!(%error, len, "no sparse buffer");
        return None;
    }
    let buffer = buffer?;
    crate::host::name(&buffer, || format!("copies of title memory {start:#x}+{len:#x}"));
    // SAFETY: a live buffer.
    let gpu = unsafe { buffer.GetGPUVirtualAddress() };
    tracing::info!(start = format_args!("{start:#x}"), len, alias = of.is_some(), "sparse copies of title memory");
    Some(Shadow {
        buffer,
        data: std::ptr::null_mut(),
        gpu,
        start,
        len,
        copied: Mutex::new(if of.is_some() { Vec::new() } else { vec![0; (len / PAGE as u64) as usize] }),
        watched: AtomicU8::new(0),
        pending: Mutex::default(),
        before: Mutex::default(),
        direct: None,
        unwatched: Mutex::default(),
        clean_spans: Mutex::default(),
        sparse: Some(Sparse {
            tiles: Mutex::default(),
            before: Mutex::default(),
            looks: Mutex::default(),
            kept: Mutex::default(),
            aliases: Mutex::default(),
            of,
        }),
        card: None,
    })
}

/// The sparse copy `[address, end)` lies in.
fn sparse(address: u64, end: u64) -> Option<Arc<Shadow>> {
    if cfg!(test) || (end - 1) / CHUNK != address / CHUNK {
        return None;
    }
    SPARSE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(address / CHUNK)
        .or_insert_with(|| sparse_buffer(address / CHUNK * CHUNK, CHUNK, None).map(Arc::new))
        .clone()
}

/// The sparse copy of the 2 GiB `address` is in.
pub(crate) fn chunk(address: u64) -> Option<Arc<Shadow>> {
    sparse(address, address + 1)
}

/// Shaders run by `list` reach the memory at `address` (and `len` bytes
/// after it, more or less) by its address.
pub(crate) fn declare(list: &HostList, address: u64, len: u64) {
    if address >> 40 != 0 || address < 1 << 20 {
        return;
    }
    let Some(s) = find_reaching(address, 1) else { return };
    if s.is_direct() {
        crate::memory::reach(address, Some((&s.buffer, s.start, s.len)));
        return;
    }
    crate::memory::reach(address, None);
    let Some(root) = chunk(address) else { return };
    let Some(sparse) = &root.sparse else { return };
    let end = (address + len).min(root.start + root.len);
    {
        let page = PAGE as u64;
        let mut kept = sparse.kept.lock().unwrap_or_else(|e| e.into_inner());
        for tile in (address - root.start) / page..(end - root.start).div_ceil(page) {
            kept.insert(tile as u32);
        }
    }
    root.refresh(list, address, end - address);
}

/// Maps tiles of a sparse buffer (`first`, then each the next) to heaps.
fn map_tiles(buffer: &ID3D12Resource, tiles: &[(u32, Tile)]) {
    use windows::Win32::Graphics::Direct3D12::*;
    let Some(host) = host() else { return };
    let pool = TILES.lock().unwrap_or_else(|e| e.into_inner());
    for &(at, tile) in tiles {
        let coordinate = D3D12_TILED_RESOURCE_COORDINATE {
            X: at,
            ..Default::default()
        };
        let region = D3D12_TILE_REGION_SIZE {
            NumTiles: 1,
            ..Default::default()
        };
        // SAFETY: a live sparse buffer and heap, tiles inside both.
        unsafe {
            host.queue().UpdateTileMappings(
                buffer,
                1,
                Some(&coordinate),
                Some(&region),
                &pool.heaps[tile.heap].0,
                1,
                Some(&D3D12_TILE_RANGE_FLAG_NONE),
                Some(&tile.offset),
                Some(&1),
                D3D12_TILE_MAPPING_FLAG_NONE,
            );
        }
    }
}

impl Shadow {
    /// The copy whose tiles these are.
    fn sparse_root(&self) -> Option<(&Shadow, &Sparse)> {
        let s = self.sparse.as_ref()?;
        match &s.of {
            Some(root) => Some((root, root.sparse.as_ref()?)),
            None => Some((self, s)),
        }
    }

    /// The CPU's view of the tile `address` is in, with memory behind it.
    fn tile(&self, s: &Sparse, address: usize) -> Option<usize> {
        let at = ((address - self.start as usize) / PAGE) as u32;
        let mut tiles = s.tiles.lock().unwrap_or_else(|e| e.into_inner());
        let frame = crate::queue::presented();
        if let Some(t) = tiles.get_mut(&at) {
            t.used = frame;
            return Some(t.data);
        }
        let mut tile = new_tile()?;
        tile.used = frame;
        tiles.insert(at, tile);
        map_tiles(&self.buffer, &[(at, tile)]);
        for alias in s.aliases.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            let shift = ((alias.start - self.start) / PAGE as u64) as u32;
            if at >= shift {
                map_tiles(&alias.buffer, &[(at - shift, tile)]);
            }
        }
        Some(tile.data)
    }

    /// Runs `f` on the CPU's view of each tile's part of `[from, to)`: its
    /// title address, the view, its length.
    fn each_tile(&self, s: &Sparse, from: usize, to: usize, mut f: impl FnMut(usize, *mut u8, usize)) {
        let mut at = from;
        while at < to {
            let n = (PAGE - at % PAGE).min(to - at);
            if let Some(data) = self.tile(s, at) {
                f(at, (data + at % PAGE) as *mut u8, n);
            }
            at += n;
        }
    }

    /// A sparse copy from a start `address` is a multiple of `element` bytes
    /// after.
    fn sparse_on_element(self: &Arc<Self>, address: u64, element: u64) -> Option<Arc<Shadow>> {
        if element == 0 || (address - self.start) % element == 0 {
            return Some(self.clone());
        }
        let s = self.sparse.as_ref()?;
        let page = PAGE as u64;
        let mut base = self.start + page;
        while base <= address && base - self.start <= 4096 * page {
            if (address - base) % element == 0 {
                let mut aliases = s.aliases.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(a) = aliases.iter().find(|a| a.start == base) {
                    return Some(a.clone());
                }
                let len = self.start + self.len - base;
                let made = Arc::new(sparse_buffer(base, len, Some(self.clone()))?);
                aliases.push(made.clone());
                drop(aliases);
                let shift = ((base - self.start) / page) as u32;
                let known: Vec<(u32, Tile)> = s
                    .tiles
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .filter(|(at, _)| **at >= shift)
                    .map(|(at, t)| (at - shift, *t))
                    .collect();
                map_tiles(&made.buffer, &known);
                return Some(made);
            }
            base += page;
        }
        None
    }

    /// The CPU's views of each tile's part of `[from, to)`: its title
    /// address, the view, its length. Tiles already mapped are found under
    /// one lock (the world declares ~160 GB of ranges a frame: one lock per
    /// 64 KiB tile was a tenth of the submitting thread's time).
    fn tiles_of(&self, s: &Sparse, from: usize, to: usize) -> Vec<(usize, usize, usize)> {
        let start = self.start as usize;
        let mut views = Vec::with_capacity((to - from).div_ceil(PAGE) + 1);
        let mut missing = Vec::new();
        {
            let mut tiles = s.tiles.lock().unwrap_or_else(|e| e.into_inner());
            let frame = crate::queue::presented();
            let mut at = from;
            while at < to {
                let n = (PAGE - at % PAGE).min(to - at);
                match tiles.get_mut(&(((at - start) / PAGE) as u32)) {
                    Some(t) => {
                        t.used = frame;
                        views.push((at, t.data + at % PAGE, n));
                    }
                    None => missing.push((at, n)),
                }
                at += n;
            }
        }
        for (at, n) in missing {
            if let Some(data) = self.tile(s, at) {
                views.push((at, data + at % PAGE, n));
            }
        }
        views
    }

    /// [`Shadow::writing`] of a sparse copy. A range inside one already
    /// declared for this submission has its tiles' snapshots taken; new
    /// snapshots are copied on several threads (hundreds of MB a frame).
    fn sparse_writing(&self, s: &Sparse, address: usize, len: usize) {
        let end = address + len;
        let covered = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let covered = pending.iter().any(|&(a, b)| a <= address && end <= b);
            pending.push((address, end));
            covered
        };
        #[cfg(feature = "diagnostics")]
        SPARSE_WRITES[0].fetch_add(len as u64, Ordering::Relaxed);
        if covered {
            return;
        }
        let views = self.tiles_of(s, address, end);
        let mut before = s.before.lock().unwrap_or_else(|e| e.into_inner());
        let start = self.start as usize;
        // (tile, the start of its view) of tiles without a snapshot yet.
        let new: Vec<(u32, usize)> = views
            .iter()
            .map(|&(at, data, _)| (((at - start) / PAGE) as u32, data & !(PAGE - 1)))
            .filter(|(tile, _)| !before.contains_key(tile))
            .collect();
        if new.is_empty() {
            return;
        }
        #[cfg(feature = "diagnostics")]
        SPARSE_WRITES[1].fetch_add((new.len() * PAGE) as u64, Ordering::Relaxed);
        let mut bufs = pooled_tiles(new.len());
        let copies: Vec<(usize, usize)> =
            new.iter().zip(&mut bufs).map(|(&(_, view), buf)| (view, buf.as_mut_ptr() as usize)).collect();
        crate::host::in_parallel(&copies, 16, |chunk| {
            for &(view, buf) in chunk {
                // SAFETY: the tile's mapping; a buffer with room for a tile,
                // each written by one thread.
                unsafe { std::ptr::copy_nonoverlapping(view as *const u8, buf as *mut u8, PAGE) };
            }
        });
        for (&(tile, _), mut buf) in new.iter().zip(bufs) {
            // SAFETY: filled above, within its capacity.
            unsafe { buf.set_len(PAGE) };
            before.insert(tile, buf);
        }
    }

    /// [`Shadow::copy_back`] of a sparse copy, its tiles on several threads.
    fn sparse_back(&self, s: &Sparse, address: usize, len: usize) -> usize {
        let views = self.tiles_of(s, address, address + len);
        let mut before = s.before.lock().unwrap_or_else(|e| e.into_inner());
        let start = self.start as usize;
        // (title address, view, length, the snapshot's bytes): one tile each,
        // so no two touch the same memory.
        let work: Vec<(usize, usize, usize, usize)> = views
            .iter()
            .filter_map(|&(at, data, n)| {
                let old = before.get_mut(&(((at - start) / PAGE) as u32))?;
                Some((at, data, n, old.as_mut_ptr() as usize + at % PAGE))
            })
            .collect();
        let written = std::sync::atomic::AtomicUsize::new(0);
        crate::host::in_parallel(&work, 8, |chunk| {
            let mut title: Option<Vec<u8>> = None;
            for &(at, data, n, old) in chunk {
                // SAFETY: the tile's mapping and its snapshot (kept under the
                // lock held above); this tile is this thread's alone.
                let host = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, n) };
                let old = unsafe { std::slice::from_raw_parts_mut(old as *mut u8, n) };
                #[cfg(feature = "diagnostics")]
                SPARSE_WRITES[2].fetch_add(n as u64, Ordering::Relaxed);
                if host == old {
                    continue;
                }
                #[cfg(feature = "diagnostics")]
                SPARSE_WRITES[3].fetch_add(n as u64, Ordering::Relaxed);
                let buf = title.get_or_insert_with(|| {
                    let mut v = pooled(PAGE);
                    v.resize(PAGE, 0);
                    v
                });
                if !crate::host::read_into(at as *const u8, &mut buf[..n]) {
                    continue;
                }
                if let Some((dirty_start, dirty_end)) = merge_gpu_writes_mut(&mut buf[..n], host, old) {
                    let dirty_len = dirty_end - dirty_start;
                    crate::watch::writing(at + dirty_start, dirty_len);
                    crate::host::write_mapped(
                        (at + dirty_start) as *mut u8,
                        buf[dirty_start..].as_ptr(),
                        dirty_len,
                    );
                    written.fetch_add(dirty_len, Ordering::Relaxed);
                }
            }
            if let Some(buf) = title {
                give_back(buf);
            }
        });
        let written = written.into_inner();
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|&r| r != (address, address + len));
        if pending.is_empty() {
            give_back_tiles(before.drain().map(|(_, buf)| buf));
        }
        written
    }
}

/// Merges bytes the GPU changed (`host != old`) into `title`, syncs `host` to
/// `title`, and returns the tight `[first, last)` byte span where `title` was
/// modified (or `None` if nothing changed). Scans 8 bytes at a time so
/// unchanged 64-bit words skip per-byte branching.
fn merge_gpu_writes(title: &mut [u8], host: &mut [u8], old: &[u8]) -> Option<(usize, usize)> {
    let n = title.len().min(host.len()).min(old.len());
    let mut first = usize::MAX;
    let mut last = 0usize;
    let mut i = 0;
    while i + 8 <= n {
        let hw = u64::from_ne_bytes(host[i..i + 8].try_into().unwrap());
        let ow = u64::from_ne_bytes(old[i..i + 8].try_into().unwrap());
        if hw == ow {
            host[i..i + 8].copy_from_slice(&title[i..i + 8]);
        } else {
            for k in i..i + 8 {
                let h = host[k];
                if h != old[k] && title[k] != h {
                    title[k] = h;
                    first = first.min(k);
                    last = k + 1;
                }
                host[k] = title[k];
            }
        }
        i += 8;
    }
    for k in i..n {
        let h = host[k];
        if h != old[k] && title[k] != h {
            title[k] = h;
            first = first.min(k);
            last = k + 1;
        }
        host[k] = title[k];
    }
    (first < last).then_some((first, last))
}

/// Same as [`merge_gpu_writes`], also updating `old` to match `title`.
fn merge_gpu_writes_mut(
    title: &mut [u8],
    host: &mut [u8],
    old: &mut [u8],
) -> Option<(usize, usize)> {
    let n = title.len().min(host.len()).min(old.len());
    let mut first = usize::MAX;
    let mut last = 0usize;
    let mut i = 0;
    while i + 8 <= n {
        let hw = u64::from_ne_bytes(host[i..i + 8].try_into().unwrap());
        let ow = u64::from_ne_bytes(old[i..i + 8].try_into().unwrap());
        if hw == ow {
            let tw = &title[i..i + 8];
            host[i..i + 8].copy_from_slice(tw);
            old[i..i + 8].copy_from_slice(tw);
        } else {
            for k in i..i + 8 {
                let h = host[k];
                if h != old[k] && title[k] != h {
                    title[k] = h;
                    first = first.min(k);
                    last = k + 1;
                }
                let t = title[k];
                host[k] = t;
                old[k] = t;
            }
        }
        i += 8;
    }
    for k in i..n {
        let h = host[k];
        if h != old[k] && title[k] != h {
            title[k] = h;
            first = first.min(k);
            last = k + 1;
        }
        let t = title[k];
        host[k] = t;
        old[k] = t;
    }
    (first < last).then_some((first, last))
}

/// Merges GPU-modified bytes (`gpu_new != staging_old`) into `title` and syncs
/// `staging_old` to the merged bytes. Returns `(dirty_span_for_title, cpu_modified)`
/// where `cpu_modified` is true if any byte unchanged by the GPU differed in `title`
/// (a concurrent title CPU write that must be uploaded back to VRAM).
fn merge_gpu_writes_card(
    title: &mut [u8],
    staging_old: &mut [u8],
    gpu_new: &[u8],
) -> (Option<(usize, usize)>, bool) {
    let n = title.len().min(staging_old.len()).min(gpu_new.len());
    let mut first = usize::MAX;
    let mut last = 0usize;
    let mut cpu_modified = false;
    let mut i = 0;
    while i + 8 <= n {
        let gw = u64::from_ne_bytes(gpu_new[i..i + 8].try_into().unwrap());
        let ow = u64::from_ne_bytes(staging_old[i..i + 8].try_into().unwrap());
        if gw == ow {
            let tw = u64::from_ne_bytes(title[i..i + 8].try_into().unwrap());
            if tw != ow {
                staging_old[i..i + 8].copy_from_slice(&title[i..i + 8]);
                cpu_modified = true;
            }
        } else {
            for k in i..i + 8 {
                let g = gpu_new[k];
                let o = staging_old[k];
                if g != o {
                    if title[k] != g {
                        title[k] = g;
                        first = first.min(k);
                        last = k + 1;
                    }
                    staging_old[k] = g;
                } else if title[k] != o {
                    staging_old[k] = title[k];
                    cpu_modified = true;
                }
            }
        }
        i += 8;
    }
    for k in i..n {
        let g = gpu_new[k];
        let o = staging_old[k];
        if g != o {
            if title[k] != g {
                title[k] = g;
                first = first.min(k);
                last = k + 1;
            }
            staging_old[k] = g;
        } else if title[k] != o {
            staging_old[k] = title[k];
            cpu_modified = true;
        }
    }
    ((first < last).then_some((first, last)), cpu_modified)
}

struct Entry {
    end: u64,
    shadow: Option<Arc<Shadow>>,
}

/// Title buffers by (start address, resource), with their copy once made.
static BUFFERS: Mutex<BTreeMap<(u64, usize), Entry>> = Mutex::new(BTreeMap::new());

fn buffers() -> std::sync::MutexGuard<'static, BTreeMap<(u64, usize), Entry>> {
    BUFFERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A title buffer exists at `start` (`len` bytes).
pub(crate) fn register(resource: usize, start: u64, len: u64) {
    buffers().insert(
        (start, resource),
        Entry {
            end: start + len,
            shadow: None,
        },
    );
}

/// The title released it; lists that still hold its copy keep it alive.
pub(crate) fn unregister(resource: usize, start: u64) {
    let gone = buffers().remove(&(start, resource));
    if let Some(shadow) = gone.and_then(|e| e.shadow) {
        crate::host::retire(shadow);
    }
}

/// Whether a title buffer holds `[address, address + len)` (its memory is
/// the title's, without asking the kernel).
pub(crate) fn in_buffer(address: u64, len: u64) -> bool {
    let Some(end) = address.checked_add(len.max(1)) else {
        return false;
    };
    buffers()
        .range(..=(address, usize::MAX))
        .rev()
        .take(64)
        .any(|(_, e)| e.end >= end)
}

/// The copy of a title buffer holding `[address, address + len)`: of one
/// whose copy is made already if there is one (a buffer inside another,
/// say in the world's 174 MB heap, then shares its copy instead of paying
/// for its bytes twice, and the GPU's writes land in one copy), else of
/// the innermost, made now.
pub(crate) fn find(address: u64, len: u64) -> Option<Arc<Shadow>> {
    let end = address.checked_add(len.max(1))?;
    if let Some(window) = direct(address, end) {
        return Some(window);
    }
    find_copy(address, len)
}

fn find_copy(address: u64, len: u64) -> Option<Arc<Shadow>> {
    let end = address.checked_add(len.max(1))?;
    if let Some(copy) = sparse(address, end) {
        return Some(copy);
    }
    let mut map = buffers();
    let holding: Vec<((u64, usize), bool)> = map
        .range(..=(address, usize::MAX))
        .rev()
        .take(64)
        .filter(|(_, e)| e.end >= end)
        .map(|(k, e)| (*k, e.shadow.is_some()))
        .collect();
    let key = holding
        .iter()
        .find(|(_, made)| *made)
        .or(holding.first())
        .map(|(k, _)| *k);
    let Some(key) = key else {
        drop(map);
        return loose(address, end);
    };
    let entry = map.get_mut(&key)?;
    if entry.shadow.is_none() {
        let len = entry.end - key.0;
        let size = len.next_multiple_of(1 << 16);
        // Rounded up so that views ending in the buffer's last 256 bytes fit.
        // Shaders write some buffers (unordered access), so not upload memory.
        let h = host()?;
        let (staging, data) = h.shared_buffer(size)?;
        crate::host::name(&staging, || format!("title buffer {:#x}+{len:#x}", key.0));
        let (buffer, card) = match crate::card::CardState::try_new(h, &staging, size) {
            Some(card) => {
                let vram = card.vram.clone();
                crate::host::name(&vram, || format!("title buffer {:#x}+{len:#x} VRAM", key.0));
                tracing::info!(
                    start = format_args!("{:#x}", key.0),
                    len,
                    "large buffer placed in VRAM (card buffer)"
                );
                (vram, Some(card))
            }
            None => {
                if len >= 16 << 20 {
                    tracing::info!(
                        start = format_args!("{:#x}", key.0),
                        len,
                        "large buffer copied to the host"
                    );
                } else {
                    tracing::debug!(
                        start = format_args!("{:#x}", key.0),
                        len,
                        "buffer copied to the host"
                    );
                }
                (staging, None)
            }
        };
        // SAFETY: a live buffer.
        let gpu = unsafe { buffer.GetGPUVirtualAddress() };
        entry.shadow = Some(Arc::new(Shadow::new(buffer, data, gpu, key.0, len, card)));
    }
    entry.shadow.clone()
}

/// The copy holding `address` for a view without a size (a root view, which
/// the host does not bound): in memory outside buffers, one with at least
/// `reach` bytes after `address`. A window ending sooner than the data read
/// through the view is a GPU page fault.
pub(crate) fn find_reaching(address: u64, reach: u64) -> Option<Arc<Shadow>> {
    // To the end of the innermost buffer holding it: a window ending sooner
    // faults.
    let end = buffers()
        .range(..=(address, usize::MAX))
        .rev()
        .take(64)
        .map(|(_, e)| e.end)
        .find(|&e| e > address);
    if let Some(end) = end {
        return direct(address, end).or_else(|| find_copy(address, 1));
    }
    if let Some(window) = direct(address, address.checked_add(reach)?) {
        return Some(window);
    }
    loose(address, address.checked_add(reach)?)
}

/// Titles also use memory outside any buffer object as GPU memory (Grand
/// Theft Auto VI's interface vertices, from its own allocator). Such memory
/// gets copies of the aligned windows it touches.
/// shortcut: copies of windows are never freed; key them to the title's
/// allocations if memory use grows.
const WINDOW: u64 = 4 << 20;

/// Copies of title memory outside buffers, by (start, length).
static LOOSE: Mutex<BTreeMap<(u64, u64), Arc<Shadow>>> = Mutex::new(BTreeMap::new());

fn loose(address: u64, end: u64) -> Option<Arc<Shadow>> {
    if let Some(copy) = sparse(address, end) {
        return Some(copy);
    }
    let start = address & !(WINDOW - 1);
    let len = end.next_multiple_of(WINDOW) - start;
    let mut map = LOOSE.lock().unwrap_or_else(|e| e.into_inner());
    // Any window copied already that holds the range: a range crossing a
    // window's end used to get a second copy of the same bytes.
    if let Some(shadow) = map
        .range(..=(start, u64::MAX))
        .rev()
        .take(64)
        .find(|((s, l), _)| *s <= address && s + l >= end)
        .map(|(_, shadow)| shadow.clone())
    {
        return Some(shadow);
    }
    let h = host()?;
    let (staging, data) = h.shared_buffer(len)?;
    crate::host::name(&staging, || format!("title memory {start:#x}+{len:#x}"));
    let (buffer, card) = match crate::card::CardState::try_new(h, &staging, len) {
        Some(card) => {
            let vram = card.vram.clone();
            crate::host::name(&vram, || format!("title memory {start:#x}+{len:#x} VRAM"));
            tracing::info!(
                start = format_args!("{start:#x}"),
                len,
                "loose memory placed in VRAM (card buffer)"
            );
            (vram, Some(card))
        }
        None => (staging, None),
    };
    // SAFETY: a live buffer.
    let gpu = unsafe { buffer.GetGPUVirtualAddress() };
    tracing::debug!(
        start = format_args!("{start:#x}"),
        len,
        "loose memory copied to the host"
    );
    let shadow = Arc::new(Shadow::new(buffer, data, gpu, start, len, card));
    map.insert((start, len), shadow.clone());
    Some(shadow)
}

/// For device-removed reports: the copy of title memory whose host range
/// starts closest below host GPU address `va`, and where `va` is in it.
pub(crate) fn near(va: u64) -> String {
    let mut all: Vec<(u64, u64, u64)> = buffers()
        .values()
        .filter_map(|e| e.shadow.as_deref().map(|s| (s.gpu, s.start, s.len)))
        .collect();
    all.extend(
        LOOSE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|s| (s.gpu, s.start, s.len)),
    );
    for s in SPARSE.lock().unwrap_or_else(|e| e.into_inner()).values().flatten() {
        all.push((s.gpu, s.start, s.len));
        if let Some(sparse) = &s.sparse {
            let aliases = sparse.aliases.lock().unwrap_or_else(|e| e.into_inner());
            all.extend(aliases.iter().map(|a| (a.gpu, a.start, a.len)));
        }
    }
    all.extend(windows_of_memory().values().map(|s| (s.gpu, s.start, s.len)));
    let below = all.iter().filter(|c| c.0 <= va).max();
    let above = all.iter().filter(|c| c.0 > va).min();
    let mut text = match below {
        Some(&(gpu, start, len)) => format!(
            "; the nearest title memory copy below it: {start:#x}+{len:#x} at host {gpu:#x}, offset {:#x}",
            va - gpu
        ),
        None => "; no title memory copy below it".into(),
    };
    if let Some(&(gpu, start, len)) = above {
        text.push_str(&format!(
            "; the nearest above: {start:#x}+{len:#x} at host {gpu:#x} ({} copies in all)",
            all.len()
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    #[test]
    fn partial_match_leaves_unchecked_bytes_visible() {
        let mut unchanged = UnchangedPages::default();
        let before = vec![0; PAGE];
        let mut host = before.clone();
        host[PAGE - 1] = 1;
        unchanged.begin(PAGE, 2 * PAGE);

        assert!(!unchanged.differs(PAGE, &host[..4], &before[..4]));
        assert!(!unchanged.pages[0]);
        assert!(unchanged.differs(PAGE, &host, &before));
        assert!(!unchanged.pages[0]);
    }

    #[test]
    fn snapshot_edges_are_cacheable_and_survive_later_ranges() {
        let mut unchanged = UnchangedPages::default();
        unchanged.begin(PAGE - 3, PAGE + 4);
        assert!(!unchanged.differs(PAGE - 3, &[1; 3], &[1; 3]));
        assert!(!unchanged.differs(PAGE, &[2; 4], &[2; 4]));
        assert_eq!(unchanged.pages, [true, true]);

        unchanged.begin(PAGE - 3, PAGE + 4);
        assert!(!unchanged.differs(PAGE - 2, &[1; 2], &[1; 2]));
        assert!(!unchanged.differs(PAGE + 1, &[2; 2], &[2; 2]));
        assert_eq!(unchanged.pages, [true, true]);
    }

    #[test]
    fn next_submission_checks_the_same_page_again() {
        let mut unchanged = UnchangedPages::default();
        unchanged.begin(0, 4);
        assert!(!unchanged.differs(0, &[0; 4], &[0; 4]));
        unchanged.clear();
        unchanged.begin(0, 4);
        assert!(unchanged.differs(0, &[0, 0, 0, 1], &[0; 4]));
    }

    // Both versions walk exactly the same declared ranges, in their
    // original order. Only the page comparison differs.
    fn scan(
        host: &[u8],
        before: &[u8],
        ranges: &[Range<usize>],
        mut differs: impl FnMut(usize, &[u8], &[u8]) -> bool,
    ) -> usize {
        let mut changed = 0;
        for range in ranges {
            let mut at = range.start;
            while at < range.end {
                let n = (PAGE - at % PAGE).min(range.end - at);
                changed += usize::from(differs(at, &host[at..at + n], &before[at..at + n]));
                at += n;
            }
        }
        changed
    }

    #[test]
    fn overlapping_ranges_match_uncached_comparisons() {
        let before = vec![0; 4 * PAGE];
        let mut host = before.clone();
        for at in [2, PAGE - 1, PAGE + 5, 3 * PAGE + 7] {
            host[at] = 1;
        }
        let ranges = [
            0..4,
            2..2 * PAGE + 3,
            PAGE - 2..4 * PAGE,
            2 * PAGE..3 * PAGE,
        ];
        let uncached = scan(&host, &before, &ranges, |_, host, before| host != before);
        let mut unchanged = UnchangedPages::default();
        unchanged.begin(0, before.len());
        let cached = scan(&host, &before, &ranges, |at, host, before| {
            unchanged.differs(at, host, before)
        });
        assert_eq!(uncached, 6);
        assert_eq!(cached, uncached);
        assert_eq!(unchanged.pages, [false, false, true, false]);
    }

    // cargo test --release -p victory-gpu benchmark_overlapping_copy_back -- --ignored --nocapture
    #[test]
    #[ignore = "synthetic CPU release benchmark"]
    fn benchmark_overlapping_copy_back() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};

        const SIZE: usize = 128 << 20;
        let before = vec![0xA5; SIZE];
        let mut host = before.clone();
        for page in (0..SIZE / PAGE).step_by(64) {
            host[page * PAGE + PAGE / 2] ^= 0xFF;
        }
        let ranges: Vec<_> = (0..24).map(|i| i * PAGE + i..SIZE - i * 13).collect();
        let mut baseline = [Duration::ZERO; 5];
        let mut cached = [Duration::ZERO; 5];
        let mut unchanged = UnchangedPages::default();
        for (baseline, cached) in baseline.iter_mut().zip(&mut cached) {
            let started = Instant::now();
            let expected = scan(
                black_box(&host),
                black_box(&before),
                black_box(&ranges),
                |_, host, before| host != before,
            );
            *baseline = started.elapsed();

            unchanged.clear();
            let started = Instant::now();
            unchanged.begin(0, before.len());
            let actual = scan(
                black_box(&host),
                black_box(&before),
                black_box(&ranges),
                |at, host, before| unchanged.differs(at, host, before),
            );
            *cached = started.elapsed();
            assert_eq!(actual, expected);
        }
        baseline.sort_unstable();
        cached.sort_unstable();
        println!(
            "median of five runs, 24 overlapping ranges across 128 MiB: {:?} -> {:?} ({:.1}x)",
            baseline[2],
            cached[2],
            baseline[2].as_secs_f64() / cached[2].as_secs_f64(),
        );
    }
}
