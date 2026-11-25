//! Which title memory the title wrote since a host copy took it (decision
//! 0007, "Copies of title memory"). The runtime reserves title GPU memory
//! with write watching (`victory-hle` xmem), so the kernel keeps a dirty bit
//! per 4 KiB page; asking for a range's written pages also clears them. Host
//! copies of the same memory can overlap (nested buffers, textures in
//! buffers), so what one asks is kept here for the others: a journal of
//! each page's last write, numbered by the asking.
//!
//! Physical-page views (xmem's sections) cannot be write-watched. There a
//! copy leaves each 64 KiB block it took read-only, and our vectored handler
//! takes the first write to it: it notes the block as written and gives the
//! write access back. The runtime tells us when it maps or unmaps views
//! ([`title_memory_remapped`]), which resets their protection. Blocks found
//! wholly uncommitted are skipped while they remain so; one query can
//! confirm a whole region, including ordinary reservations between views.
//!
//! Writes into physical pages through another view are seen too: the
//! runtime tells us the views of the pages it wrote itself ([`writing`],
//! xmem's `write_physical` and DirectStorage's page destinations), and a
//! write by the title through any view of a guarded page faults like one
//! at the guarded address (the guard makes every view of the page
//! read-only, [`open`] gives them all back; not a page of more than
//! [`MANY_VIEWS`] views). A
//! system call writing into a guarded block fails (none is known to).

use std::collections::BTreeSet;
use std::sync::Mutex;

/// The unit tracked and copied: 64 KiB, the console's page (the kernel's
/// 4 KiB pages fold into it). Finer units made the checks of the world's
/// big buffers cost more than the copies they saved.
pub(crate) const PAGE: usize = 64 << 10;
/// The kernel's page, the unit of its answers.
const KERNEL_PAGE: usize = 4096;

/// A title-memory range whose changed pages another subsystem needs to read.
pub(crate) struct Tracked {
    start: usize,
    end: usize,
    seen: Mutex<std::collections::BTreeMap<usize, u64>>,
}

/// Tracks a runtime GPU-memory range, including physical-page views when
/// `views` is true. Other process memory is not tracked.
pub(crate) fn track(start: usize, len: usize, views: bool) -> Option<Tracked> {
    let end = start.checked_add(len)?;
    if len == 0 || writes(start, end, views).is_none() {
        return None;
    }
    Some(Tracked {
        start,
        end,
        seen: Mutex::default(),
    })
}

struct Journal {
    /// Room for the kernel's answer.
    found: Vec<usize>,
    /// Page ranges asked ahead for a submission's refreshes ([`ask_ahead`])
    /// and the asking that did: `writes` answers for a range inside one
    /// from the journal, without another kernel call.
    ahead: Vec<(usize, usize)>,
    ahead_asked: u64,
    /// The ahead asking stands for a whole submission ([`ask_once`]): no
    /// other ahead asking is made, and `ahead` stays, until
    /// [`ask_once_end`].
    kept: bool,
}

// (Neptune) Read by many at once: the refreshes of a submission run on several threads, and
// each asks the journal (`writes`); behind one lock they stood in line (a third of the
// submitting thread's time in a sample).
static JOURNAL: std::sync::RwLock<Journal> = std::sync::RwLock::new(Journal {
    found: Vec::new(),
    ahead: Vec::new(),
    ahead_asked: 0,
    kept: false,
});

/// (Neptune) Numbers the askings; 0 is "never". Outside the journal's lock, with the pages
/// below: the refreshes of a submission run on several threads and each asks what was
/// written in its range, the thread that copies back notes its own writes, and behind one
/// lock they stood in line (two fifths of the submitting thread's time in two samples).
static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The next asking's number.
fn next_asking() -> u64 {
    ASKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Journal pages to a part of the table: 4 GiB of addresses.
const PART: usize = 1 << 16;

/// Page number → the asking that last found it written (0: never), for one part.
struct Part([std::sync::atomic::AtomicU64; PART]);

/// The parts by `page / PART`, made when a page of theirs is first noted written and
/// kept for good.
static PARTS: [std::sync::atomic::AtomicPtr<Part>; 1 << 16] =
    [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 1 << 16];

fn part(page: usize, make: bool) -> Option<&'static Part> {
    part_of(&PARTS, page, make)
}

/// The clean blocks of the guards again (`Guards::clean`), 1 for a clean block: a table read
/// without the guards' lock. It is written under that lock, with the set. A refresh of view
/// memory asks whether its blocks are all clean, nearly all are, and the refreshes of a
/// submission run on several threads: behind the lock they waited for every thread that
/// was setting or opening a guard (an eighth of the submitting thread's time in a sample).
static CLEAN: [std::sync::atomic::AtomicPtr<Part>; 1 << 16] =
    [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 1 << 16];

/// (Neptune) `VICTORY_GUARD_HOT=<frames>` (0 or unset: off): a block of view memory that was
/// found written at three checks running, frame after frame, is left without its guard for
/// that many frames, and counts as written at every check meanwhile.
///
/// A block the title writes every frame cost a fault in the title's thread, the guard's
/// opening there, and the guard's setting again at the next refresh, each a change of
/// protection behind the guards' one lock: the refreshes of a submission waited for one
/// another there (a tenth of the submitting thread's time in a sample). Left open, such a
/// block is read and compared at each refresh of it, on the refreshes' own threads, and
/// nothing waits. Not a block shown in other views too: a write through this one could not
/// open theirs.
fn guard_hot() -> u64 {
    static FRAMES: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *FRAMES.get_or_init(|| std::env::var("VICTORY_GUARD_HOT").ok().and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// Of a block, for [`guard_hot`]: the frame it was last found written in (40 bits), how
/// many checks running found it so (16 bits), and whether it is left open since that frame.
static HOT: [std::sync::atomic::AtomicPtr<Part>; 1 << 16] =
    [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 1 << 16];
const HOT_FRAME: u64 = (1 << 40) - 1;
const HOT_OPEN: u64 = 1 << 63;

fn hot_get(block: usize) -> u64 {
    part_of(&HOT, block, false).map_or(0, |part| part.0[block % PART].load(std::sync::atomic::Ordering::Acquire))
}

fn hot_set(block: usize, value: u64) {
    if let Some(part) = part_of(&HOT, block, value != 0) {
        part.0[block % PART].store(value, std::sync::atomic::Ordering::Release);
    }
}

/// Whether `block` is left open now ([`guard_hot`]).
fn left_open(block: usize, frame: u64, keep: u64) -> bool {
    let v = hot_get(block);
    v & HOT_OPEN != 0 && frame.wrapping_sub(v & HOT_FRAME) <= keep
}

/// A check found `block` written and without a guard: whether it is left so ([`guard_hot`]).
/// Under the guards' lock.
fn stays_open(g: &Guards, block: usize, frame: u64, keep: u64) -> bool {
    if g.aliased.contains(&block) {
        return false;
    }
    let v = hot_get(block);
    let (was, streak) = (v & HOT_FRAME, (v >> 40) & 0xffff);
    if v & HOT_OPEN != 0 {
        if frame.wrapping_sub(was) <= keep {
            return true;
        }
        // Its time is over: guarded again, and counted from nothing.
        hot_set(block, frame & HOT_FRAME);
        return false;
    }
    let streak = match frame.wrapping_sub(was) {
        0 if v != 0 => streak,
        1 | 2 if v != 0 => streak + 1,
        _ => 1,
    };
    if streak >= 3 && crate::inplace::pages_of(block * BLOCK).len() < 2 {
        hot_set(block, HOT_OPEN | (frame & HOT_FRAME));
        HOT_LEFT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return true;
    }
    hot_set(block, (streak.min(0xffff) << 40) | (frame & HOT_FRAME));
    false
}

/// Blocks left open since the start ([`guard_hot`]).
static HOT_LEFT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// (Neptune) Since the last line in the log: write faults that opened a guard, changes of
/// protection that set guards, the blocks they set, checks that took the guards' lock to
/// change something, checks answered without it; kernel askings of [`ask_once`], the 4 KiB
/// pages they walked, the written pages they found.
static COUNTS: [std::sync::atomic::AtomicU64; 8] = [const { std::sync::atomic::AtomicU64::new(0) }; 8];

fn count(which: usize, n: u64) {
    COUNTS[which].fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// (Neptune) The absent blocks of the guards again (`Guards::absent`), as [`CLEAN`] is of the
/// clean ones: a range with blocks that are not committed never was "all clean", so every
/// check of it took the guards' lock to find nothing to do (a thousand checks a frame under
/// the lock for three hundred that set a guard).
static ABSENT: [std::sync::atomic::AtomicPtr<Part>; 1 << 16] =
    [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 1 << 16];

/// Notes `block` absent or not, with `Guards::absent`.
fn absent_set(block: usize, absent: bool) {
    if let Some(part) = part_of(&ABSENT, block, absent) {
        part.0[block % PART].store(u64::from(absent), std::sync::atomic::Ordering::Release);
    }
}

fn absent_one(block: usize) -> bool {
    part_of(&ABSENT, block, false).is_some_and(|part| part.0[block % PART].load(std::sync::atomic::Ordering::Acquire) != 0)
}

/// Notes `block` clean or not, with `Guards::clean`.
fn clean_set(block: usize, clean: bool) {
    if let Some(part) = part_of(&CLEAN, block, clean) {
        part.0[block % PART].store(u64::from(clean), std::sync::atomic::Ordering::Release);
    }
}

/// Whether the blocks `[first, last)` are all clean.
fn clean_all(first: usize, last: usize) -> bool {
    let mut block = first;
    while block < last {
        let stop = ((block / PART + 1) * PART).min(last);
        let Some(part) = part_of(&CLEAN, block, false) else {
            return false;
        };
        if (block..stop).any(|b| part.0[b % PART].load(std::sync::atomic::Ordering::Acquire) == 0) {
            return false;
        }
        block = stop;
    }
    true
}

fn part_of(table: &'static [std::sync::atomic::AtomicPtr<Part>; 1 << 16], page: usize, make: bool) -> Option<&'static Part> {
    use std::sync::atomic::Ordering::{AcqRel, Acquire};
    let slot = table.get(page / PART)?;
    let mut at = slot.load(Acquire);
    if at.is_null() {
        if !make {
            return None;
        }
        let layout = std::alloc::Layout::new::<Part>();
        // SAFETY: a layout of non-zero size; zeroed memory is a valid `Part`.
        let fresh = unsafe { std::alloc::alloc_zeroed(layout) }.cast::<Part>();
        if fresh.is_null() {
            return None;
        }
        match slot.compare_exchange(std::ptr::null_mut(), fresh, AcqRel, Acquire) {
            Ok(_) => at = fresh,
            Err(other) => {
                // SAFETY: ours alone, allocated with this layout just above.
                unsafe { std::alloc::dealloc(fresh.cast(), layout) };
                at = other;
            }
        }
    }
    // SAFETY: a part once made is never freed.
    Some(unsafe { &*at })
}

/// (Neptune) Whether written-ness is also kept per 4 KiB (`VICTORY_WATCH_4K=1`; off by
/// default): where the kernel says which 4 KiB pages were written, [`stale_mask`] tells a
/// refresh which sub-pages of a stale 64 KiB page it need copy.
pub(crate) fn watch_4k() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WATCH_4K").is_some_and(|v| v == "1"))
}

/// Per 64 KiB page: the asking that last noted it written without saying which 4 KiB pages
/// (guards, our own writes, kernel writes we were told of): such a page counts written whole.
static WHOLE: [std::sync::atomic::AtomicPtr<Part>; 1 << 16] =
    [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 1 << 16];

/// Per 4 KiB page (`address / 4096`): the asking that last found it written by the kernel.
static SUBS: [std::sync::atomic::AtomicPtr<Part>; 1 << 16] =
    [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; 1 << 16];

/// Notes `page` written at asking `now`; what it held before (0: nothing). Says nothing of
/// which 4 KiB pages, so with [`watch_4k`] the whole page counts.
fn written_set(page: usize, now: u64) -> u64 {
    use std::sync::atomic::Ordering::Release;
    if watch_4k() {
        // Before the page's stamp, which a reader looks at first.
        match part_of(&WHOLE, page, true) {
            Some(part) => part.0[page % PART].store(now, Release),
            None => {}
        }
    }
    stamp_set(page, now)
}

/// The page's stamp alone: what it held before.
fn stamp_set(page: usize, now: u64) -> u64 {
    match part(page, true) {
        Some(part) => part.0[page % PART].swap(now, std::sync::atomic::Ordering::AcqRel),
        None => 0,
    }
}

/// Notes the 4 KiB page at `address` found written by the kernel at asking `now` (as
/// [`written_set`], but only that sub-page). Where the sub-page table cannot be made the
/// whole page counts.
fn found_set(address: usize, now: u64) {
    let (page, sub) = (address / PAGE, address / KERNEL_PAGE);
    if !watch_4k() {
        written_set(page, now);
        return;
    }
    let Some(part) = part_of(&SUBS, sub, true) else {
        written_set(page, now);
        return;
    };
    let cell = &part.0[sub % PART];
    cell.store(now, std::sync::atomic::Ordering::Release);
    // Noted with a later asking meanwhile (the copy-back's own write): what the kernel found
    // is newer than that, and gets a number that says so.
    if stamp_set(page, now) > now {
        let later = next_asking();
        cell.store(later, std::sync::atomic::Ordering::Release);
        stamp_set(page, later);
    }
}

/// With [`watch_4k`]: which 4 KiB pages (bit k: the k-th) of `page` were written after
/// asking `at`, and `None` where that is not known, and the whole page is to be taken: never
/// copied (`at == 0`), written without a 4 KiB answer, or nothing found.
pub(crate) fn stale_mask(page: usize, at: u64) -> Option<u16> {
    use std::sync::atomic::Ordering::Acquire;
    if !watch_4k() || at == 0 {
        return None;
    }
    if part_of(&WHOLE, page, false).is_some_and(|p| p.0[page % PART].load(Acquire) > at) {
        return None;
    }
    let sub = page * (PAGE / KERNEL_PAGE);
    let part = part_of(&SUBS, sub, false)?;
    let stamps: [u64; PAGE / KERNEL_PAGE] = std::array::from_fn(|k| part.0[(sub + k) % PART].load(Acquire));
    mask_of(&stamps, at)
}

/// The sub-pages stamped after `at`; `None` if there are none (the page's stamp then rests on
/// something this cannot see: copy it whole).
fn mask_of(stamps: &[u64; PAGE / KERNEL_PAGE], at: u64) -> Option<u16> {
    let mask = stamps.iter().enumerate().fold(0u16, |m, (k, &s)| m | (u16::from(s > at) << k));
    (mask != 0).then_some(mask)
}

/// Bytes refreshed through 4 KiB masks and the pages so, then bytes and pages that fell back
/// to the whole 64 KiB.
static COPY_4K: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// Notes one stale page refreshed: `mask` (`None`: whole) -> its 4 KiB pages.
pub(crate) fn note_4k(mask: Option<u16>) {
    let (i, n) = match mask {
        Some(m) => (0, u64::from(m.count_ones())),
        None => (2, 16),
    };
    COPY_4K[i].fetch_add(n * KERNEL_PAGE as u64, std::sync::atomic::Ordering::Relaxed);
    COPY_4K[i + 1].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// [`COPY_4K`], taken.
pub(crate) fn copy_4k_stats() -> [u64; 4] {
    [0, 1, 2, 3].map(|i| COPY_4K[i].swap(0, std::sync::atomic::Ordering::Relaxed))
}

/// The pages of `[first, last)` ever noted written, in order, each with the asking that
/// last found it so.
fn written_in(first: usize, last: usize) -> Vec<(usize, u64)> {
    let mut out = Vec::new();
    let mut page = first;
    while page < last {
        let stop = ((page / PART + 1) * PART).min(last);
        if let Some(part) = part(page, false) {
            for p in page..stop {
                let w = part.0[p % PART].load(std::sync::atomic::Ordering::Acquire);
                if w != 0 {
                    out.push((p, w));
                }
            }
        }
        page = stop;
    }
    out
}

/// Ranges of one submission closer than this are asked about together:
/// walking the pages between them costs less than another kernel call (a
/// call that resets written pages costs 30-70 microseconds whatever the
/// range).
const AHEAD_GAP: usize = (8 << 20) / PAGE;
/// The most one asking covers (its answer needs 8 bytes per 4 KiB page).
const AHEAD_MAX: usize = (1 << 30) / PAGE;

/// (Neptune) `VICTORY_ASK_WIDE_MB=<n>`: a submission's ranges are asked about together when
/// they lie within `n` MiB of one another, and one asking covers up to `n` MiB (at most
/// 16384), in place of 8 MiB and 1 GiB. A frame made about 3,000 askings, each resetting
/// pages and with that flushing every core's address cache (36 ms a frame); a reservation
/// asked about whole is one such call per submission. Unset or 0: as before.
fn ahead_limits() -> (usize, usize) {
    static WIDE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let mb = *WIDE.get_or_init(|| {
        std::env::var("VICTORY_ASK_WIDE_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(16384)
    });
    if mb == 0 {
        (AHEAD_GAP, AHEAD_MAX)
    } else {
        ((mb << 20) / PAGE, ((mb.max(1024)) << 20) / PAGE)
    }
}

/// Asks the kernel about many ranges `(start, end)` at once, before the
/// [`writes`] of each: ranges close together are one asking, and `writes`
/// then answers from the journal until [`ask_ahead_done`]. The world's
/// submissions refresh thousands of host copies (6,664 askings a frame,
/// 456 ms, in run 14). Ranges that are not write-watched, or lie alone, are
/// left to `writes`.
pub(crate) fn ask_ahead(ranges: &[(usize, usize)]) {
    let mut pieces: Vec<(usize, usize)> = ranges
        .iter()
        .filter(|r| r.0 < r.1)
        .map(|&(start, end)| (start / PAGE, end.div_ceil(PAGE)))
        .collect();
    if pieces.len() < 2 {
        return;
    }
    pieces.sort_unstable();
    let mut j = JOURNAL.write().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    // The submission's one asking stands ([`ask_once`]): what it does not
    // hold is left to `writes`.
    if j.kept {
        return;
    }
    j.ahead_asked = next_asking();
    j.ahead.clear();
    let (gap, most) = ahead_limits();
    let mut i = 0;
    while i < pieces.len() {
        let first = pieces[i].0;
        let mut last = pieces[i].1;
        let mut k = i + 1;
        while k < pieces.len()
            && pieces[k].0 <= last + gap
            && last.max(pieces[k].1) - first <= most
        {
            last = last.max(pieces[k].1);
            k += 1;
        }
        ask_run(j, &pieces[i..k]);
        i = k;
    }
}

/// Since the last frame line: the kernel askings that took a millisecond
/// or more, their microseconds, and the longest asking's. An asking walks
/// page tables, some ten microseconds for most ranges; in run 75 the 2,644
/// askings of a frame took 219 ms in the lines where the pass over the
/// heap's views ran long, 25-50 ms in the others. These numbers say
/// whether a few askings wait for something or all of them are slow.
#[cfg(feature = "diagnostics")]
static SLOW: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

#[cfg(feature = "diagnostics")]
fn slow(us: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    if us >= 1000 {
        SLOW[0].fetch_add(1, Relaxed);
        SLOW[1].fetch_add(us, Relaxed);
    }
    SLOW[2].fetch_max(us, Relaxed);
}

/// [`SLOW`], taken.
#[cfg(feature = "diagnostics")]
pub(crate) fn slow_asks() -> [u64; 3] {
    [0, 1, 2].map(|i| SLOW[i].swap(0, std::sync::atomic::Ordering::Relaxed))
}

/// The submission's refreshes are done: `writes` asks the kernel again.
pub(crate) fn ask_ahead_done() {
    let mut j = JOURNAL.write().unwrap_or_else(|e| e.into_inner());
    if !j.kept {
        j.ahead.clear();
    }
}

/// Whether a submission asks the kernel once, before its first refresh,
/// about every range its lists refresh (`VICTORY_ASK_ONCE=1`; off by
/// default).
///
/// A submission's refreshes are done in groups, between the title's own
/// buffer copies (each copy flushes the refreshes recorded before it), and
/// every group asked the kernel again: 2,644 askings a frame, 70 a
/// submission. With the switch the journal answers all of them from one
/// asking made when the submission starts.
///
/// What changes: memory the title's other threads write while the
/// submission is being prepared is seen by the next submission's asking,
/// not by this one's later groups. What this submission's lists read was
/// written before they were submitted, so before the asking. Our own
/// writes into title memory while preparing are told to the journal
/// ([`wrote`]) or end the asking ([`ask_once_end`]).
pub(crate) fn once() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // On by default since run 81 (with card buffers: 375-450 ms a frame
    // against 573-647, the picture unchanged over four minutes);
    // `VICTORY_ASK_ONCE=0` asks per range, as before.
    *ON.get_or_init(|| std::env::var_os("VICTORY_ASK_ONCE").is_none_or(|v| v != "0"))
}

/// With [`once`]: asks about `ranges` (everything a submission's lists
/// refresh) as [`ask_ahead`] does, and lets that asking stand until
/// [`ask_once_end`].
pub(crate) fn ask_once(ranges: &[(usize, usize)]) {
    if !once() {
        return;
    }
    ask_once_end();
    if quiet_frames() > 0 {
        ask_once_quiet(ranges);
        return;
    }
    ask_ahead(ranges);
    let mut j = JOURNAL.write().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    if j.ahead.is_empty() {
        // `ask_ahead` asked nothing (fewer than two ranges).
        j.ahead_asked = next_asking();
    }
    // `ask_ahead` leaves a range that lies alone to `writes`, which asks
    // once per flush: with one asking for the submission that was every
    // such range asked again at each of its 70 flushes (run 76: 2,300
    // askings a frame with the switch on, as without it). They are asked
    // here, once; what is not write-watched stays with `writes`.
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let (first, last) = (start / PAGE, end.div_ceil(PAGE));
        if !j.ahead.iter().any(|&(a, b)| a <= first && last <= b) && ask_kernel(j, first, last) {
            j.ahead.push((first, last));
        }
    }
    j.kept = !j.ahead.is_empty();
}

/// (Neptune) `VICTORY_ASK_QUIET=<frames>` (0 or unset: off): memory nothing was found written
/// in for that many frames is asked about once a frame, not once a submission.
///
/// The kernel walks every page of a range it is asked about, and each of a frame's thirty
/// submissions asked about the ranges its lists refresh, mostly the same ones: some 3,000
/// askings over 11 GB, a third of the submitting thread's time. Title memory is taken in
/// blocks of 1 MiB here, and a block is asked about whole. One in which a written page was
/// found within the last `frames` frames is asked about at every submission that refreshes
/// part of it, as before; any other at the first such submission of a frame, and the
/// journal answers for it until the next frame.
///
/// What changes: a write into a quiet block that arrives after the first asking of the
/// frame is seen by the next frame, so a list later in the same frame draws from the bytes
/// of before, once. From then on the block is asked about at every submission.
fn quiet_frames() -> u64 {
    static FRAMES: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *FRAMES.get_or_init(|| std::env::var("VICTORY_ASK_QUIET").ok().and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// Blocks that may lie between two blocks of one asking (`VICTORY_ASK_QUIET_GAP_MB`, 32: a
/// clean page walked is 4 nanoseconds, a call that resets pages some 50 microseconds).
fn quiet_gap() -> usize {
    static GAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *GAP.get_or_init(|| std::env::var("VICTORY_ASK_QUIET_GAP_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(32))
}

/// Journal pages to a block of [`quiet_frames`].
const QUIET_BLOCK: usize = (1 << 20) / PAGE;

/// Of a block: the frame it was last asked about whole (`u64::MAX`: never), the last frame
/// a written page was found in it (`u64::MAX`: never), and whether asking about it whole
/// fails (it is not all inside one write-watched reservation).
#[derive(Clone, Copy)]
struct Quiet {
    asked: u64,
    written: u64,
    partial: bool,
}

const NEVER: Quiet = Quiet { asked: u64::MAX, written: u64::MAX, partial: false };

struct Blocks {
    blocks: std::collections::HashMap<usize, Quiet>,
    /// Since the last line in the log: frames, blocks asked about because they were
    /// written lately, blocks asked about as the first of a frame, ranges answered by the
    /// journal alone, ranges asked about on their own.
    told: [u64; 5],
    frame: u64,
}

static BLOCKS: Mutex<Option<Blocks>> = Mutex::new(None);

/// [`ask_once`] with [`quiet_frames`].
fn ask_once_quiet(ranges: &[(usize, usize)]) {
    let frame = crate::queue::presented();
    let keep = quiet_frames();
    let mut held = BLOCKS.lock().unwrap_or_else(|e| e.into_inner());
    let state = held.get_or_insert_with(|| Blocks { blocks: Default::default(), told: [0; 5], frame });
    let mut j = JOURNAL.write().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    j.ahead_asked = next_asking();
    j.ahead.clear();
    if state.frame != frame {
        state.told[0] += frame.wrapping_sub(state.frame).min(1 << 20);
        state.frame = frame;
        if state.told[0] >= 512 {
            let n = state.told[0] as f64;
            tracing::info!(
                "title memory asked about per frame (VICTORY_ASK_QUIET={keep}): {:.0} blocks of 1 MiB written lately, {:.0} quiet ones as the first of a frame, {:.0} ranges answered from the journal, {:.0} ranges on their own; {} blocks known",
                state.told[1] as f64 / n,
                state.told[2] as f64 / n,
                state.told[3] as f64 / n,
                state.told[4] as f64 / n,
                state.blocks.len()
            );
            state.told = [0; 5];
            let a = crate::fence::asked_taken();
            tracing::info!(
                "fence events per frame: {:.2} asked for a value already reached, {:.2} for one not yet reached, set {:.1} ms after they were asked for (in all, per frame)",
                a[0] as f64 / n, a[1] as f64 / n, a[2] as f64 / n / 1000.0
            );
            let w = crate::frame::waits_taken();
            tracing::info!(
                "frame waits per frame: on a tick {:.2} origin ({:.1} ms slept) and {:.2} other ({:.1} ms); timed out {:.2} origin ({:.1} ms) and {:.2} other ({:.1} ms); longest time-out asked {} ms",
                w[0] as f64 / n, w[4] as f64 / n / 1000.0, w[1] as f64 / n, w[5] as f64 / n / 1000.0,
                w[2] as f64 / n, w[6] as f64 / n / 1000.0, w[3] as f64 / n, w[7] as f64 / n / 1000.0, w[8]
            );
            let c: Vec<f64> = COUNTS.iter().map(|c| c.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / n).collect();
            tracing::info!(
                "guards per frame: {:.0} write faults, {:.0} settings of {:.0} blocks, {:.0} checks under the lock, {:.0} without; askings per frame: {:.0} over {:.0} pages of 4 KiB, {:.0} found written",
                c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]
            );
        }
        if guard_hot() > 0 && frame % 512 == 0 {
            tracing::info!(
                "blocks of view memory left without their guard so far (VICTORY_GUARD_HOT={}): {}",
                guard_hot(),
                HOT_LEFT.load(std::sync::atomic::Ordering::Relaxed)
            );
        }
        if crate::shadow::copy_changed() && frame % 512 == 0 {
            let [read, differed] = crate::shadow::copy_changed_stats();
            let [quiet, through] = crate::shadow::quiet_back_stats();
            if watch_4k() {
                let [part_b, part_n, whole_b, whole_n] = copy_4k_stats();
                tracing::info!(
                    "stale pages refreshed (VICTORY_WATCH_4K=1): {} MB by 4 KiB masks ({} pages), {} MB whole ({} pages fell back to 64 KiB)",
                    part_b >> 20,
                    part_n,
                    whole_b >> 20,
                    whole_n
                );
            }
            tracing::info!(
                "refreshes of copies in the card's memory since the last such line (VICTORY_COPY_CHANGED=1): {} MB of title memory read, {} MB of it differed and went up; {} pages the copy-back kept quiet, {} of its writes through a view of our own (VICTORY_QUIET_BACK)",
                read >> 20,
                differed >> 20,
                quiet,
                through
            );
        }
    }
    // The blocks to ask about now.
    let mut wanted: Vec<usize> = Vec::new();
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let (first, last) = (start / PAGE, end.div_ceil(PAGE));
        for block in first / QUIET_BLOCK..=(last - 1) / QUIET_BLOCK {
            let q = *state.blocks.entry(block).or_insert(NEVER);
            if q.partial {
                continue;
            }
            let lately = q.written != u64::MAX && frame.wrapping_sub(q.written) <= keep;
            if q.asked != frame || lately {
                wanted.push(block);
            }
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    let mut i = 0;
    while i < wanted.len() {
        let mut k = i + 1;
        // Blocks up to 16 MiB apart are one asking: the kernel walks the pages between
        // them in less time than another call takes (and they count as asked about).
        while k < wanted.len() && wanted[k] <= wanted[k - 1] + 1 + quiet_gap() && wanted[k] - wanted[i] < 4096 {
            k += 1;
        }
        let run = &wanted[i..k];
        let whole = ask_blocks(j, state, run[0], run[run.len() - 1] + 1, frame, keep);
        if !whole && run.len() > 1 {
            // One of them is not all write-watched memory: each on its own.
            for &block in run {
                if !ask_blocks(j, state, block, block + 1, frame, keep)
                    && let Some(q) = state.blocks.get_mut(&block)
                {
                    q.partial = true;
                }
            }
        } else if !whole && let Some(q) = state.blocks.get_mut(&run[0]) {
            q.partial = true;
        }
        i = k;
    }
    // What the journal now answers for: a range whose blocks were all asked about this
    // frame. Any other is asked about on its own, as without the switch.
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let (first, last) = (start / PAGE, end.div_ceil(PAGE));
        let covered = (first / QUIET_BLOCK..=(last - 1) / QUIET_BLOCK)
            .all(|block| state.blocks.get(&block).is_some_and(|q| !q.partial && q.asked == frame));
        if covered {
            state.told[3] += 1;
            j.ahead.push((first, last));
        } else if !j.ahead.iter().any(|&(a, b)| a <= first && last <= b) && ask_kernel(j, first, last) {
            state.told[4] += 1;
            j.ahead.push((first, last));
        }
    }
    j.kept = !j.ahead.is_empty();
}

/// Asks about blocks `[first, last)` whole and notes the answer in them; false if the kernel
/// does not answer for all of it.
fn ask_blocks(j: &mut Journal, state: &mut Blocks, first: usize, last: usize, frame: u64, keep: u64) -> bool {
    let Some(count) = ask_kernel_found(j, first * QUIET_BLOCK, last * QUIET_BLOCK) else {
        return false;
    };
    for block in first..last {
        let q = state.blocks.entry(block).or_insert(NEVER);
        let lately = q.written != u64::MAX && frame.wrapping_sub(q.written) <= keep;
        state.told[if q.asked == frame || lately { 1 } else { 2 }] += 1;
        q.asked = frame;
    }
    for k in 0..count {
        let block = j.found[k] / PAGE / QUIET_BLOCK;
        if let Some(q) = state.blocks.get_mut(&block) {
            q.written = frame;
        }
    }
    true
}

/// (Neptune) Whether what the copy-back writes into title memory is kept from counting as the
/// title's writing for the copy it came from (`VICTORY_QUIET_BACK=1`; off by default).
///
/// The card's bytes go into title memory through the kernel, and the kernel then reports
/// those pages written like any other. The copy the bytes came from read them again at the
/// next submission, compared them, found nothing, and a range shaders write every frame
/// was looked at this way at every submission (an eighth of the submitting thread's time in
/// a sample, beside the refreshes' own reading). With the switch the thread that copies
/// back asks about the pages it is about to write (what the title wrote there is noted in
/// the journal as before), writes, resets the kernel's note of its own write, and notes the
/// pages in the journal itself: other copies of the same memory still see them written,
/// and the copy they came from is told it holds them ([`ours_after`]).
pub(crate) fn quiet_back() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_QUIET_BACK").is_some_and(|v| v == "1"))
}

/// Whether [`quiet_back`] also takes write-watched memory (`VICTORY_QUIET_BACK_WATCHED=1`; off
/// by default: it can lose a write of the title's, see `Shadow::card_merge_relay`).
pub(crate) fn quiet_back_watched() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_QUIET_BACK_WATCHED").is_some_and(|v| v == "1"))
}

/// Before our own write of `[at, at + len)` ([`quiet_back`]): asks the kernel about its 4 KiB
/// pages and notes what the title wrote there in the journal. The journal pages that held
/// such a write; `None` if the memory is not write-watched (nothing is to be done then).
pub(crate) fn ours_before(at: usize, len: usize) -> Option<Vec<usize>> {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    let first = at & !(KERNEL_PAGE - 1);
    let last = at.saturating_add(len).div_ceil(KERNEL_PAGE) * KERNEL_PAGE;
    let mut found = [0usize; 40];
    if len == 0 || (last - first) / KERNEL_PAGE > found.len() {
        return None;
    }
    let mut count = found.len();
    let mut granularity = 0;
    // SAFETY: the answer buffer holds `count` entries.
    let failed = unsafe {
        GetWriteWatch(
            WRITE_WATCH_FLAG_RESET,
            first as *const _,
            last - first,
            found.as_mut_ptr().cast(),
            &mut count,
            &mut granularity,
        )
    } != 0;
    if failed {
        return None;
    }
    let mut pages: Vec<usize> = found[..count].iter().map(|&at| at / PAGE).collect();
    pages.dedup();
    if !pages.is_empty() {
        let now = next_asking();
        for &at in &found[..count] {
            found_set(at, now);
        }
    }
    Some(pages)
}

/// After our own write of `[at, at + len)`, which [`ours_before`] answered for: the kernel
/// forgets it, and the journal takes the pages as written now. Per journal page: the asking
/// that had last found it written (0: none) and this one.
pub(crate) fn ours_after(at: usize, len: usize) -> Vec<(usize, u64, u64)> {
    use windows_sys::Win32::System::Memory::ResetWriteWatch;
    let first = at & !(KERNEL_PAGE - 1);
    let last = at.saturating_add(len).div_ceil(KERNEL_PAGE) * KERNEL_PAGE;
    if ours_together() {
        // Forgotten with the landing's other writes ([`ours_flush`]).
        OURS.lock().unwrap_or_else(|e| e.into_inner()).push((first, last));
    } else {
        // SAFETY: a range the kernel just answered for.
        unsafe { ResetWriteWatch(first as *const _, last - first) };
    }
    let now = next_asking();
    (at / PAGE..at.saturating_add(len).div_ceil(PAGE)).map(|page| (page, written_set(page, now), now)).collect()
}

/// (Neptune) Whether the kernel forgets the copy-back's writes of a landing together
/// (`VICTORY_QUIET_BACK_TOGETHER=0`: each as it is made, as before).
///
/// A call that resets written pages has every core drop what it knows of them, and the
/// caller waits for all of them: some 50 microseconds here, whatever the range (the walk
/// of a clean page is 4 nanoseconds). The copy-back made such a call for each of the 650
/// pieces it wrote in a frame. Now the ranges are kept, and when the landing's merges are
/// done one asking takes each run of them that lie near one another: what it finds outside
/// our own ranges the title wrote, and the journal is told so.
fn ours_together() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_QUIET_BACK_TOGETHER").is_none_or(|v| v != "0"))
}

/// Ranges of whole 4 KiB pages the copy-back wrote and the kernel has not forgotten yet.
static OURS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// The kernel forgets the writes [`ours_after`] kept ([`ours_together`]). Called when a
/// landing's merges are done.
pub(crate) fn ours_flush() {
    use windows_sys::Win32::System::Memory::{GetWriteWatch, ResetWriteWatch};
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    // Ranges up to 32 MiB apart are one asking, of 1 GiB at most.
    const GAP: usize = 32 << 20;
    const MOST: usize = 1 << 30;
    let mut ours = std::mem::take(&mut *OURS.lock().unwrap_or_else(|e| e.into_inner()));
    if ours.is_empty() {
        return;
    }
    ours.sort_unstable();
    thread_local! {
        static FOUND: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    FOUND.with(|found| {
        let mut found = found.borrow_mut();
        let mut i = 0;
        while i < ours.len() {
            let first = ours[i].0;
            let mut last = ours[i].1;
            let mut k = i + 1;
            while k < ours.len() && ours[k].0 <= last + GAP && ours[k].1.max(last) - first <= MOST {
                last = last.max(ours[k].1);
                k += 1;
            }
            let run = &ours[i..k];
            let mut count = (last - first) / KERNEL_PAGE;
            if found.len() < count {
                found.resize(count, 0);
            }
            let mut granularity = 0;
            // SAFETY: the answer buffer holds `count` entries.
            let failed = unsafe {
                GetWriteWatch(WRITE_WATCH_FLAG_RESET, first as *const _, last - first, found.as_mut_ptr().cast(), &mut count, &mut granularity)
            } != 0;
            COUNTS[5].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if failed {
                // Not all one reservation: each on its own, as before.
                for &(a, b) in run {
                    // SAFETY: a range the kernel answered for before the write.
                    unsafe { ResetWriteWatch(a as *const _, b - a) };
                }
            } else {
                // What lies outside our own ranges the title wrote.
                let mut now = 0;
                for &at in &found[..count] {
                    let before = run.partition_point(|r| r.0 <= at);
                    // The ranges are in order of their starts and short: the few before
                    // `at` are looked at.
                    let ours_too = run[before.saturating_sub(8)..before].iter().any(|r| at >= r.0 && at < r.1);
                    if !ours_too {
                        if now == 0 {
                            now = next_asking();
                        }
                        written_set(at / PAGE, now);
                    }
                }
            }
            i = k;
        }
    });
}

/// (Neptune) Whether view memory `[at, at + len)` is all guarded and clean: nothing was written
/// there since a copy last took it ([`quiet_back`]).
pub(crate) fn guarded_clean(at: usize, len: usize) -> bool {
    let (first, last) = (at / BLOCK, at.saturating_add(len).div_ceil(BLOCK));
    first < last && clean_all(first, last)
}

/// After our own write of view memory `[at, at + len)` through a view of our own
/// (`crate::inplace::host_alias`), which no guard saw: the journal takes its pages as
/// written now, at every title view that shows them. Per journal page of the range: the
/// asking that had last found it written (0: none) and this one.
pub(crate) fn ours_through(at: usize, len: usize) -> Vec<(usize, u64, u64)> {
    let now = next_asking();
    let mut marks = Vec::new();
    for page in at / PAGE..at.saturating_add(len).div_ceil(PAGE) {
        marks.push((page, written_set(page, now), now));
        for alias in crate::inplace::pages_of(page * PAGE) {
            if alias / PAGE != page {
                written_set(alias / PAGE, now);
            }
        }
    }
    marks
}

/// The submission's one asking stands no longer: its refreshes are done,
/// or title memory was written in a way the journal was not told.
pub(crate) fn ask_once_end() {
    let mut j = JOURNAL.write().unwrap_or_else(|e| e.into_inner());
    if j.kept {
        j.kept = false;
        j.ahead.clear();
    }
}

/// We wrote title memory `[at, at + len)` while a submission's one asking
/// stands (the title's buffer copies, done at submission): its pages count
/// as written after that asking, so the refreshes that follow copy them.
pub(crate) fn wrote(at: usize, len: usize) {
    if len == 0 || !JOURNAL.read().unwrap_or_else(|e| e.into_inner()).kept {
        return;
    }
    let now = next_asking();
    // The asking that stands moves with it: `asked` and `writes` give it as
    // "now", and a page noted written after "now" stays stale for every
    // copy that takes it (copied at `now`, written at `now + 1`): copied
    // again at every refresh, and a reason to settle flights
    // (fxceless/victory-fork#410).
    JOURNAL.write().unwrap_or_else(|e| e.into_inner()).ahead_asked = now;
    for page in at / PAGE..at.saturating_add(len).div_ceil(PAGE) {
        written_set(page, now);
    }
}

/// Bytes [`written_through_view`] was told of since the last ram line.
pub(crate) static NOTIFIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `VICTORY_DS_NOTIFY=1` (default off): the runtime's writes into physical pages through a view
/// of its own are told to the journal at the title's addresses that show them.
pub fn notify_view_writes() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_DS_NOTIFY").is_some_and(|v| v == "1"))
}

/// The runtime wrote the physical pages shown at title memory `[at, at + len)` through a view of
/// its own (DirectStorage, `write_physical`): the pages count as written now, so host copies
/// and textures taken before refresh. Unlike [`wrote`] with no asking standing too: nothing
/// else (no guard fault, no kernel watch of the title's view) sees such a write.
pub fn written_through_view(at: usize, len: usize) {
    if len == 0 {
        return;
    }
    let now = next_asking();
    for page in at / PAGE..at.saturating_add(len).div_ceil(PAGE) {
        written_set(page, now);
    }
    NOTIFIED.fetch_add(len as u64, std::sync::atomic::Ordering::Relaxed);
}

/// One kernel asking for `pieces` (page ranges in order); by halves if it
/// fails, as a run across two reservations or into memory that is not
/// write-watched does. A piece alone is left to [`writes`].
fn ask_run(j: &mut Journal, pieces: &[(usize, usize)]) {
    if pieces.len() < 2 {
        return;
    }
    let first = pieces[0].0;
    let last = pieces.iter().map(|p| p.1).max().unwrap_or(first);
    if ask_kernel(j, first, last) {
        j.ahead.push((first, last));
        return;
    }
    let (a, b) = pieces.split_at(pieces.len() / 2);
    ask_run(j, a);
    ask_run(j, b);
}

/// Asks the kernel for the written pages of pages `[first, last)` and notes
/// them in the journal under the ahead asking; false (and nothing changed)
/// if the range is not write-watched memory of one reservation.
fn ask_kernel(j: &mut Journal, first: usize, last: usize) -> bool {
    ask_kernel_found(j, first, last).is_some()
}

/// [`ask_kernel`], saying how many written 4 KiB pages the kernel reported (they are the
/// first of `j.found`); `None` where it says false.
fn ask_kernel_found(j: &mut Journal, first: usize, last: usize) -> Option<usize> {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    let mut count = (last - first) * PAGE / KERNEL_PAGE;
    j.found.resize(count.max(j.found.len()), 0);
    let mut granularity = 0;
    #[cfg(feature = "diagnostics")]
    let started = std::time::Instant::now();
    // (Neptune) View memory the guards already hold cannot be write-watched, and a range that
    // starts in it fails as a whole: no asking (fxceless/victory-fork 9376fd6).
    // SAFETY: the answer buffer holds `count` entries.
    let failed = known_view(first * PAGE)
        || unsafe {
            GetWriteWatch(
                WRITE_WATCH_FLAG_RESET,
                (first * PAGE) as *const _,
                (last - first) * PAGE,
                j.found.as_mut_ptr().cast(),
                &mut count,
                &mut granularity,
            )
        } != 0;
    if failed {
        return None;
    }
    #[cfg(feature = "diagnostics")]
    {
        use crate::copystat::{C, add};
        add(C::Watches, 1);
        add(C::WatchUs, started.elapsed().as_micros() as u64);
        slow(started.elapsed().as_micros() as u64);
    }
    COUNTS[5].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    COUNTS[6].fetch_add(((last - first) * PAGE / KERNEL_PAGE) as u64, std::sync::atomic::Ordering::Relaxed);
    COUNTS[7].fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
    let now = j.ahead_asked;
    for k in 0..count {
        // Noted with a later asking meanwhile (the copy-back's own write) it gets a number
        // that says the kernel's find is newer (`found_set`).
        found_set(j.found[k], now);
    }
    Some(count)
}

/// What [`writes`] would say of `[start, end)` from the journal alone, if
/// the range lies in one asked ahead ([`ask_ahead`], [`ask_once`]): the
/// pages written since tracking began and the asking that last found each
/// written, with the number of the asking that stands (as [`writes`] gives
/// it). `None` if the kernel would have to be asked.
pub(crate) fn asked(start: usize, end: usize) -> Option<(u64, Vec<(usize, u64)>)> {
    let (first, last) = (start / PAGE, end.div_ceil(PAGE));
    let standing = {
        let j = JOURNAL.read().unwrap_or_else(|e| e.into_inner());
        j.ahead.iter().any(|&(a, b)| a <= first && last <= b).then_some(j.ahead_asked)
    };
    standing.map(|asked| (asked, written_in(first, last)))
}

/// The pages of `[start, end)` (whole pages) as last written, for pages
/// written since tracking began: `(page number, asking)` in page order, and
/// this asking's number, which is higher than any in the list but those the
/// kernel just reported. `None` if the memory is not write-watched (nor,
/// with `guarded`, view memory, which is then guarded).
pub(crate) fn writes(start: usize, end: usize, guarded: bool) -> Option<(u64, Vec<(usize, u64)>)> {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    let (first, last) = (start / PAGE, end.div_ceil(PAGE));
    // Asked ahead with the submission's other ranges: the journal has it. (Neptune) The lock
    // is held for the look alone, and the kernel is asked without it: the pages are
    // noted in a table of their own (`written_set`).
    let standing = {
        let j = JOURNAL.read().unwrap_or_else(|e| e.into_inner());
        j.ahead.iter().any(|&(a, b)| a <= first && last <= b).then_some(j.ahead_asked)
    };
    if let Some(asked) = standing {
        return Some((asked, written_in(first, last)));
    }
    thread_local! {
        /// Room for the kernel's answer.
        static FOUND: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    FOUND.with(|found| {
        let mut found = found.borrow_mut();
        let mut count = (last - first) * PAGE / KERNEL_PAGE;
        if found.len() < count {
            found.resize(count, 0);
        }
        let mut granularity = 0;
        #[cfg(feature = "diagnostics")]
        let started = std::time::Instant::now();
        // SAFETY: the answer buffer holds `count` entries.
        let failed = unsafe {
            GetWriteWatch(
                WRITE_WATCH_FLAG_RESET,
                (first * PAGE) as *const _,
                (last - first) * PAGE,
                found.as_mut_ptr().cast(),
                &mut count,
                &mut granularity,
            )
        } != 0;
        let guarded = if failed {
            count = 0;
            if !guarded {
                return None;
            }
            #[cfg(feature = "diagnostics")]
            let guarding = std::time::Instant::now();
            let written = guard(first * PAGE, last * PAGE);
            #[cfg(feature = "diagnostics")]
            crate::copystat::add(crate::copystat::C::GuardUs, guarding.elapsed().as_micros() as u64);
            written?
        } else {
            Vec::new()
        };
        #[cfg(feature = "diagnostics")]
        {
            use crate::copystat::{C, add};
            add(C::Watches, 1);
            add(C::WatchUs, started.elapsed().as_micros() as u64);
            slow(started.elapsed().as_micros() as u64);
        }
        let now = next_asking();
        for &at in &found[..count] {
            found_set(at, now);
        }
        for page in guarded {
            written_set(page, now);
        }
        Some((now, written_in(first, last)))
    })
}

impl Tracked {
    /// Takes changed pages of `[from, to)`, cut to the tracked range. A
    /// partial page stays changed so a later take can read its other bytes.
    pub(crate) fn take(&self, from: usize, to: usize) -> Vec<(usize, usize)> {
        let (from, to) = (from.max(self.start), to.min(self.end));
        if from >= to {
            return Vec::new();
        }
        let Some((_, pages)) = writes(from, to, true) else {
            return Vec::new();
        };
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for (page, write) in pages {
            if seen.get(&page).is_some_and(|&last| last >= write) {
                continue;
            }
            let (start, end) = ((page * PAGE).max(from), ((page + 1) * PAGE).min(to));
            if start >= end {
                continue;
            }
            // Keep partial pages dirty: the caller has not taken the rest.
            if (page * PAGE).max(self.start) >= from
                && ((page + 1) * PAGE).min(self.end) <= to
            {
                seen.insert(page, write);
            }
            match ranges.last_mut() {
                Some((_, last)) if *last == start => *last = end,
                _ => ranges.push((start, end)),
            }
        }
        ranges
    }
}

/// Guarded blocks of view memory.
const BLOCK: usize = PAGE;

/// Whether the block holding `start` is view memory the guards already hold (read-only by our
/// hand, or noted absent): asking the kernel's write watch about it can only fail.
/// `VICTORY_VIEW_ASK=1` (or `VICTORY_GUARD_ASK=1`) asks anyway, as before.
fn known_view(start: usize) -> bool {
    static ASK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let ask = *ASK.get_or_init(|| {
        ["VICTORY_VIEW_ASK", "VICTORY_GUARD_ASK"]
            .iter()
            .any(|n| std::env::var_os(n).is_some_and(|v| v == "1"))
    });
    if ask {
        return false;
    }
    let g = guards_read();
    let block = start / BLOCK;
    g.protected.contains(&block) || g.absent.contains(&block)
}

struct Guards {
    /// Blocks we made read-only.
    protected: BTreeSet<usize>,
    /// Of those, the ones not written since a copy took them.
    clean: BTreeSet<usize>,
    /// Blocks already copied as zeros. While they remain wholly uncommitted,
    /// the copy can skip hundreds of MB of failing kernel reads per world frame.
    absent: BTreeSet<usize>,
    /// Blocks whose physical page was guarded together with its other views
    /// (those views' blocks are in it too): a write to one opens them all.
    /// A write range can be huge (`fread`), so only these are looked up.
    aliased: BTreeSet<usize>,
}

// (Neptune) Read by many at once: a refresh of view memory asks whether its blocks are all
// clean, which nearly all are, and the refreshes of a submission run on several threads;
// behind one lock they stood in line with one another and with the thread that copies back
// (a sixth of the submitting thread's time in a sample).
static GUARDS: std::sync::RwLock<Guards> = std::sync::RwLock::new(Guards {
    protected: BTreeSet::new(),
    clean: BTreeSet::new(),
    absent: BTreeSet::new(),
    aliased: BTreeSet::new(),
});

fn guards() -> std::sync::RwLockWriteGuard<'static, Guards> {
    GUARDS.write().unwrap_or_else(|e| e.into_inner())
}

/// The guards, to look at only.
fn guards_read() -> std::sync::RwLockReadGuard<'static, Guards> {
    GUARDS.read().unwrap_or_else(|e| e.into_inner())
}

/// For view memory: the pages of `[start, end)`'s blocks written since
/// they were last guarded (all of a block's pages), guarding them again
/// before the caller copies them. `None` if the memory is not a view.
fn guard(start: usize, end: usize) -> Option<Vec<usize>> {
    // A range whose first block the guards hold already (read-only by our
    // hand, or noted absent) is a view: no question to Windows. The
    // question walks the page tables to the end of the pages that share the
    // first one's state, and a guarded view is one long run of read-only
    // pages: 0.6 ms for the title's 174 MB heap, 16 times a frame, and most
    // of the 68 ms a frame of "guards" (run 58).
    // `VICTORY_GUARD_ASK=1` asks every time, as before.
    static ASK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let ask = *ASK.get_or_init(|| std::env::var_os("VICTORY_GUARD_ASK").is_some_and(|v| v == "1"));
    let (first, last) = (start / BLOCK, end.div_ceil(BLOCK));
    if !ask && first < last && clean_all(first, last) {
        // (Neptune) All clean, so all guarded and a view: said without the lock.
        count(4, 1);
        return Some(Vec::new());
    }
    let hot = guard_hot();
    let frame = crate::queue::presented();
    if !ask && first < last {
        // (Neptune) Clean blocks, blocks left open and blocks that are still not committed,
        // nothing else: said without the lock too, the open ones as written.
        let mut written = Vec::new();
        let mut settled = true;
        let mut uncommitted_until = first;
        for block in first..last {
            if clean_all(block, block + 1) {
                continue;
            }
            if hot > 0 && left_open(block, frame, hot) {
                written.extend(block * BLOCK / PAGE..(block + 1) * BLOCK / PAGE);
                continue;
            }
            if absent_one(block) {
                // As the check under the lock asks: one question for the blocks that
                // follow in the same region.
                if block >= uncommitted_until {
                    uncommitted_until = uncommitted_end(block);
                }
                if block < uncommitted_until {
                    continue;
                }
            }
            settled = false;
            break;
        }
        if settled {
            count(4, 1);
            return Some(written);
        }
    }
    let (known, all_clean) = {
        let g = guards_read();
        (
            !ask && (g.protected.contains(&first) || g.absent.contains(&first) || (hot > 0 && left_open(first, frame, hot))),
            g.clean.range(first..last).count() == last - first,
        )
    };
    if known && all_clean {
        // (Neptune) Nothing written, mapped or new since the range was last taken: said
        // beside the other readers.
        return Some(Vec::new());
    }
    if !known && !is_view(start, end) {
        return None;
    }
    install();
    count(3, 1);
    let mut g = guards();
    let mut written = Vec::new();
    let mut run: Option<usize> = None;
    // The range's clean blocks, in order: read off the set in one pass,
    // not looked up one block at a time. A frame asks about some 10 GB of
    // view memory, 150,000 blocks, nearly all clean: the lookups were most
    // of the 60-90 ms a frame of "guards" (run 54). What the loop adds to
    // the set meanwhile is the block it stands at or ones before it.
    let clean: Vec<usize> = g.clean.range(first..last).copied().collect();
    if clean.len() == last - first {
        // Nothing written, mapped or new since the range was last taken.
        return Some(written);
    }
    let mut next_clean = 0;
    let mut uncommitted_until = first;
    for block in first..=last {
        while next_clean < clean.len() && clean[next_clean] < block {
            next_clean += 1;
        }
        let is_clean = next_clean < clean.len() && clean[next_clean] == block;
        let mut open = block < last && !is_clean;
        if open && g.absent.contains(&block) {
            // Ordinary commits need no runtime mapping notification. Reuse
            // this asking's query for the following blocks in the same region.
            if block >= uncommitted_until {
                uncommitted_until = uncommitted_end(block);
            }
            open = block >= uncommitted_until;
            if open {
                g.absent.remove(&block);
                absent_set(block, false);
            }
        }
        if open {
            written.extend(block * BLOCK / PAGE..(block + 1) * BLOCK / PAGE);
            if g.protected.contains(&block) {
                // Written through the runtime, still read-only.
                g.clean.insert(block);
                clean_set(block, true);
            } else if hot > 0 && stays_open(&g, block, frame, hot) {
                // (Neptune) Left open (`guard_hot`): the run of blocks to guard ends before it.
            } else {
                run.get_or_insert(block);
                continue;
            }
        }
        if let Some(from) = run.take() {
            protect(&mut g, from, block);
        }
    }
    Some(written)
}

/// Whether `[start, end)` is view memory. The title's views lie in
/// placeholders, mapped and unmapped block by block as it streams, so a
/// range can begin in a block that is not mapped right now: the first
/// pages of the range that are there decide. A range with nothing there
/// counts as a view (its blocks are noted absent, and looked at again when
/// the runtime maps them).
///
/// Looking at the first address only took such a range for "not a view":
/// its copy was marked not watched for good and copied whole at every
/// refresh, around its unmapped pages (run 54: 300 such ranges, 570 MB
/// copied a frame for them, 255 copies a frame reading around holes).
/// `VICTORY_GUARD_HOLES=0` looks at the first address only, as before.
fn is_view(start: usize, end: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_MAPPED, MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    static HOLES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let holes = *HOLES.get_or_init(|| std::env::var_os("VICTORY_GUARD_HOLES").is_none_or(|v| v != "0"));
    let mut at = start;
    loop {
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer of the right size.
        let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
        if n == 0 {
            return false;
        }
        if info.Type == MEM_MAPPED {
            return true;
        }
        // Committed memory that is no view (the title's heaps, ours), or
        // the answer as it was before.
        if info.State == MEM_COMMIT || !holes {
            return false;
        }
        // A hole (a placeholder, or free): on to what follows it.
        let next = (info.BaseAddress as usize).saturating_add(info.RegionSize);
        if next <= at || next >= end {
            return true;
        }
        at = next;
    }
}

/// Makes blocks `[from, to)` read-only and notes them guarded and clean;
/// block by block if they span views, leaving out any that fail, and
/// noting those not committed (unmapped) absent. The same physical pages
/// at every other view of them are made read-only too: a write through
/// one of those is this memory written, and only a fault can say so.
/// `VICTORY_VIEW_GUARD=0` leaves the other views as they are, as before.
fn protect(g: &mut Guards, from: usize, to: usize) {
    protect_here(g, from, to);
    if !view_guard() {
        return;
    }
    for (base, bytes) in crate::inplace::views_between(from.saturating_mul(BLOCK), to.saturating_mul(BLOCK)) {
        let first = (base / BLOCK).max(from);
        let last = (base.saturating_add(bytes).div_ceil(BLOCK)).min(to);
        for block in first..last {
            let shown = crate::inplace::pages_of(block * BLOCK);
            // One view (nothing else to guard), or the page the title shows
            // wherever it has nothing (thousands of views): each view of
            // that one is guarded by itself.
            if shown.len() < 2 || shown.len() > MANY_VIEWS {
                continue;
            }
            #[cfg(feature = "diagnostics")]
            ALIASES[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            for alias in shown {
                let alias = alias / BLOCK;
                g.aliased.insert(alias);
                if alias != block {
                    protect_alias(g, alias);
                }
            }
        }
    }
}

/// Views of one page past which its views are not guarded together.
pub(crate) const MANY_VIEWS: usize = 64;

/// Whether all the views of a guarded page are guarded together
/// (`VICTORY_VIEW_GUARD`).
fn view_guard() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_VIEW_GUARD").is_none_or(|v| v != "0"))
}

/// Diagnostics: blocks guarded together with their other views, blocks
/// opened because another view of theirs was written, and blocks opened
/// because a view of theirs was mapped; since the start.
#[cfg(feature = "diagnostics")]
pub(crate) static ALIASES: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

/// Makes `block` read-only: another view of pages a copy has just taken
/// through its own view, so that a write through this one faults too. It is
/// not noted clean. The copies of title memory at this view have not taken
/// the pages: noted clean, the block would not be reported written at their
/// next check and they would keep the bytes from before the write (`guard`
/// reports a block that is read-only and not clean as written, then notes it
/// clean). A block that is clean stays so: had the pages been written since
/// its copies took them, `open` would have opened it.
fn protect_alias(g: &mut Guards, block: usize) {
    use windows_sys::Win32::System::Memory::{PAGE_READONLY, VirtualProtect};
    if g.protected.contains(&block) {
        return;
    }
    let mut old = 0;
    // SAFETY: title view memory; only its protection changes.
    if unsafe { VirtualProtect((block * BLOCK) as *const _, BLOCK, PAGE_READONLY, &mut old) } != 0 {
        g.protected.insert(block);
    }
}

/// [`protect`] for one range, without the other views of its pages.
fn protect_here(g: &mut Guards, from: usize, to: usize) {
    use windows_sys::Win32::System::Memory::{PAGE_READONLY, VirtualProtect};
    let set = |from: usize, to: usize| {
        let mut old = 0;
        // SAFETY: title view memory; only its protection changes.
        unsafe {
            VirtualProtect(
                (from * BLOCK) as *const _,
                (to - from) * BLOCK,
                PAGE_READONLY,
                &mut old,
            ) != 0
        }
    };
    let note = |g: &mut Guards, from: usize, to: usize| {
        g.protected.extend(from..to);
        g.clean.extend(from..to);
        (from..to).for_each(|block| clean_set(block, true));
    };
    count(1, 1);
    count(2, (to - from) as u64);
    if set(from, to) {
        note(g, from, to);
        return;
    }
    for block in from..to {
        if set(block, block + 1) {
            note(g, block, block + 1);
        } else if block < uncommitted_end(block) {
            g.absent.insert(block);
            absent_set(block, true);
        }
    }
}

/// The end (exclusive) of the whole uncommitted blocks starting at `block`.
fn uncommitted_end(block: usize) -> usize {
    use windows_sys::Win32::System::Memory::{
        MEM_FREE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery((block * BLOCK) as *const _, &mut info, size_of_val(&info)) };
    if n != 0 && matches!(info.State, MEM_RESERVE | MEM_FREE) {
        (info.BaseAddress as usize).saturating_add(info.RegionSize) / BLOCK
    } else {
        block
    }
}

/// Gives write access back to the guarded blocks of `[start, end)` and
/// notes them written; true if any was guarded. The same physical pages at
/// every other view of them are opened too: the write that faulted (or is
/// ours, `crate::host::copy_mapped`) goes through one view, and every
/// guard over the pages must copy them again.
fn open(g: &mut Guards, start: usize, end: usize) -> bool {
    let any = open_here(g, start, end);
    if start >= end || g.aliased.is_empty() {
        return any;
    }
    let aliased: Vec<usize> = g.aliased.range(start / BLOCK..end.div_ceil(BLOCK)).copied().collect();
    for block in aliased {
        g.aliased.remove(&block);
        for alias in crate::inplace::pages_of(block * BLOCK) {
            if alias / BLOCK != block && open_here(g, alias, alias.saturating_add(BLOCK)) {
                #[cfg(feature = "diagnostics")]
                ALIASES[1].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            g.aliased.remove(&(alias / BLOCK));
        }
    }
    any
}

/// [`open`] for one range, without the other views of its pages.
fn open_here(g: &mut Guards, start: usize, end: usize) -> bool {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    let blocks: Vec<usize> = g
        .protected
        .range(start / BLOCK..end.div_ceil(BLOCK))
        .copied()
        .collect();
    let mut run: Option<(usize, usize)> = None;
    let give = |run: Option<(usize, usize)>| {
        if let Some((first, n)) = run {
            let mut old = 0;
            // SAFETY: blocks we made read-only, one after the other. (One call for the run;
            // if it is refused, block by block as before.)
            if unsafe { VirtualProtect((first * BLOCK) as *const _, n * BLOCK, PAGE_READWRITE, &mut old) } == 0 {
                for block in first..first + n {
                    // SAFETY: a block we made read-only.
                    unsafe { VirtualProtect((block * BLOCK) as *const _, BLOCK, PAGE_READWRITE, &mut old) };
                }
            }
        }
    };
    for &block in &blocks {
        g.protected.remove(&block);
        g.clean.remove(&block);
        clean_set(block, false);
        run = match run {
            Some((first, n)) if first + n == block => Some((first, n + 1)),
            other => {
                give(other);
                Some((block, 1))
            }
        };
    }
    give(run);
    !blocks.is_empty()
}

/// Before our own write into title memory `[at, at + len)` through the
/// kernel (which does not fault into our handler): opens its guarded blocks.
/// The runtime calls it (as `kernel_will_write`) before the title's kernel
/// reads into its own memory.
pub fn writing(at: usize, len: usize) {
    // The length is the title's (`fread`'s size times count, saturated).
    let end = at.saturating_add(len);
    // (Neptune) Nothing guarded there (most of the copy-back's writes): looked at beside the
    // other readers, and left at that.
    let guarded = {
        let g = guards_read();
        let blocks = at / BLOCK..end.div_ceil(BLOCK);
        g.protected.range(blocks.clone()).next().is_some() || g.aliased.range(blocks).next().is_some()
    };
    if guarded {
        open(&mut guards(), at, end);
    }
    // A placed texture there changes with no descriptor written.
    crate::descriptor::host::remapped(at, len);
}

/// What `VICTORY_PAGE_WRITES` asks to be done with a write the guards do
/// not see once its bytes are there ([`title_memory_written`]), as bits:
/// 1, the guarded blocks under it are opened and count as written (host
/// copies of buffers and textures read through tables take them again);
/// 2, the placed textures picked by index that lie under it are due again;
/// 4, both also for reads to plain addresses when they have finished.
/// Unset or `guards`: bit 1 alone. `0`: nothing, counted only (the default
/// until run 199). `1`: bits 1 and 2. `slots`: bit 2 alone. `all`:
/// everything.
/// (Bit 1 by default: in every run 10,000 to 50,000 guarded blocks of host
/// copies lay under such writes and kept the bytes from before; runs 197
/// and 198 opened them, 9,766 and 10,796 blocks, with nothing worse for
/// it. Bit 2 makes texture slots due and is the one that flooded the
/// queue in an earlier form: still asked for by name.)
pub(crate) fn page_writes() -> u8 {
    static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_PAGE_WRITES").as_deref().map(str::trim) {
        Ok("0") => 0,
        Ok("1") => 3,
        Ok("slots") => 2,
        Ok("all") => 7,
        _ => 1,
    })
}

/// Since the start: writes into physical pages through the runtime's own
/// views that landed at addresses the title has mapped (one for each run
/// of addresses); their bytes; guarded blocks that lay under them (host
/// copies that would otherwise keep the bytes from before); reads to
/// plain addresses that finished.
pub(crate) static WRITTEN: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// The bytes of a write the guards do not see are there: title memory
/// `[at, at + len)`. `pages`: DirectStorage wrote them into physical pages
/// through a view of the runtime's own, and this is an address where the
/// title has those pages mapped (no title address faulted, nothing was
/// mapped or unmapped; the shortcut this file's header names). Otherwise a
/// read to a plain address has finished, of which [`writing`] was told
/// before it began.
///
/// Upstream's c75fdba does the same for its trackers (its `Writing` marks
/// the pages once more when dropped; page destinations are reported at
/// each address showing them). Here it is counted always and acted on as
/// [`page_writes`] says. Called without the runtime's page-table lock;
/// takes the guards' lock, then, alone, the lock of the ranges kept for
/// the texture pass.
pub fn title_memory_written(at: usize, len: usize, pages: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    if len == 0 {
        return;
    }
    let mode = page_writes();
    let end = at.saturating_add(len);
    if pages {
        WRITTEN[0].fetch_add(1, Relaxed);
        WRITTEN[1].fetch_add(len as u64, Relaxed);
    } else {
        WRITTEN[3].fetch_add(1, Relaxed);
        if mode & 4 == 0 {
            return;
        }
    }
    {
        let mut g = guards();
        let guarded = g.protected.range(at / BLOCK..end.div_ceil(BLOCK)).count() as u64;
        if pages {
            WRITTEN[2].fetch_add(guarded, Relaxed);
        }
        if mode & 1 != 0 && guarded != 0 {
            open(&mut g, at, end);
        }
    }
    // A placed texture there changed with no descriptor written.
    crate::descriptor::host::page_written(at, len);
}

/// The runtime mapped a view of the physical bytes `[offset, offset + len)`
/// of `section` (`crate::inplace::title_memory_view`): the view is
/// writable, so the same bytes guarded at another view must be checked
/// again — and are, the pages noted written, which also has the next check
/// guard the new view (`protect`).
pub(crate) fn views_mapped(section: usize, offset: u64, len: usize) {
    if !view_guard() {
        return;
    }
    let ranges = crate::inplace::views_of_section(section, offset, len);
    if ranges.is_empty() {
        return;
    }
    let mut g = guards();
    for &(at, n) in &ranges {
        if open_here(&mut g, at, at.saturating_add(n)) {
            #[cfg(feature = "diagnostics")]
            ALIASES[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Held shared by a read of title memory in place, and alone by the runtime
/// while it changes what title memory shows.
///
/// The runtime does not change a mapping in one step. To unmap one page of
/// a view it unmaps the whole view and maps what stays again
/// (`xmem.rs` `unmap`), and a view it maps is a reservation until the next
/// call commits it (`map`, then `commit`). For that moment pages the title
/// never gave up are "reserved, never committed", and a thread reading
/// them in place takes an access violation: run 71 in `Layout::detile`,
/// run 74 in the hash before it, both under `Staging::refresh_now`, which
/// had asked whether the texture was all mapped and been told yes. Asking
/// cannot help; not changing the mapping while such a read is under way
/// does.
///
/// One lock per 16 MB of addresses (taken modulo [`SHARDS`]), not one for
/// all title memory: a read holds the locks of the addresses it reads, a
/// change those of the addresses it changes. With one lock, every texture
/// refresh waited for every remap anywhere in title memory, and the remaps
/// are kernel calls of 0.1-0.8 ms each, some 1,000-2,700 a second while the
/// world streams in (run mem-W3a): up to 1.1 s of a frame's texture
/// preparation was waiting for it (mem-W0). Several locks are always taken
/// in ascending order, by readers and changes alike, so none waits on
/// another in a circle.
static MAPPING: [std::sync::RwLock<()>; SHARDS] = [const { std::sync::RwLock::new(()) }; SHARDS];

/// Locks in [`MAPPING`], and the bits of address each covers (16 MB).
const SHARDS: usize = 256;
const SHARD_BITS: u32 = 24;

/// The locks of [`MAPPING`] covering `[at, at + len)`, ascending; every
/// one for `len` 0 (the whole of title memory) or a range of [`SHARDS`]
/// spans or more.
fn shards(at: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return (0..SHARDS).collect();
    }
    let first = at >> SHARD_BITS;
    let last = at.saturating_add(len - 1) >> SHARD_BITS;
    if last - first + 1 >= SHARDS {
        return (0..SHARDS).collect();
    }
    let mut all: Vec<usize> = (first..=last).map(|s| s % SHARDS).collect();
    all.sort_unstable();
    all
}

/// What the runtime holds while it changes what title memory shows
/// ([`title_memory_remapping`]).
pub struct Remapping {
    _guards: Vec<std::sync::RwLockWriteGuard<'static, ()>>,
    /// When it was had, how long it was waited for (µs), and where.
    #[cfg(feature = "diagnostics")]
    taken: (std::time::Instant, u64, &'static std::panic::Location<'static>),
}

/// The runtime is about to map, unmap or move physical pages of title
/// memory: until what it returns is dropped, no read of title memory in
/// place is under way and none begins ([`title_memory_reading`]). Taken
/// around the whole change, the unmapping and the mapping again. All of
/// title memory; [`title_memory_remapping_range`] for part of it.
#[track_caller]
pub fn title_memory_remapping() -> Remapping {
    title_memory_remapping_range(0, 0)
}

/// [`title_memory_remapping`] for `[at, at + len)` only: every address
/// whose mapping the change touches, the parts of views it takes away and
/// maps again included. Reads elsewhere go on.
#[track_caller]
pub fn title_memory_remapping_range(at: usize, len: usize) -> Remapping {
    #[cfg(feature = "diagnostics")]
    let asked = std::time::Instant::now();
    let guards = shards(at, len)
        .into_iter()
        .map(|s| MAPPING[s].write().unwrap_or_else(|e| e.into_inner()))
        .collect();
    Remapping {
        _guards: guards,
        #[cfg(feature = "diagnostics")]
        taken: (
            std::time::Instant::now(),
            asked.elapsed().as_micros() as u64,
            std::panic::Location::caller(),
        ),
    }
}

/// Diagnostics: the changes of the mapping by where the runtime made them since the
/// last asking: file, line, and [changes, µs waited for in-place reads to end, µs held,
/// longest µs held]. The texture refreshes wait on these ("waiting for title memory to
/// hold still").
#[cfg(feature = "diagnostics")]
static REMAPS: std::sync::Mutex<Vec<(&'static str, u32, [u64; 4])>> = std::sync::Mutex::new(Vec::new());

#[cfg(feature = "diagnostics")]
impl Drop for Remapping {
    fn drop(&mut self) {
        let (at, waited, place) = self.taken;
        let held = at.elapsed().as_micros() as u64;
        let mut all = REMAPS.lock().unwrap_or_else(|e| e.into_inner());
        let i = match all.iter().position(|r| r.0 == place.file() && r.1 == place.line()) {
            Some(i) => i,
            None => {
                all.push((place.file(), place.line(), [0; 4]));
                all.len() - 1
            }
        };
        let k = &mut all[i].2;
        k[0] += 1;
        k[1] += waited;
        k[2] += held;
        k[3] = k[3].max(held);
    }
}

/// [`REMAPS`], taken (and reset).
#[cfg(feature = "diagnostics")]
pub(crate) fn remaps_taken() -> Vec<(&'static str, u32, [u64; 4])> {
    std::mem::take(&mut *REMAPS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// What a read of title memory in place holds ([`title_memory_reading`]).
pub struct Reading(#[allow(dead_code)] Vec<std::sync::RwLockReadGuard<'static, ()>>);

/// For reading title memory in place: until what it returns is dropped,
/// the runtime changes no mapping, so memory found mapped stays mapped.
/// Not to be taken twice by one thread, nor held across anything that
/// maps title memory. All of title memory; [`title_memory_reading_range`]
/// for part of it.
pub fn title_memory_reading() -> Reading {
    title_memory_reading_range(0, 0)
}

/// [`title_memory_reading`] for `[at, at + len)` only: changes of mapping
/// elsewhere go on meanwhile.
pub fn title_memory_reading_range(at: usize, len: usize) -> Reading {
    Reading(
        shards(at, len)
            .into_iter()
            .map(|s| MAPPING[s].read().unwrap_or_else(|e| e.into_inner()))
            .collect(),
    )
}

/// [`title_memory_reading_range`] if it can be had at once: `None` while
/// the runtime changes a mapping there (or waits to), for a caller that has
/// another way to read and would rather not wait.
pub fn title_memory_reading_now(at: usize, len: usize) -> Option<Reading> {
    let mut guards = Vec::new();
    for s in shards(at, len) {
        match MAPPING[s].try_read() {
            Ok(guard) => guards.push(guard),
            Err(std::sync::TryLockError::Poisoned(e)) => guards.push(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        }
    }
    Some(Reading(guards))
}

/// The runtime mapped or unmapped title memory `[at, at + len)`: what it
/// shows is new and read-write, so it is no longer guarded (nor known
/// unmapped).
pub fn title_memory_remapped(at: usize, len: usize) {
    // A placed texture there arrives, goes or changes with no descriptor
    // written.
    crate::descriptor::host::remapped(at, len);
    // The views the runtime told us of there are no longer what it shows.
    crate::inplace::views_gone(at, len);
    // And nothing stays in place over it.
    crate::inplace::title_memory_freeing(at, len);
    let mut g = guards();
    let range = at / BLOCK..at.saturating_add(len).div_ceil(BLOCK);
    if guard_hot() > 0 {
        for block in range.clone() {
            if hot_get(block) != 0 {
                hot_set(block, 0);
            }
        }
    }
    let absent: Vec<usize> = g.absent.range(range.clone()).copied().collect();
    for block in absent {
        g.absent.remove(&block);
        absent_set(block, false);
    }
    let blocks: Vec<usize> = g.protected.range(range).copied().collect();
    for block in blocks {
        g.protected.remove(&block);
        g.clean.remove(&block);
        clean_set(block, false);
    }
    let aliased: Vec<usize> = g.aliased.range(at / BLOCK..at.saturating_add(len).div_ceil(BLOCK)).copied().collect();
    for block in aliased {
        g.aliased.remove(&block);
    }
    let aliased: Vec<usize> = g.aliased.range(at / BLOCK..at.saturating_add(len).div_ceil(BLOCK)).copied().collect();
    for block in aliased {
        g.aliased.remove(&block);
    }
}

/// Registers the handler that takes writes to guarded blocks, once, ahead
/// of the crash reporter's.
fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
        EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_write(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        let r = unsafe { &*(*info).ExceptionRecord };
        const WRITE: usize = 1;
        if r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS
            || r.NumberParameters < 2
            || r.ExceptionInformation[0] != WRITE
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let at = r.ExceptionInformation[1];
        // Held across both checks: another thread may have opened the block
        // since this one faulted, and a copy may have guarded it again since
        // (then it is ours again, not read-only for some other reason).
        let mut g = guards();
        // (Neptune) `VICTORY_GUARD_OPEN_AHEAD=<blocks>` (1): the guarded blocks that follow
        // the one written are opened with it, so that a run of writes through a buffer is
        // one fault and not one for every 64 KiB (each of them takes this lock, and the
        // recording threads stood a tenth of their working time waiting for it).
        static AHEAD: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let ahead = *AHEAD.get_or_init(|| {
            std::env::var("VICTORY_GUARD_OPEN_AHEAD").ok().and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 64)
        });
        let block = at / BLOCK * BLOCK;
        if open(&mut g, at, at + 1) {
            if ahead > 1 {
                open(&mut g, block + BLOCK, block + ahead * BLOCK);
            }
            count(0, 1);
            #[cfg(feature = "diagnostics")]
            crate::copystat::add(crate::copystat::C::GuardFaults, 1);
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        if writable(at) {
            EXCEPTION_CONTINUE_EXECUTION
        } else {
            EXCEPTION_CONTINUE_SEARCH
        }
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: the handler only touches our guard state and protections.
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_write));
    });
}

/// Whether `at` is committed read-write memory now.
fn writable(at: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualQuery,
    };
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of_val(&info)) };
    n != 0 && info.State == MEM_COMMIT && info.Protect == PAGE_READWRITE
}

#[cfg(test)]
mod tests {
    #[test]
    fn sub_page_masks_follow_the_stamps() {
        let mut stamps = [0u64; 16];
        assert_eq!(mask_of(&stamps, 5), None);
        stamps[3] = 6;
        stamps[9] = 5;
        stamps[15] = 9;
        assert_eq!(mask_of(&stamps, 5), Some(1 << 3 | 1 << 15));
        assert_eq!(mask_of(&stamps, 9), None);
    }

    use super::*;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };
    const MEM_WRITE_WATCH: u32 = 0x20_0000;

    #[test]
    fn written_pages_are_reported_to_every_asker() {
        // SAFETY: a fresh write-watched allocation, freed at the end.
        let p = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                4 * PAGE,
                MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        } as usize;
        assert_ne!(p, 0);
        let (start, end) = (p, p + 4 * PAGE);
        let (_, before) = writes(start, end, false).expect("watched");
        assert!(before.is_empty());
        // SAFETY: committed pages of ours.
        unsafe { ((p + PAGE + 8) as *mut u8).write(1) };
        let (now, after) = writes(start, end, false).expect("watched");
        assert_eq!(after, [(p / PAGE + 1, now)]);
        // Another asker after the kernel's bit was cleared still sees it.
        let (later, again) = writes(p + PAGE, p + 2 * PAGE, false).expect("watched");
        assert!(later > now);
        assert_eq!(again, [(p / PAGE + 1, now)]);
        let mut plain = vec![0u8; PAGE];
        assert!(
            writes(
                plain.as_mut_ptr() as usize,
                plain.as_ptr() as usize + PAGE,
                true
            )
            .is_none()
        );
        // SAFETY: our allocation.
        unsafe { VirtualFree(p as *mut _, 0, MEM_RELEASE) };
    }

    /// A kernel read into a guarded view block fails; opened first, it
    /// lands and the block counts as written.
    #[test]
    fn kernel_reads_into_guarded_blocks_after_opening() {
        use std::io::{Read, Seek, Write};
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, UnmapViewOfFile,
        };
        // SAFETY: a pagefile section of two blocks, mapped read-write.
        let view = unsafe {
            let section = CreateFileMappingW(
                -1isize as _,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                (2 * BLOCK) as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, 2 * BLOCK).Value as usize
        };
        assert_ne!(view, 0);
        assert!(guard(view, view + 2 * BLOCK).is_some()); // now read-only
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"from the disk").unwrap();
        // `ReadFile` inside: the kernel writes the view.
        let read = |file: &mut std::fs::File| {
            file.rewind().unwrap();
            // SAFETY: 13 bytes of our view, nothing else refers to them.
            let into = unsafe { std::slice::from_raw_parts_mut((view + 100) as *mut u8, 13) };
            file.read(into).is_ok_and(|n| n == 13)
        };
        assert!(
            !read(&mut file),
            "the kernel does not fault into our handler"
        );
        writing(view + 100, 13);
        assert!(read(&mut file));
        // SAFETY: the bytes just read.
        let landed = unsafe { std::slice::from_raw_parts((view + 100) as *const u8, 13) };
        assert_eq!(landed, b"from the disk");
        assert_eq!(
            guard(view, view + 2 * BLOCK),
            Some((view / PAGE..view / PAGE + 1).collect())
        );
        // SAFETY: our view.
        unsafe {
            UnmapViewOfFile(
                windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: view as *mut _,
                },
            )
        };
        title_memory_remapped(view, 2 * BLOCK);
    }

    #[test]
    fn writes_in_partly_committed_view_blocks_are_reported() {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            SEC_RESERVE, UnmapViewOfFile,
        };

        const SIZE: usize = 3 * BLOCK;
        struct View(MEMORY_MAPPED_VIEW_ADDRESS);
        impl Drop for View {
            fn drop(&mut self) {
                title_memory_remapped(self.0.Value as usize, SIZE);
                // SAFETY: our view is still mapped.
                unsafe { UnmapViewOfFile(self.0) };
            }
        }
        // SAFETY: a new reserved pagefile section. The view holds its own
        // reference after we close the handle, including on assertion failure.
        let view = unsafe {
            let section = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE | SEC_RESERVE,
                0,
                SIZE as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            let view = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, SIZE);
            CloseHandle(section);
            assert!(!view.Value.is_null());
            View(view)
        };
        let start = view.0.Value as usize;
        let commit = |at: usize, len| {
            // SAFETY: these pages belong to our reserved view.
            let committed = unsafe { VirtualAlloc(at as _, len, MEM_COMMIT, PAGE_READWRITE) };
            assert_eq!(committed as usize, at);
        };
        let early = start + BLOCK + 3 * KERNEL_PAGE;
        let late = start + 2 * BLOCK + 3 * KERNEL_PAGE;
        commit(start, BLOCK);
        commit(early, KERNEL_PAGE);
        let refresh = || guard(start, start + SIZE).unwrap();
        assert_eq!(refresh(), [start / PAGE, early / PAGE, late / PAGE]);
        // The last block is still entirely reserved.
        assert!(!refresh().contains(&(late / PAGE)));

        commit(late, KERNEL_PAGE); // Ordinary commit, without a runtime mapping notification.
        for at in [early, late] {
            assert!(writable(at));
            // SAFETY: each address is inside its committed read-write page.
            unsafe { (at as *mut u8).write_volatile(1) };
        }
        assert_eq!(refresh(), [early / PAGE, late / PAGE]);
    }

    /// A range of view memory whose first block is not mapped is a view
    /// all the same: its blocks are reported once, then the ones written
    /// or mapped since.
    #[test]
    fn a_range_that_begins_in_an_unmapped_block_is_guarded() {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, MEM_PRESERVE_PLACEHOLDER, MEM_REPLACE_PLACEHOLDER,
            MEM_RESERVE_PLACEHOLDER, MapViewOfFile3, PAGE_NOACCESS, UnmapViewOfFile2,
            VirtualAlloc2,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: a fresh placeholder of three blocks and a pagefile
        // section of three; the middle block is mapped first.
        let (p, section) = unsafe {
            let p = VirtualAlloc2(
                GetCurrentProcess(),
                std::ptr::null(),
                3 * BLOCK,
                MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                PAGE_NOACCESS,
                std::ptr::null_mut(),
                0,
            ) as usize;
            assert_ne!(p, 0);
            let section = CreateFileMappingW(
                -1isize as _,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                (3 * BLOCK) as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            (p, section)
        };
        let map = |block: usize| {
            // SAFETY: splits the placeholder at the block and maps the
            // section's block there.
            let view = unsafe {
                VirtualFree(
                    (p + block * BLOCK) as *mut _,
                    BLOCK,
                    MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
                );
                MapViewOfFile3(
                    section,
                    GetCurrentProcess(),
                    (p + block * BLOCK) as *const _,
                    (block * BLOCK) as u64,
                    BLOCK,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(view.Value as usize, p + block * BLOCK);
            view
        };
        let pages = |block: usize| (p + block * BLOCK) / PAGE..(p + (block + 1) * BLOCK) / PAGE;
        let all: Vec<usize> = (0..3).flat_map(pages).collect();
        // Nothing mapped at all: a view with every block absent.
        assert!(is_view(p, p + 3 * BLOCK));
        let middle = map(1);
        title_memory_remapped(p + BLOCK, BLOCK);
        // The range begins in the placeholder, its middle block is a view.
        assert!(is_view(p, p + 3 * BLOCK));
        assert_eq!(guard(p, p + 3 * BLOCK), Some(all));
        assert_eq!(guard(p, p + 3 * BLOCK), Some(Vec::new()), "nothing written since");
        // SAFETY: inside the mapped middle block.
        unsafe { ((p + BLOCK + 8) as *mut u8).write_volatile(1) };
        assert_eq!(guard(p, p + 3 * BLOCK), Some(pages(1).collect()));
        let first = map(0);
        title_memory_remapped(p, BLOCK);
        assert_eq!(guard(p, p + 3 * BLOCK), Some(pages(0).collect()));
        assert_eq!(guard(p, p + 3 * BLOCK), Some(Vec::new()));
        // Memory of ours that is no view stays none.
        let mut local = [0u8; 16];
        let at = local.as_mut_ptr() as usize;
        assert!(!is_view(at, at + local.len()));
        // SAFETY: our views and placeholder.
        unsafe {
            UnmapViewOfFile2(GetCurrentProcess(), first, MEM_PRESERVE_PLACEHOLDER);
            UnmapViewOfFile2(GetCurrentProcess(), middle, MEM_PRESERVE_PLACEHOLDER);
            for block in 0..3 {
                VirtualFree((p + block * BLOCK) as *mut _, 0, MEM_RELEASE);
            }
            windows_sys::Win32::Foundation::CloseHandle(section);
        }
        title_memory_remapped(p, 3 * BLOCK);
    }

    /// Blocks of a placeholder between views are reported once, then not
    /// again (they cannot be guarded) until the runtime maps them.
    #[test]
    fn unmapped_blocks_are_reported_once_until_mapped() {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, MEM_PRESERVE_PLACEHOLDER, MEM_REPLACE_PLACEHOLDER,
            MEM_RESERVE_PLACEHOLDER, MapViewOfFile3, PAGE_NOACCESS, UnmapViewOfFile2,
            VirtualAlloc2,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: a fresh placeholder of three blocks and a pagefile
        // section of two; block 0 shows the section's first block, as the
        // runtime maps physical pages.
        let (p, section) = unsafe {
            let p = VirtualAlloc2(
                GetCurrentProcess(),
                std::ptr::null(),
                3 * BLOCK,
                MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                PAGE_NOACCESS,
                std::ptr::null_mut(),
                0,
            ) as usize;
            assert_ne!(p, 0);
            let section = CreateFileMappingW(
                -1isize as _,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                (2 * BLOCK) as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            (p, section)
        };
        let map = |block: usize| {
            // SAFETY: splits the placeholder at the block and maps the
            // section's block there.
            let view = unsafe {
                VirtualFree(
                    (p + block * BLOCK) as *mut _,
                    BLOCK,
                    MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
                );
                MapViewOfFile3(
                    section,
                    GetCurrentProcess(),
                    (p + block * BLOCK) as *const _,
                    (block * BLOCK) as u64,
                    BLOCK,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(view.Value as usize, p + block * BLOCK);
            view
        };
        let first = map(0);
        let pages = |block: usize| (p + block * BLOCK) / PAGE..(p + (block + 1) * BLOCK) / PAGE;
        let all: Vec<usize> = (0..3).flat_map(pages).collect();
        assert_eq!(guard(p, p + 3 * BLOCK), Some(all));
        assert_eq!(
            guard(p, p + 3 * BLOCK),
            Some(Vec::new()),
            "nothing written since"
        );
        let second = map(1);
        title_memory_remapped(p + BLOCK, BLOCK);
        assert_eq!(guard(p, p + 3 * BLOCK), Some(pages(1).collect()));
        assert_eq!(guard(p, p + 3 * BLOCK), Some(Vec::new()));
        // SAFETY: our views and placeholder.
        unsafe {
            UnmapViewOfFile2(GetCurrentProcess(), first, MEM_PRESERVE_PLACEHOLDER);
            UnmapViewOfFile2(GetCurrentProcess(), second, MEM_PRESERVE_PLACEHOLDER);
            for block in 0..3 {
                VirtualFree((p + block * BLOCK) as *mut _, 0, MEM_RELEASE);
            }
            windows_sys::Win32::Foundation::CloseHandle(section);
        }
        title_memory_remapped(p, 3 * BLOCK);
    }

    /// A pagefile section mapped twice: two views of the same physical
    /// pages, the second left for the test; the section handle, the two
    /// addresses.
    fn two_views(bytes: usize) -> (usize, usize, usize) {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile,
        };
        // SAFETY: a fresh pagefile section, mapped twice; the views keep
        // the section alive and the handle is closed by the caller.
        unsafe {
            let section = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                bytes as u32,
                std::ptr::null(),
            );
            assert!(!section.is_null());
            let one = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, bytes).Value as usize;
            let two = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, bytes).Value as usize;
            assert_ne!(one, 0);
            assert_ne!(two, 0);
            crate::inplace::title_memory_view(one, bytes, section as usize, 0);
            crate::inplace::title_memory_view(two, bytes, section as usize, 0);
            (section as usize, one, two)
        }
    }

    /// The section of [`two_views`] is closed and its views unmapped; the
    /// guard is told they are gone first.
    fn drop_views(section: usize, views: &[usize], bytes: usize) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
        for &view in views {
            title_memory_remapped(view, bytes);
            // SAFETY: a view of the section mapped by `two_views`.
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: view as *mut _,
                })
            };
        }
        // SAFETY: the section's last reference.
        unsafe { CloseHandle(section as *mut std::ffi::c_void) };
    }

    /// The title wrote the pages of a guarded texture through another view
    /// of them: the fault at that view is the guard's to see, and the
    /// guarded view's next check reports the pages written.
    #[test]
    fn a_write_through_another_view_is_reported_at_the_guarded_one() {
        const SIZE: usize = 2 * BLOCK;
        let (section, guarded, other) = two_views(SIZE);
        let pages = |block: usize| (guarded + block * BLOCK) / PAGE;
        // The first check takes it in (and every view of its pages).
        assert_eq!(guard(guarded, guarded + SIZE), Some(vec![pages(0), pages(1)]));
        assert_eq!(guard(guarded, guarded + SIZE), Some(Vec::new()), "nothing written yet");
        // Writing through the other view faults into the handler, which
        // opens that view and the guarded one: the next check sees it.
        // SAFETY: the second view, read-write once the handler has opened it.
        unsafe { ((other + BLOCK + 8) as *mut u8).write(1) };
        assert_eq!(guard(guarded, guarded + SIZE), Some(vec![pages(1)]));
        assert_eq!(guard(guarded, guarded + SIZE), Some(Vec::new()));
        drop_views(section, &[guarded, other], SIZE);
    }

    /// A view of guarded pages is mapped anew: it is writable, so the
    /// guarded view checks its pages again (and guards the new view).
    #[test]
    fn a_view_mapped_beside_a_guarded_one_makes_it_check_again() {
        const SIZE: usize = 2 * BLOCK;
        let (section, guarded, other) = two_views(SIZE);
        let pages = |block: usize| (guarded + block * BLOCK) / PAGE;
        assert_eq!(guard(guarded, guarded + SIZE), Some(vec![pages(0), pages(1)]));
        assert_eq!(guard(guarded, guarded + SIZE), Some(Vec::new()), "nothing written yet");
        title_memory_remapped(other, SIZE);
        crate::inplace::title_memory_view(other, SIZE, section, 0);
        assert_eq!(guard(guarded, guarded + SIZE), Some(vec![pages(0), pages(1)]));
        assert_eq!(guard(guarded, guarded + SIZE), Some(Vec::new()));
        drop_views(section, &[guarded, other], SIZE);
    }

    /// A write of the runtime's own, told at one view of the pages
    /// (`writing`): the other views' next checks report the pages written.
    #[test]
    fn a_write_told_at_one_view_is_reported_at_the_others() {
        const SIZE: usize = 2 * BLOCK;
        let (section, guarded, other) = two_views(SIZE);
        let pages = |block: usize| (guarded + block * BLOCK) / PAGE;
        assert_eq!(guard(guarded, guarded + SIZE), Some(vec![pages(0), pages(1)]));
        assert_eq!(guard(guarded, guarded + SIZE), Some(Vec::new()), "nothing written yet");
        writing(other + BLOCK, BLOCK);
        assert_eq!(guard(guarded, guarded + SIZE), Some(vec![pages(1)]));
        assert_eq!(guard(guarded, guarded + SIZE), Some(Vec::new()));
        // Half a page and the next: both blocks, not just the first's.
        writing(other + BLOCK / 2, BLOCK);
        assert_eq!(guard(guarded, guarded + SIZE), Some(vec![pages(0), pages(1)]));
        drop_views(section, &[guarded, other], SIZE);
    }

    /// Both views of the pages are checked, each for a copy of its own
    /// (the title shows one physical page at several addresses, and each
    /// address has its host copy): a write through either is reported at
    /// both, whichever is checked first. Guarding the pages for the first
    /// view's check must not note the second view clean, or its copy would
    /// keep the bytes from before the write.
    #[test]
    fn a_write_is_reported_at_every_checked_view() {
        const SIZE: usize = 2 * BLOCK;
        let (section, one, two) = two_views(SIZE);
        let pages = |view: usize, block: usize| (view + block * BLOCK) / PAGE;
        assert_eq!(guard(one, one + SIZE), Some(vec![pages(one, 0), pages(one, 1)]));
        assert_eq!(guard(two, two + SIZE), Some(vec![pages(two, 0), pages(two, 1)]), "the second view's first check");
        assert_eq!(guard(one, one + SIZE), Some(Vec::new()), "nothing written yet");
        assert_eq!(guard(two, two + SIZE), Some(Vec::new()), "nothing written yet");
        // SAFETY: the second view, read-write once the handler has opened it.
        unsafe { ((two + BLOCK + 8) as *mut u8).write(1) };
        assert_eq!(guard(one, one + SIZE), Some(vec![pages(one, 1)]));
        assert_eq!(guard(two, two + SIZE), Some(vec![pages(two, 1)]), "written through this view, checked second");
        // SAFETY: the first view, as above.
        unsafe { ((one + 8) as *mut u8).write(1) };
        assert_eq!(guard(two, two + SIZE), Some(vec![pages(two, 0)]));
        assert_eq!(guard(one, one + SIZE), Some(vec![pages(one, 0)]), "written through this view, checked second");
        assert_eq!(guard(one, one + SIZE), Some(Vec::new()));
        assert_eq!(guard(two, two + SIZE), Some(Vec::new()));
        drop_views(section, &[one, two], SIZE);
    }

    /// A page shown in more than `MANY_VIEWS` views is guarded at each by
    /// itself: the others stay writable, and nothing looks through them all.
    #[test]
    fn a_page_of_many_views_is_guarded_one_view_at_a_time() {
        let (section, one, two) = two_views(BLOCK);
        let filler = one + (1 << 45);
        for n in 0..MANY_VIEWS {
            crate::inplace::title_memory_view(filler + n * BLOCK, BLOCK, section, 0);
        }
        assert_eq!(guard(one, one + BLOCK), Some(vec![one / PAGE]));
        assert!(writable(two), "another view of it is left alone");
        // SAFETY: the guarded view, opened by the handler.
        unsafe { ((one + 8) as *mut u8).write(1) };
        assert_eq!(guard(one, one + BLOCK), Some(vec![one / PAGE]));
        title_memory_remapped(filler, MANY_VIEWS * BLOCK);
        drop_views(section, &[one, two], BLOCK);
    }

    /// A write or mapping that runs past the end of the address space (a
    /// title's `fread` with a huge count) covers up to the end; the sum used
    /// to overflow. Up there no test has guarded blocks.
    #[test]
    fn ranges_past_the_address_space_end_do_not_overflow() {
        let top = usize::MAX - 16;
        writing(top, PAGE);
        title_memory_remapped(top, usize::MAX);
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::{
        SHARDS, shards, title_memory_reading, title_memory_reading_now, title_memory_reading_range,
        title_memory_remapping, title_memory_remapping_range,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    /// One of these tests at a time: the first takes every lock, which the
    /// second's "at once" would meet.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A change of mapping waits for a read in place that is under way, and
    /// a read in place waits for a change of mapping.
    #[test]
    fn reads_in_place_and_remappings_wait_for_each_other() {
        let _alone = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        // A reader holds; the change does not begin until it lets go.
        let changed = Arc::new(AtomicBool::new(false));
        let reading = title_memory_reading();
        let (began, wait) = mpsc::channel();
        let change = {
            let changed = changed.clone();
            std::thread::spawn(move || {
                began.send(()).unwrap();
                let _changing = title_memory_remapping();
                changed.store(true, Ordering::SeqCst);
            })
        };
        wait.recv().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!changed.load(Ordering::SeqCst), "the mapping changed under a read in place");
        drop(reading);
        change.join().unwrap();
        assert!(changed.load(Ordering::SeqCst));
        // A change holds; the read does not begin until it is done.
        let read = Arc::new(AtomicBool::new(false));
        let changing = title_memory_remapping();
        let (began, wait) = mpsc::channel();
        let reader = {
            let read = read.clone();
            std::thread::spawn(move || {
                began.send(()).unwrap();
                let _reading = title_memory_reading();
                read.store(true, Ordering::SeqCst);
            })
        };
        wait.recv().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!read.load(Ordering::SeqCst), "a read in place began during a change of mapping");
        drop(changing);
        reader.join().unwrap();
        assert!(read.load(Ordering::SeqCst));
    }

    /// The locks a range takes: those of its 16 MB spans, ascending, and
    /// all of them for the whole of title memory or a range that wraps.
    #[test]
    fn a_range_takes_the_locks_of_its_spans() {
        let mb = 1 << 20;
        assert_eq!(shards(0, 0).len(), SHARDS);
        assert_eq!(shards(0x1_0000_0000 + 3 * mb, mb), vec![0]);
        assert_eq!(shards(0x1_0000_0000 + 15 * mb, 2 * mb), vec![0, 1]);
        // Spans that wrap past the last lock come back to the first.
        assert_eq!(shards(255 << 24, 32 * mb), vec![0, 255]);
        assert_eq!(shards(0, 256 << 24).len(), SHARDS);
    }

    /// A read in place holds off a change of the addresses it reads, and
    /// only those: a change elsewhere goes on meanwhile.
    #[test]
    fn a_read_holds_off_changes_of_its_own_addresses_only() {
        let _alone = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        // Addresses no other test takes, far apart in the locks.
        let (here, there) = (0x7_0000_0000 + (40 << 24), 0x7_0000_0000 + (90 << 24));
        let reading = title_memory_reading_range(here, 64 << 10);
        // Elsewhere: at once.
        let elsewhere = std::thread::spawn(move || drop(title_memory_remapping_range(there, 64 << 10)));
        elsewhere.join().unwrap();
        // Here: not until the read lets go.
        let changed = Arc::new(AtomicBool::new(false));
        let change = {
            let changed = changed.clone();
            std::thread::spawn(move || {
                let _changing = title_memory_remapping_range(here + (32 << 10), 1);
                changed.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!changed.load(Ordering::SeqCst), "the mapping changed under a read in place");
        drop(reading);
        change.join().unwrap();
        // A change holds its addresses: a read there cannot be had at once,
        // one elsewhere can.
        let changing = title_memory_remapping_range(here, 1);
        let (asked, answer) = mpsc::channel();
        std::thread::spawn(move || {
            asked
                .send((title_memory_reading_now(here, 1).is_some(), title_memory_reading_now(there, 1).is_some()))
                .unwrap();
        });
        assert_eq!(answer.recv().unwrap(), (false, true));
        drop(changing);
    }
}
