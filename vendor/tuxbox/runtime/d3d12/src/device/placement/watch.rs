//! Which guest pages the CPU wrote since the last refresh, for placed
//! textures whose memory the title allocated with MEM_WRITE_WATCH (xmem keeps
//! the flag). An unchanged mip is then not read or compared at all. Memory
//! without a watch (physical-page views, plain allocations) falls back to the
//! full snapshot compare. TUXBOX_PLACED_WATCH=0 always compares.
//!
//! Textures may overlap, so the ranges are merged first and each merged
//! range is queried (and reset) once per refresh: one texture's reset cannot
//! hide a write from another. The query goes through xmem's
//! TuxBoxTakeWriteWatch, which shares the list with the title's own
//! write-watch reads; without that export every mip is compared.
use core::ffi::c_void;
use core::ops::Range;

type Take = unsafe extern "system" fn(*const c_void, usize, *mut *mut c_void, *mut usize, *mut u32) -> u32;

fn take() -> Option<Take> {
    static TAKE: std::sync::OnceLock<Option<Take>> = std::sync::OnceLock::new();
    *TAKE.get_or_init(|| unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let module = GetModuleHandleW(windows::core::w!("xmem.dll")).ok()?;
        GetProcAddress(module, windows::core::s!("TuxBoxTakeWriteWatch")).map(|f| core::mem::transmute::<_, Take>(f))
    })
}

/// Ranges closer than this are scanned as one: TUXBOX_WATCH_GAP_MB (default
/// 64, from xcl 712bbbe; 0 scans each range alone).
fn watch_gap() -> u64 {
    static GAP: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *GAP.get_or_init(|| crate::env_toggle!("TUXBOX_WATCH_GAP_MB").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(64) << 20)
}

static MERGED_SCANS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SAVED_SCANS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// (scans of merged ranges, write-watch calls they saved), for the memory line.
/// TUXBOX_WATCH_CLIP=0: a merged span is scanned whole, and one range at a
/// time when the OS refuses it (b16f2f9).
fn clip_on() -> bool {
    crate::env_toggle!("TUXBOX_WATCH_CLIP") != Some("0")
}

/// `ranges` (sorted, disjoint) grouped by the allocation each lies in: one
/// piece per allocation, from its first range's start to its last's end.
fn clip(ranges: &[Range<u64>]) -> Vec<Range<u64>> {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let allocation = |a: u64| {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery only reads the address space map.
        (unsafe { VirtualQuery(Some(a as _), &mut info, size_of_val(&info)) } != 0).then(|| info.AllocationBase as u64)
    };
    let mut out: Vec<(Option<u64>, Range<u64>)> = Vec::new();
    for r in ranges {
        let base = allocation(r.start);
        match out.last_mut() {
            Some((b, piece)) if base.is_some() && *b == base && allocation(r.end - 1) == base => piece.end = r.end,
            _ => out.push((base, r.clone())),
        }
    }
    out.into_iter().map(|(_, r)| r).collect()
}

pub(crate) fn scan_stats() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (MERGED_SCANS.load(Relaxed), SAVED_SCANS.load(Relaxed))
}

pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TUXBOX_PLACED_WATCH").is_ok_and(|v| v.trim() == "0") && take().is_some())
}

#[derive(Default)]
pub(super) struct Watch {
    /// Merged ranges with a working write watch.
    watched: Vec<Range<u64>>,
    /// Written page addresses (sorted) and the watch granularity.
    dirty: Vec<u64>,
    granularity: u64,
    buffer: Vec<*mut c_void>,
}

// SAFETY: the buffer holds plain addresses, never dereferenced.
unsafe impl Send for Watch {}

impl Watch {
    /// Query (and reset) the write watch over every range once.
    pub(super) fn refresh(&mut self, mut ranges: Vec<Range<u64>>) {
        self.watched.clear();
        self.dirty.clear();
        ranges.retain(|r| r.start < r.end);
        ranges.sort_unstable_by_key(|r| r.start);
        let mut merged: Vec<Range<u64>> = Vec::with_capacity(ranges.len());
        for r in ranges {
            match merged.last_mut() {
                // Only overlapping ranges merge; adjacent ones may be separate
                // allocations, and one query cannot span two.
                Some(last) if r.start < last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        let Some(take) = take() else { return };
        // xcl 712bbbe: a write-watch call costs ~3.5 us however little it
        // covers and one over 2 GiB ~1.3 ms, so ranges under WATCH_GAP apart
        // are one scan (the gap's pages count as written; uploads keep to
        // what they track). A scan the OS refuses, because the span crosses
        // allocations, falls back to one per range.
        let gap = watch_gap();
        let mut start = 0;
        while start < merged.len() {
            let mut end = start + 1;
            while end < merged.len() && merged[end].start.saturating_sub(merged[end - 1].end) < gap {
                end += 1;
            }
            let span = merged[start].start..merged[end - 1].end;
            if end - start > 1 && clip_on() {
                // xcl 712bbbe watch_ranges (GPL-3): the span is clipped to
                // the allocations it meets, a piece in each, so no scan
                // crosses one and none is refused for it.
                let pieces = clip(&merged[start..end]);
                let mut all = true;
                for piece in &pieces {
                    all &= self.scan(take, piece.clone());
                }
                if all && pieces.len() < end - start {
                    MERGED_SCANS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    SAVED_SCANS.fetch_add((end - start - pieces.len()) as u64, std::sync::atomic::Ordering::Relaxed);
                }
            } else if end - start == 1 || !self.scan(take, span.clone()) {
                for range in &merged[start..end] {
                    self.scan(take, range.clone());
                }
            } else {
                MERGED_SCANS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                SAVED_SCANS.fetch_add((end - start - 1) as u64, std::sync::atomic::Ordering::Relaxed);
            }
            start = end;
        }
        self.dirty.sort_unstable();
    }

    /// Queries (and resets) one range; false when the OS refused it.
    fn scan(&mut self, take: Take, range: Range<u64>) -> bool {
        let base = range.start & !0xfff;
        let size = (range.end - base) as usize;
        let pages = size.div_ceil(0x1000);
        if self.buffer.len() < pages {
            self.buffer.resize(pages, core::ptr::null_mut());
        }
        let mut count = pages;
        let mut granularity = 0u32;
        // SAFETY: the buffer holds `count` entries; the OS validates the range.
        let failed = unsafe { take(base as *const c_void, size, self.buffer.as_mut_ptr(), &mut count, &mut granularity) };
        if failed != 0 || granularity == 0 {
            return false;
        }
        self.granularity = u64::from(granularity);
        self.dirty.extend(self.buffer[..count.min(pages)].iter().map(|&p| p as u64));
        self.watched.push(range);
        true
    }

    /// False when `range` must be compared: not watched, or a page written.
    pub(super) fn unchanged(&self, range: Range<u64>) -> bool {
        if range.start >= range.end || !self.watched.iter().any(|w| w.start <= range.start && range.end <= w.end) {
            return false;
        }
        let first = range.start & !(self.granularity.max(1) - 1);
        let at = self.dirty.partition_point(|&p| p < first);
        self.dirty.get(at).is_none_or(|&p| p >= range.end)
    }
}
