//! Two readers of one write-watch list: the title (XMemGetWriteWatch, or
//! kernel32 GetWriteWatch/ResetWriteWatch from its imports) and the d3d12
//! runtime (TuxBoxTakeWriteWatch, to skip unwritten placed textures). A reset
//! by either would hide writes from the other, so every page one side resets
//! is kept for the other until that side reads it.
//!
//! The 4 KiB granularity and the OS's own ordering (ascending) are kept.

use core::ffi::c_void;
use std::collections::BTreeSet;
use std::sync::Mutex;

use windows_sys::Win32::System::Memory::{GetWriteWatch, ResetWriteWatch};

const RESET: u32 = 1;

/// Pages the runtime reset that the title has not read yet.
static OWED_TITLE: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());
/// Pages the title reset that the runtime has not read yet.
static OWED_TUXBOX: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());
/// One query-and-hand-over at a time, so no page falls between the two sets.
static QUERY: Mutex<()> = Mutex::new(());
/// Ranges each side has ever queried or reset (base -> bytes): [title, runtime].
static WATCHED: Mutex<[std::collections::BTreeMap<usize, usize>; 2]> = Mutex::new([std::collections::BTreeMap::new(), std::collections::BTreeMap::new()]);

/// test165: the 4K intro's watch refresh took 17 ms in one call. Every page
/// one side reset was kept for the other, even in ranges the other never
/// watches (the title watches none of Bink's planes, the runtime none of the
/// title's own streaming), so both owed sets grew without bound and every
/// query paid for it. A page is now owed only to a side that has watched a
/// range holding it. TUXBOX_WATCH_OWE_WATCHED=0 owes every page as before.
fn owe_watched() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_WATCH_OWE_WATCHED").map_or(true, |v| v.trim() != "0"))
}

fn note_watched(side: usize, base: usize, bytes: usize) {
    let mut w = lock(&WATCHED);
    let e = w[side].entry(base).or_insert(0);
    *e = (*e).max(bytes);
}

/// Pages of `pages` (ascending) inside a range `side` has watched.
fn owed_to(side: usize, pages: &[usize]) -> Vec<usize> {
    if !owe_watched() {
        return pages.to_vec();
    }
    let w = lock(&WATCHED);
    pages.iter().copied().filter(|&p| {
        // Nested or overlapping ranges: any of the nearest bases below covers it.
        w[side].range(..=p).rev().take(64).any(|(&b, &n)| p < b.saturating_add(n))
            // A range may start inside the page (its base rounded down).
            || w[side].range(p..p + 0x1000).next().is_some()
    }).collect()
}

/// test169: the runtime's watch refresh took 29 ms/frame in 14 queries.
/// Per side every 10 s: queries, MiB asked, OS pages returned, and ms waiting
/// for the query lock, in GetWriteWatch, and in the owed sets.
/// TUXBOX_WATCH_STATS=0 off.
struct Stat {
    side: usize,
    bytes: usize,
    os_pages: usize,
    times: [std::time::Instant; 3],
}

static STATS: Mutex<[[u64; 6]; 2]> = Mutex::new([[0; 6]; 2]);

impl Drop for Stat {
    fn drop(&mut self) {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var("TUXBOX_WATCH_STATS").map_or(true, |v| v.trim() != "0")) {
            return;
        }
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10);
        let [t0, t1, t2] = self.times;
        let us = |a: std::time::Instant, b: std::time::Instant| b.duration_since(a).as_micros() as u64;
        let mut stats = lock(&STATS);
        let s = &mut stats[self.side];
        s[0] += 1;
        s[1] += self.bytes as u64;
        s[2] += self.os_pages as u64;
        s[3] += us(t0, t1);
        s[4] += us(t1, t2);
        s[5] += us(t2, std::time::Instant::now());
        let secs = START.get_or_init(std::time::Instant::now).elapsed().as_secs();
        let next = NEXT.load(std::sync::atomic::Ordering::Relaxed);
        if secs < next || NEXT.compare_exchange(next, secs + 10, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed).is_err() {
            return;
        }
        let line = |n: &str, s: &[u64; 6]| format!("{n} {} queries {} MiB, {} OS pages, ms lock {:.1} os {:.1} owed {:.1}",
            s[0], s[1] >> 20, s[2], s[3] as f64 / 1e3, s[4] as f64 / 1e3, s[5] as f64 / 1e3);
        let text = format!("write watch last 10 s: {}; {} (TUXBOX_WATCH_STATS=0 off)", line("title", &stats[0]), line("runtime", &stats[1]));
        *stats = [[0; 6]; 2];
        drop(stats);
        crate::loader::log(&text);
    }
}

/// ztrx victory-gpu watch.rs (decision 0007 "Copies of title memory",
/// Rust, copied in its design with credit to its authors): one journal of
/// each 64 KiB block's last write, numbered by the asking that found it, and
/// per reader and range the asking it last read through. A reader is told
/// the blocks written after that; no page moves between per-side sets, the
/// kernel's answer buffer is reused, and the lock is held only for map
/// steps. test169: 14 runtime queries a frame cost 29 ms (ztrx: 30-70 us a
/// kernel call). Blocks are reported as their 4 KiB pages inside the range
/// (a block written anywhere counts whole, which only over-reports).
/// TUXBOX_WATCH_JOURNAL=0 keeps the owed sets.
fn journal_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_WATCH_JOURNAL").map_or(true, |v| v.trim() != "0"))
}

const BLOCK: usize = 64 << 10;

struct Journal {
    asked: u64,
    written: std::collections::BTreeMap<usize, u64>,
    /// (reader, block) -> the asking through which it last read in that
    /// block and which of its sixteen 4 KiB pages it has read since the
    /// block's last write (a range may end inside a block).
    seen: std::collections::BTreeMap<(usize, usize), (u64, u16)>,
    found: Vec<usize>,
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal { asked: 0, written: std::collections::BTreeMap::new(), seen: std::collections::BTreeMap::new(), found: Vec::new() });

impl Journal {
    /// Asks the kernel (resetting) about [base, base+bytes) and journals the
    /// blocks it found written under a new asking number.
    unsafe fn ask(&mut self, base: *const c_void, bytes: usize) -> Result<u64, u32> {
        let want = bytes.div_ceil(0x1000).max(1);
        self.found.clear();
        self.found.reserve(want);
        let mut count = want;
        let mut gran = 0u32;
        let failed = unsafe { GetWriteWatch(RESET, base, bytes, self.found.as_mut_ptr().cast(), &mut count, &mut gran) };
        if failed != 0 {
            return Err(failed);
        }
        // SAFETY: the kernel wrote `count` (<= want) entries.
        unsafe { self.found.set_len(count.min(want)) };
        self.asked += 1;
        let n = self.asked;
        let mut last = usize::MAX;
        for &p in &self.found {
            let b = p / BLOCK;
            if b != last {
                self.written.insert(b, n);
                last = b;
            }
        }
        Ok(n)
    }
}

/// Pages of block `b` inside [start, end) as a 16-bit mask.
fn page_mask(b: usize, start: usize, end: usize) -> u16 {
    let lo = (start.max(b * BLOCK) - b * BLOCK) / 0x1000;
    let hi = (end.min((b + 1) * BLOCK) - b * BLOCK).div_ceil(0x1000);
    (lo..hi).fold(0u16, |m, i| m | 1 << i)
}

/// Records that a reader read `pages` of a block last written at `w`, at asking `n`.
fn mark(seen: &mut std::collections::BTreeMap<(usize, usize), (u64, u16)>, key: (usize, usize), w: u64, n: u64, pages: u16) {
    let e = seen.entry(key).or_insert((0, 0));
    let kept = if w > e.0 { 0 } else { e.1 };
    *e = (n, kept | pages);
}

unsafe fn journal_query(me: usize, reset: bool, base: *const c_void, bytes: usize, out: *mut *mut c_void, count: *mut usize, granularity: *mut u32) -> u32 {
    let t0 = std::time::Instant::now();
    let mut j = lock(&JOURNAL);
    let t1 = std::time::Instant::now();
    let n = match unsafe { j.ask(base, bytes) } {
        Ok(n) => n,
        Err(e) => return e,
    };
    let t2 = std::time::Instant::now();
    let _stat = Stat { side: me, bytes, os_pages: j.found.len(), times: [t0, t1, t2] };
    let (start, end) = (base as usize, (base as usize).saturating_add(bytes));
    let capacity = unsafe { count.read() };
    let mut reported = 0usize;
    let mut read = Vec::new();
    let Journal { written, seen, .. } = &mut *j;
    'blocks: for (&b, &w) in written.range(start / BLOCK..=(end.max(1) - 1) / BLOCK) {
        let (n_seen, mask) = seen.get(&(me, b)).copied().unwrap_or((0, 0));
        let fresh = if w > n_seen { 0 } else { mask };
        let from = (b * BLOCK).max(start & !0xfff);
        let to = ((b + 1) * BLOCK).min(end);
        let mut p = from;
        let mut got = 0u16;
        while p < to {
            let bit = 1u16 << ((p - b * BLOCK) / 0x1000);
            if fresh & bit == 0 {
                if reported == capacity {
                    break 'blocks;
                }
                unsafe { out.add(reported).write(p as *mut c_void) };
                reported += 1;
            }
            got |= bit;
            p += 0x1000;
        }
        read.push((b, w, got));
    }
    if reset {
        for (b, w, got) in read {
            mark(seen, (me, b), w, n, got);
        }
    }
    unsafe {
        count.write(reported);
        granularity.write(0x1000);
    }
    0
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// OS pages written in [base, base+bytes), resetting them when `reset`.
unsafe fn os_pages(reset: bool, base: *const c_void, bytes: usize, granularity: &mut u32) -> Result<Vec<usize>, u32> {
    let mut pages = vec![core::ptr::null_mut::<c_void>(); bytes.div_ceil(0x1000).max(1)];
    let mut count = pages.len();
    let failed = unsafe { GetWriteWatch(if reset { RESET } else { 0 }, base, bytes, pages.as_mut_ptr(), &mut count, granularity) };
    if failed != 0 {
        return Err(failed);
    }
    pages.truncate(count);
    Ok(pages.into_iter().map(|p| p as usize).collect())
}

/// Shared body: `mine` holds pages owed to the caller, `theirs` to the other side.
unsafe fn query(
    reset: bool, base: *const c_void, bytes: usize, out: *mut *mut c_void, count: *mut usize, granularity: *mut u32,
    mine: &Mutex<BTreeSet<usize>>, theirs: &Mutex<BTreeSet<usize>>,
) -> u32 {
    let (me, other) = if std::ptr::eq(mine, &OWED_TITLE) { (0, 1) } else { (1, 0) };
    if count.is_null() || out.is_null() || granularity.is_null() {
        return unsafe { GetWriteWatch(if reset { RESET } else { 0 }, base, bytes, out, count, granularity) };
    }
    if journal_on() {
        return unsafe { journal_query(me, reset, base, bytes, out, count, granularity) };
    }
    let t0 = std::time::Instant::now();
    let _one = lock(&QUERY);
    let t1 = std::time::Instant::now();
    note_watched(me, base as usize, bytes);
    let mut gran = 0;
    let os = match unsafe { os_pages(reset, base, bytes, &mut gran) } {
        Ok(p) => p,
        Err(e) => return e,
    };
    let t2 = std::time::Instant::now();
    let _stat = Stat { side: me, bytes, os_pages: os.len(), times: [t0, t1, t2] };
    let range = base as usize..(base as usize).saturating_add(bytes);
    if reset {
        let owed = owed_to(other, &os);
        lock(theirs).extend(owed);
    }
    let mut owed = lock(mine);
    let mut all: BTreeSet<usize> = os.into_iter().collect();
    all.extend(owed.range(range.clone()).copied());
    let capacity = unsafe { count.read() };
    let reported: Vec<usize> = all.into_iter().take(capacity).collect();
    if reset {
        for p in &reported {
            owed.remove(p);
        }
    }
    for (i, p) in reported.iter().enumerate() {
        unsafe { out.add(i).write(*p as *mut c_void) };
    }
    unsafe {
        count.write(reported.len());
        granularity.write(gran);
    }
    0
}

/// The title's view (XMemGetWriteWatch and its kernel32 GetWriteWatch import).
pub(crate) unsafe extern "system" fn title_get(flags: u32, base: *const c_void, bytes: usize, out: *mut *mut c_void, count: *mut usize, granularity: *mut u32) -> u32 {
    unsafe { query(flags & RESET != 0, base, bytes, out, count, granularity, &OWED_TITLE, &OWED_TUXBOX) }
}

/// The title's ResetWriteWatch import: pages it drops stay owed to the runtime.
pub(crate) unsafe extern "system" fn title_reset(base: *const c_void, bytes: usize) -> u32 {
    if journal_on() {
        let mut j = lock(&JOURNAL);
        return match unsafe { j.ask(base, bytes) } {
            Ok(n) => {
                let (start, end) = (base as usize, (base as usize).saturating_add(bytes));
                let blocks: Vec<usize> = j.written.range(start / BLOCK..=(end.max(1) - 1) / BLOCK).map(|(&b, _)| b).collect();
                for b in blocks {
                    let w = j.written[&b];
                    mark(&mut j.seen, (0, b), w, n, page_mask(b, start, end));
                }
                0
            }
            Err(_) => unsafe { ResetWriteWatch(base, bytes) },
        };
    }
    let _one = lock(&QUERY);
    let mut gran = 0;
    match unsafe { os_pages(true, base, bytes, &mut gran) } {
        Ok(pages) => {
            note_watched(0, base as usize, bytes);
            let owed = owed_to(1, &pages);
            lock(&OWED_TUXBOX).extend(owed);
            let range = base as usize..(base as usize).saturating_add(bytes);
            let mut owed = lock(&OWED_TITLE);
            let gone: Vec<usize> = owed.range(range).copied().collect();
            for p in gone {
                owed.remove(&p);
            }
            0
        }
        Err(_) => unsafe { ResetWriteWatch(base, bytes) },
    }
}

/// d3d12: pages written in [base, base+bytes) since its last take (reset).
/// Returns 0 on success, else the GetWriteWatch error (not a watched range).
#[unsafe(export_name = "TuxBoxTakeWriteWatch")]
pub unsafe extern "system" fn tuxbox_take(base: *const c_void, bytes: usize, out: *mut *mut c_void, count: *mut usize, granularity: *mut u32) -> u32 {
    unsafe { query(true, base, bytes, out, count, granularity, &OWED_TUXBOX, &OWED_TITLE) }
}

/// Called from loader::install: the exe's own write-watch imports share the list.
pub(crate) unsafe fn install() {
    let patched = unsafe {
        crate::loader::patch_imports(
            |d| d == "kernel32.dll" || d == "kernelbase.dll" || d.starts_with("api-ms-win-core-memory"),
            &[("GetWriteWatch", title_get as *const () as usize), ("ResetWriteWatch", title_reset as *const () as usize)],
        )
    };
    crate::loader::log(&format!("write watch: {patched} GetWriteWatch/ResetWriteWatch import(s) share the list with the d3d12 runtime"));
}
