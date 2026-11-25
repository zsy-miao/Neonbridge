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

use std::collections::{BTreeMap, BTreeSet};
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
    seen: Mutex<BTreeMap<usize, u64>>,
}

/// Tracks a runtime GPU-memory range, including physical-page views when
/// `views` is true. Other process memory is not tracked.
pub(crate) fn track(start: usize, len: usize, views: bool) -> Option<Tracked> {
    let end = start.checked_add(len)?;
    if len == 0 || writes(start, end, views, u64::MAX).is_none() {
        return None;
    }
    Some(Tracked {
        start,
        end,
        seen: Mutex::default(),
    })
}

struct Journal {
    /// Numbers the askings; 0 is "never".
    asked: u64,
    /// Page number → the asking that last found it written.
    written: BTreeMap<usize, u64>,
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
    /// Page runs asked ahead (`(first, last)`): askings in a row that found
    /// nothing written, and the frame of the last kernel asking ([`Cold`]).
    cold: BTreeMap<(usize, usize), Cold>,
}

/// A page run's record for [`cold_skip`].
#[derive(Clone, Copy, Default)]
struct Cold {
    clean: u32,
    frame: u64,
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal {
    asked: 0,
    written: BTreeMap::new(),
    found: Vec::new(),
    ahead: Vec::new(),
    ahead_asked: 0,
    kept: false,
    cold: BTreeMap::new(),
});

/// Askings in a row that found nothing written before a page run counts as
/// cold ([`cold_skip`]): about three frames of the world's 30 submissions.
const COLD_AFTER: u32 = 90;

/// Whether the kernel asking of the refreshed piece `[first, last)` (pages)
/// can be left out (Flamingo). The world's submissions ask the kernel about
/// the same big ranges some 30 times a frame, and 8.5 of the 8.6 GB a frame
/// they look at (17.8 ms, run 27) was never written: the title's static
/// geometry. A piece found unwritten `COLD_AFTER` times in a row is asked at
/// most once a frame and otherwise answered from the journal; the first
/// written page found brings it back to every asking. Nothing is lost: the
/// kernel keeps a page's written mark until it is asked, so a write that a
/// skipped asking would have seen is seen at the piece's next asking, at
/// most a frame later. (Whole runs of nearby pieces were never all clean:
/// a dynamic buffer lies beside most static ones.) `FLAMINGO_COLD_ASKS=0`
/// asks every time. Off by default (`FLAMINGO_COLD_ASKS=1` turns it on): in
/// run 30 it left out 168 of some 2,600 askings a frame and saved nothing
/// measurable; an asking's cost is mostly the call (7 microseconds), and the
/// pieces around a cold one are still asked in one run with it.
fn cold_skip(j: &mut Journal, first: usize, last: usize, frame: u64) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("FLAMINGO_COLD_ASKS").is_some_and(|v| v == "1")) {
        return false;
    }
    cold_ready(j, first, last, frame)
}

/// Whether the piece `[first, last)` is cold and was asked about in `frame`
/// already ([`cold_skip`], the switch aside).
fn cold_ready(j: &Journal, first: usize, last: usize, frame: u64) -> bool {
    j.cold.get(&(first, last)).is_some_and(|c| c.clean >= COLD_AFTER && c.frame == frame)
}

/// Notes what a kernel asking of `[first, last)` found ([`cold_skip`]).
fn cold_note(j: &mut Journal, first: usize, last: usize, frame: u64, written: bool) {
    // (runs are many and short-lived: the map is let go when it grows large)
    if j.cold.len() > 1 << 16 {
        j.cold.clear();
    }
    let c = j.cold.entry((first, last)).or_default();
    c.clean = if written { 0 } else { c.clean.saturating_add(1) };
    c.frame = frame;
}

/// Ranges of one submission closer than this are asked about together:
/// walking the pages between them costs less than another kernel call (a
/// call that resets written pages costs 30-70 microseconds whatever the
/// range).
const AHEAD_GAP: usize = (8 << 20) / PAGE;
/// The most one asking covers (its answer needs 8 bytes per 4 KiB page).
const AHEAD_MAX: usize = (1 << 30) / PAGE;

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
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    // The submission's one asking stands ([`ask_once`]): what it does not
    // hold is left to `writes`.
    if j.kept {
        return;
    }
    // View memory the guards hold is left to `writes`, which answers it
    // from the guards ([`known_view`]): the kernel's write watch can only
    // fail there. Asked here with the others, a run holding such a range
    // failed, was asked again by halves down to single ranges, failing
    // each time, and the range then went to `writes` all the same (after
    // triippywhale d0581a55). `VICTORY_VIEW_ASK=1` asks as before.
    #[cfg(feature = "diagnostics")]
    let all = pieces.len();
    pieces.retain(|&(first, _)| !known_view(first * PAGE));
    #[cfg(feature = "diagnostics")]
    AHEAD_VIEWS.fetch_add((all - pieces.len()) as u64, std::sync::atomic::Ordering::Relaxed);
    j.asked += 1;
    j.ahead_asked = j.asked;
    j.ahead.clear();
    // Flamingo: the cold pieces this frame already asked about are answered
    // from the journal, and the others asked ([`cold_skip`]).
    let frame = crate::frame::current();
    let all = pieces;
    let mut pieces = Vec::with_capacity(all.len());
    for &(first, last) in &all {
        if cold_skip(j, first, last, frame) {
            j.ahead.push((first, last));
            #[cfg(feature = "diagnostics")]
            crate::copystat::add(crate::copystat::C::WatchesSkipped, 1);
        } else {
            pieces.push((first, last));
        }
    }
    let skipped = j.ahead.len();
    let mut i = 0;
    while i < pieces.len() {
        let first = pieces[i].0;
        let mut last = pieces[i].1;
        let mut k = i + 1;
        while k < pieces.len()
            && pieces[k].0 <= last + AHEAD_GAP
            && last.max(pieces[k].1) - first <= AHEAD_MAX
        {
            last = last.max(pieces[k].1);
            k += 1;
        }
        ask_run(j, &pieces[i..k]);
        i = k;
    }
    // What each piece the kernel was asked about held: written pages found by
    // this asking, or none. Pieces no asking covered (not write-watched) are
    // left alone.
    let now = j.ahead_asked;
    for &(first, last) in &pieces {
        if !j.ahead[skipped..].iter().any(|&(a, b)| a <= first && last <= b) {
            continue;
        }
        let written = j.written.range(first..last).any(|(_, &w)| w == now);
        cold_note(j, first, last, frame, written);
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

/// Since the last frame line: ranges of view memory the guards hold that a
/// submission's askings ahead ([`ask_ahead`], [`ask_once`]) left out. Each
/// was a kernel asking that failed, or several, before.
#[cfg(feature = "diagnostics")]
pub(crate) static AHEAD_VIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// [`SLOW`], taken.
#[cfg(feature = "diagnostics")]
pub(crate) fn slow_asks() -> [u64; 3] {
    [0, 1, 2].map(|i| SLOW[i].swap(0, std::sync::atomic::Ordering::Relaxed))
}

/// The submission's refreshes are done: `writes` asks the kernel again.
pub(crate) fn ask_ahead_done() {
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
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
    SUBMISSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if !once() {
        return;
    }
    ask_once_end();
    ask_ahead(ranges);
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    if j.ahead.is_empty() {
        // `ask_ahead` asked nothing (fewer than two ranges).
        j.asked += 1;
        j.ahead_asked = j.asked;
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
        // View memory the guards hold is not asked about: the asking can
        // only fail there, one failed kernel call for each such copy at
        // every submission, and `writes` answers it from the guards
        // (after triippywhale cc368c7c).
        if known_view(start) {
            #[cfg(feature = "diagnostics")]
            AHEAD_VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            continue;
        }
        let (first, last) = (start / PAGE, end.div_ceil(PAGE));
        if !j.ahead.iter().any(|&(a, b)| a <= first && last <= b) && ask_kernel(j, first, last) {
            j.ahead.push((first, last));
        }
    }
    j.kept = !j.ahead.is_empty();
}

/// The submission's one asking stands no longer: its refreshes are done,
/// or title memory was written in a way the journal was not told.
pub(crate) fn ask_once_end() {
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    if j.kept {
        j.kept = false;
        j.ahead.clear();
    }
}

/// We wrote title memory `[at, at + len)` while a submission's one asking
/// stands (the title's buffer copies, done at submission): its pages count
/// as written after that asking, so the refreshes that follow copy them.
pub(crate) fn wrote(at: usize, len: usize) {
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    if !j.kept || len == 0 {
        return;
    }
    j.asked += 1;
    let now = j.asked;
    // The asking that stands moves with it: `asked` and `writes` give it as
    // "now", and a page noted written after "now" stays stale for every
    // copy that takes it (copied at `now`, written at `now + 1`): copied
    // again at every refresh, and a reason to settle flights
    // (fxceless/victory-fork#410).
    j.ahead_asked = now;
    for page in at / PAGE..at.saturating_add(len).div_ceil(PAGE) {
        j.written.insert(page, now);
    }
}

/// We wrote the 64 KiB page numbered `page` of title memory through a
/// view of our own (`crate::inplace::write_own`): no guard was opened and
/// no kernel write-watch bit set, so the journal is told here. The page,
/// and the same physical page at its other title addresses (`others`,
/// page numbers), count as written at a new asking, which every host copy
/// that holds them takes at its next refresh. Returns the asking that had
/// last found `page` written (0: never) and the new one, for the copy
/// that wrote to note itself current.
pub(crate) fn wrote_own(page: usize, others: &[usize]) -> (u64, u64) {
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    j.asked += 1;
    let now = j.asked;
    // As in [`wrote`]: an asking that stands for a submission, or for a
    // flush, moves with it, or a copy taking the page at the standing
    // asking would find it written "later" at every refresh.
    if j.kept || !j.ahead.is_empty() {
        j.ahead_asked = now;
    }
    let before = j.written.insert(page, now).unwrap_or(0);
    for &other in others {
        if other != page {
            j.written.insert(other, now);
        }
    }
    (before, now)
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
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    let mut count = (last - first) * PAGE / KERNEL_PAGE;
    j.found.resize(count.max(j.found.len()), 0);
    let mut granularity = 0;
    #[cfg(feature = "diagnostics")]
    let started = std::time::Instant::now();
    // SAFETY: the answer buffer holds `count` entries.
    let failed = unsafe {
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
        return false;
    }
    #[cfg(feature = "diagnostics")]
    {
        use crate::copystat::{C, add};
        add(C::Watches, 1);
        add(C::WatchUs, started.elapsed().as_micros() as u64);
        slow(started.elapsed().as_micros() as u64);
    }
    let now = j.ahead_asked;
    for k in 0..count {
        let page = j.found[k] / PAGE;
        j.written.insert(page, now);
    }
    true
}

/// What [`writes`] would say of `[start, end)` from the journal alone, if
/// the range lies in one asked ahead ([`ask_ahead`], [`ask_once`]): the
/// pages written since tracking began and the asking that last found each
/// written, with the number of the asking that stands (as [`writes`] gives
/// it). `None` if the kernel would have to be asked.
pub(crate) fn asked(start: usize, end: usize) -> Option<(u64, Vec<(usize, u64)>)> {
    let (first, last) = (start / PAGE, end.div_ceil(PAGE));
    let j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    j.ahead
        .iter()
        .any(|&(a, b)| a <= first && last <= b)
        .then(|| (j.ahead_asked, j.written.range(first..last).map(|(&p, &w)| (p, w)).collect()))
}

/// The pages of `[start, end)` (whole pages) as last written, for pages
/// written since tracking began: `(page number, asking)` in page order, and
/// this asking's number, which is higher than any in the list but those the
/// kernel just reported. `None` if the memory is not write-watched (nor,
/// with `guarded`, view memory, which is then guarded).
///
/// Flamingo: only pages last written by an asking after `since` are listed
/// (0: all). The callers want those alone (what their copies took is as
/// new), and the whole list, made anew at every one of some 2,400 askings
/// a frame, was most of this function's time (run 119).
pub(crate) fn writes(start: usize, end: usize, guarded: bool, since: u64) -> Option<(u64, Vec<(usize, u64)>)> {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    let (first, last) = (start / PAGE, end.div_ceil(PAGE));
    let mut j = JOURNAL.lock().unwrap_or_else(|e| e.into_inner());
    let j = &mut *j;
    // Asked ahead with the submission's other ranges: the journal has it.
    if j.ahead.iter().any(|&(a, b)| a <= first && last <= b) {
        return Some((
            j.ahead_asked,
            j.written
                .range(first..last)
                .filter(|&(_, &w)| w > since)
                .map(|(&p, &w)| (p, w))
                .collect(),
        ));
    }
    let mut count = (last - first) * PAGE / KERNEL_PAGE;
    j.found.resize(count.max(j.found.len()), 0);
    let mut granularity = 0;
    #[cfg(feature = "diagnostics")]
    let started = std::time::Instant::now();
    // View memory the guards already hold cannot be write-watched: the
    // kernel asking would only fail. Copies of it are flushed many times a
    // frame, each time paying that failed call before the guards answer.
    let known_view = guarded && known_view(first * PAGE);
    // SAFETY: the answer buffer holds `count` entries.
    let failed = known_view
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
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(
        if known_view {
            crate::copystat::C::WatchKnownView
        } else {
            crate::copystat::C::WatchFailed
        },
        failed as u64,
    );
    #[cfg(feature = "diagnostics")]
    if failed && !known_view && guarded {
        failed_asking(first * PAGE, last * PAGE);
    }
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
    j.asked += 1;
    let now = j.asked;
    for page in j.found[..count].iter().map(|&at| at / PAGE).chain(guarded) {
        j.written.insert(page, now);
    }
    Some((
        now,
        j.written
            .range(first..last)
            .filter(|&(_, &w)| w > since)
            .map(|(&p, &w)| (p, w))
            .collect(),
    ))
}

/// Diagnostics: one in 2,048 of the askings the kernel's write watch
/// refused for memory the guards did not know (some 200 a frame in the
/// first rooms, each followed by a question to Windows about the memory):
/// what the memory is and what the guards hold of its first block.
#[cfg(feature = "diagnostics")]
fn failed_asking(start: usize, end: usize) {
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if !N.fetch_add(1, std::sync::atomic::Ordering::Relaxed).is_multiple_of(2048) {
        return;
    }
    // SAFETY: plain data filled in by the query.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let n = unsafe { VirtualQuery(start as *const _, &mut info, size_of_val(&info)) };
    let (protected, clean, absent, viewed, aliased) = {
        let g = guards();
        let b = start / BLOCK;
        (g.protected.contains(&b), g.clean.contains(&b), g.absent.contains(&b), g.viewed.contains(&b), g.aliased.contains(&b))
    };
    tracing::info!(
        "a failed write-watch asking (one in 2048): {start:#x}+{:#x}; Windows says (answered {n}): base {:#x} region {:#x} state {:#x} type {:#x} protect {:#x} allocation protect {:#x}; \
         the guards of its first block: protected {protected} clean {clean} absent {absent} viewed {viewed} aliased {aliased}; is a view: {}",
        end - start,
        info.BaseAddress as usize,
        info.RegionSize,
        info.State,
        info.Type,
        info.Protect,
        info.AllocationProtect,
        is_view(start, end)
    );
}

impl Tracked {
    /// Takes changed pages of `[from, to)`, cut to the tracked range. A
    /// partial page stays changed so a later take can read its other bytes.
    pub(crate) fn take(&self, from: usize, to: usize) -> Vec<(usize, usize)> {
        let (from, to) = (from.max(self.start), to.min(self.end));
        if from >= to {
            return Vec::new();
        }
        let Some((_, pages)) = writes(from, to, true, 0) else {
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

/// Whether the block holding `start` is view memory the guards already
/// hold (read-only by our hand, or noted absent): asking the kernel's write
/// watch about it can only fail. `VICTORY_VIEW_ASK=1` (or
/// `VICTORY_GUARD_ASK=1`) asks the kernel anyway, as before.
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
    let g = guards();
    let first = start / BLOCK;
    g.protected.contains(&first) || g.absent.contains(&first) || (view_set() && covered(&g.views, first, first + 1)) || (view_known() && g.viewed.contains(&first))
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
    /// Blocks known to be view memory ([`guard`]), in order and joined:
    /// a range in them needs no question to Windows to tell.
    views: Vec<(usize, usize)>,
    /// The blocks of `views` that are not clean (written since a copy took
    /// them, never taken, or not guardable), but for the parked ones: an
    /// asking whose range holds none of them is answered in one lookup,
    /// and otherwise walks only them ([`guard_unclean`]).
    unclean: BTreeSet<usize>,
    /// View blocks found uncommitted ([`Guards::absent`]), taken out of
    /// `unclean` while they stay so ([`park`]). One goes back when the
    /// runtime maps it ([`title_memory_remapped`]), when the title commits
    /// it through the runtime ([`title_memory_committed`]), or, if it is no
    /// placeholder, when the look at those every [`park_epoch`] submissions
    /// finds it committed (an ordinary commit no one tells of). The walk
    /// over every block of a range asked Windows about each at every
    /// asking: in the first room ~86,000-96,000 such blocks, nearly all
    /// 64 KiB placeholders, each its own region, and ~88,000 questions a
    /// frame (guards 26 ms of the submitting thread's frame on an RTX 5080;
    /// 14 ms parked).
    parked: BTreeSet<usize>,
    /// The parked blocks that are no placeholder: the ones looked at again.
    parked_ordinary: BTreeSet<usize>,
    /// The epoch ([`SUBMISSIONS`] / [`park_epoch`]) of the last look at
    /// `parked_ordinary`.
    parked_at: u64,
    /// With [`view_known`]: blocks made read-only at least once since the
    /// runtime last mapped or unmapped them. They are view memory whether
    /// their guard stands now or a write has opened it.
    viewed: BTreeSet<usize>,
}

/// Whether a block once guarded stays known as view memory while its
/// guard is open (`VICTORY_VIEW_KNOWN=1`; off by default).
///
/// A range whose first block the title wrote since the last check has
/// that block neither read-only nor noted absent, so nothing says it is a
/// view: the kernel's write watch is asked and fails, and then Windows is
/// asked what the memory is (`is_view`), a walk of the page tables to the
/// end of the pages that share the first one's state. In runs 222 and 225
/// that was 338-450 failed askings a frame and 6.5-7.8 ms a frame of the
/// submitting thread in `VirtualQuery`. With the switch the block is
/// remembered from the time it was first made read-only until the runtime
/// maps or unmaps it ([`title_memory_remapped`]).
///
/// `VICTORY_VIEW_KNOWN=alt`: twenty seconds with, twenty without, in turn,
/// each change said in the log, for measuring it in one scene. (Blocks
/// noted in one phase are forgotten at a remapping in either.)
fn view_known() -> bool {
    /// 0 off, 1 on, 2 in turn.
    static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    match *MODE.get_or_init(|| match std::env::var("VICTORY_VIEW_KNOWN").as_deref() {
        Ok("1") => 1,
        Ok("alt") => 2,
        _ => 0,
    }) {
        0 => false,
        1 => true,
        _ => {
            static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let on = (EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs() / 20) % 2 == 1;
            static LAST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
            if LAST.swap(u8::from(on), std::sync::atomic::Ordering::Relaxed) != u8::from(on) {
                tracing::info!(
                    "view memory stays known while its guard is open from now (VICTORY_VIEW_KNOWN=alt): {}",
                    if on { "ON" } else { "off, as usual" }
                );
            }
            on
        }
    }
}

static GUARDS: Mutex<Guards> = Mutex::new(Guards {
    protected: BTreeSet::new(),
    clean: BTreeSet::new(),
    absent: BTreeSet::new(),
    aliased: BTreeSet::new(),
    views: Vec::new(),
    unclean: BTreeSet::new(),
    parked: BTreeSet::new(),
    parked_ordinary: BTreeSet::new(),
    parked_at: 0,
    viewed: BTreeSet::new(),
});

/// Tests: `[start, end)` is no longer known as view memory. A test's own
/// allocation can land where an earlier test's views were, which the view
/// set still names (title memory stays view memory in a run, so nothing
/// takes a block out of it there).
#[cfg(test)]
pub(crate) fn forget_views(start: usize, end: usize) {
    let mut g = guards();
    let (a, b) = (start / BLOCK, end.div_ceil(BLOCK));
    let views = std::mem::take(&mut g.views);
    for (x, y) in views {
        if y <= a || b <= x {
            g.views.push((x, y));
        } else {
            if x < a {
                g.views.push((x, a));
            }
            if b < y {
                g.views.push((b, y));
            }
        }
    }
    for block in a..b {
        g.protected.remove(&block);
        g.absent.remove(&block);
        g.viewed.remove(&block);
        g.unclean.remove(&block);
    }
}

fn guards() -> std::sync::MutexGuard<'static, Guards> {
    GUARDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `[first, last)` added to `ranges` (in order, joined). For ranges seen
/// rarely (the views, once per new range): it sorts the whole list.
fn add_range(ranges: &mut Vec<(usize, usize)>, first: usize, last: usize) {
    if first >= last {
        return;
    }
    ranges.push((first, last));
    ranges.sort_unstable();
    let mut joined: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for &(a, b) in ranges.iter() {
        match joined.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => joined.push((a, b)),
        }
    }
    *ranges = joined;
}

/// Whether blocks `[first, last)` all lie in `ranges` (in order, joined).
fn covered(ranges: &[(usize, usize)], first: usize, last: usize) -> bool {
    let at = ranges.partition_point(|r| r.1 <= first);
    ranges.get(at).is_some_and(|r| r.0 <= first && last <= r.1)
}

/// Whether to keep views' not-clean blocks in a set and answer from it
/// ([`Guards::unclean`], [`Guards::parked`]; `VICTORY_VIEW_SET=0` walks
/// every block of a range, as before). Only where Windows tells
/// placeholders apart ([`placeholders_told`]).
fn view_set() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_VIEW_SET").is_none_or(|v| v != "0") && placeholders_told())
}

/// Whether [`in_placeholder`] knows a placeholder, asked once of one of
/// our own. Where it does not (Wine may not fill the flag), every parked
/// block would be looked at each epoch, more questions than the walk
/// asks: the walk stays there.
fn placeholders_told() -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_RELEASE, MEM_RESERVE, MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, VirtualAlloc2, VirtualFree,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: a fresh placeholder of one block, released at once.
    let p = unsafe {
        VirtualAlloc2(
            GetCurrentProcess(),
            std::ptr::null(),
            BLOCK,
            MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
            PAGE_NOACCESS,
            std::ptr::null_mut(),
            0,
        )
    } as usize;
    if p == 0 {
        return false;
    }
    let told = in_placeholder(p / BLOCK);
    // SAFETY: our own placeholder, known to nothing else.
    unsafe { VirtualFree(p as _, 0, MEM_RELEASE) };
    if !told {
        tracing::info!("Windows does not tell placeholders apart here: view memory askings walk every block (VICTORY_VIEW_SET)");
    }
    told
}

/// Since the last frame line ([`view_stats`]): view askings answered in one
/// lookup, view askings that walked their not-clean blocks, the blocks they
/// walked, the questions to Windows of whether a range is a view, the
/// questions whether blocks found uncommitted still are, and the parked
/// blocks found committed ([`Guards::parked`]).
static VIEW_STATS: [std::sync::atomic::AtomicU64; 6] = [const { std::sync::atomic::AtomicU64::new(0) }; 6];

/// [`VIEW_STATS`], taken.
pub(crate) fn view_stats() -> [u64; 6] {
    [0, 1, 2, 3, 4, 5].map(|i| VIEW_STATS[i].swap(0, std::sync::atomic::Ordering::Relaxed))
}

/// Submissions' askings begun ([`ask_once`]).
static SUBMISSIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Submissions' askings between looks at the parked blocks that are no
/// placeholder (`VICTORY_PARK_EPOCH`, default 32; 1 looks at every one).
fn park_epoch() -> u64 {
    static N: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("VICTORY_PARK_EPOCH")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(32)
    })
}

/// The view blocks parked as uncommitted now ([`Guards::parked`]), and
/// those of them that are no placeholder.
pub(crate) fn parked_blocks() -> (usize, usize) {
    let g = guards();
    (g.parked.len(), g.parked_ordinary.len())
}

/// Parks `block`, found uncommitted and not clean ([`Guards::parked`]). A
/// block in a placeholder is not looked at again until the runtime maps
/// it: a plain commit into a placeholder fails, only a call that replaces
/// one fills it, and on title memory the runtime's are the only such calls
/// and tell of themselves ([`title_memory_remapped`]).
fn park(g: &mut Guards, block: usize) {
    g.unclean.remove(&block);
    g.parked.insert(block);
    if !in_placeholder(block) {
        g.parked_ordinary.insert(block);
    }
}

/// Whether `block` lies in a placeholder.
fn in_placeholder(block: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        MemoryRegionInfo, QueryVirtualMemoryInformation, WIN32_MEMORY_REGION_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // The region's flag for a placeholder reservation, after Private,
    // MappedDataFile, MappedImage, MappedPageFile, MappedPhysical,
    // DirectMapped, SoftwareEnclave and PageSize64K: the SDK's header calls
    // these bits reserved, the kernel fills them (Windows 10 1803 on). A
    // fresh, split or unmapped-to placeholder shows it, an ordinary
    // reservation (even one with no access) does not; `VirtualQuery` tells
    // the two apart by nothing.
    const PLACEHOLDER: u32 = 1 << 8;
    // SAFETY: plain data filled in by the query.
    let mut info: WIN32_MEMORY_REGION_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size; the returned size is
    // optional. Free memory fails the query: no placeholder.
    let ok = unsafe {
        QueryVirtualMemoryInformation(
            GetCurrentProcess(),
            (block * BLOCK) as *const _,
            MemoryRegionInfo,
            (&raw mut info).cast(),
            size_of_val(&info),
            std::ptr::null_mut(),
        )
    } != 0;
    // SAFETY: the flags word of the union, filled in by the query.
    ok && unsafe { info.Anonymous.Flags } & PLACEHOLDER != 0
}

/// Once per [`park_epoch`] submissions: the parked blocks that are no
/// placeholder and were committed since go back to the unclean set, no
/// longer absent, for the next asking that covers them to find written.
/// One question to Windows per uncommitted region.
fn unpark_committed(g: &mut Guards) {
    use std::sync::atomic::Ordering::Relaxed;
    let epoch = SUBMISSIONS.load(Relaxed) / park_epoch();
    if g.parked_at == epoch {
        return;
    }
    g.parked_at = epoch;
    let parked: Vec<usize> = g.parked_ordinary.iter().copied().collect();
    let mut until = 0;
    for block in parked {
        if block >= until {
            VIEW_STATS[4].fetch_add(1, Relaxed);
            // The block itself if it is (partly) committed now.
            until = uncommitted_end(block);
        }
        if block >= until {
            VIEW_STATS[5].fetch_add(1, Relaxed);
            g.parked_ordinary.remove(&block);
            g.parked.remove(&block);
            g.absent.remove(&block);
            g.unclean.insert(block);
        }
    }
}

/// Whether the block `page` is read-only by our hand and unwritten since a
/// copy took it (diagnostics of `VICTORY_BACK_OWN_CHECK`).
pub(crate) fn clean(page: usize) -> bool {
    guards().clean.contains(&page)
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
    let (first, last) = (start / BLOCK, end.div_ceil(BLOCK));
    // With the view set, a range known to be views is one too, whatever
    // its first block's state: a written first block (open, not absent)
    // asked Windows again, the walk to the region's end.
    let known = !*ASK.get_or_init(|| std::env::var_os("VICTORY_GUARD_ASK").is_some_and(|v| v == "1")) && {
        let g = guards();
        g.protected.contains(&first) || g.absent.contains(&first) || (view_set() && covered(&g.views, first, last)) || (view_known() && g.viewed.contains(&first))
    };
    if !known {
        VIEW_STATS[3].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if !is_view(start, end) {
            return None;
        }
    }
    install();
    let mut g = guards();
    if view_set() {
        return Some(guard_unclean(&mut g, first, last));
    }
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
            }
        }
        if open {
            written.extend(block * BLOCK / PAGE..(block + 1) * BLOCK / PAGE);
            if g.protected.contains(&block) {
                // Written through the runtime, still read-only.
                g.clean.insert(block);
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

/// [`guard`] with [`view_set`]: the same answer, from the view blocks that
/// are not clean ([`Guards::unclean`]) instead of every block of the range.
/// A range seen for the first time is noted as views, its blocks not clean
/// yet put in the set (once). Then: none in the range, nothing to do; else
/// each, in order, as the walk over the whole range treats it (one found
/// uncommitted parked; one written through the runtime, or guarded with
/// another view of its page, but still read-only noted clean; the others
/// guarded again in runs).
fn guard_unclean(g: &mut Guards, first: usize, last: usize) -> Vec<usize> {
    use std::sync::atomic::Ordering::Relaxed;
    unpark_committed(g);
    if !covered(&g.views, first, last) {
        let fresh: Vec<usize> = (first..last)
            .filter(|b| !g.clean.contains(b) && !covered(&g.views, *b, *b + 1))
            .collect();
        g.unclean.extend(fresh);
        add_range(&mut g.views, first, last);
    }
    let unclean: Vec<usize> = g.unclean.range(first..last).copied().collect();
    let mut written = Vec::new();
    if unclean.is_empty() {
        VIEW_STATS[0].fetch_add(1, Relaxed);
        return written;
    }
    VIEW_STATS[1].fetch_add(1, Relaxed);
    VIEW_STATS[2].fetch_add(unclean.len() as u64, Relaxed);
    let mut run: Option<(usize, usize)> = None;
    let flush = |g: &mut Guards, run: &mut Option<(usize, usize)>| {
        if let Some((a, b)) = run.take() {
            protect(g, a, b);
        }
    };
    for block in unclean {
        if g.absent.contains(&block) {
            // Found uncommitted before and still in the set (`protect`
            // parks the blocks it finds so): parked while it stays so.
            VIEW_STATS[4].fetch_add(1, Relaxed);
            if block < uncommitted_end(block) {
                park(g, block);
                flush(g, &mut run);
                continue;
            }
            g.absent.remove(&block);
        }
        written.extend(block * BLOCK / PAGE..(block + 1) * BLOCK / PAGE);
        if g.protected.contains(&block) {
            // Written through the runtime, or guarded with another view of
            // its page: still read-only.
            g.clean.insert(block);
            g.unclean.remove(&block);
            flush(g, &mut run);
            continue;
        }
        run = match run {
            Some((a, b)) if b == block => Some((a, block + 1)),
            other => {
                if let Some((a, b)) = other {
                    protect(g, a, b);
                }
                Some((block, block + 1))
            }
        };
    }
    flush(g, &mut run);
    written
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
    use windows_sys::Win32::System::Memory::{PAGE_READONLY, VirtualProtect};
    protect_here(g, from, to);
    if !view_guard() {
        return;
    }
    let mut alias_blocks: Vec<usize> = Vec::new();
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
                if alias != block && !g.protected.contains(&alias) {
                    alias_blocks.push(alias);
                }
            }
        }
    }
    // Blocks that follow one another are made read-only in one call, as
    // `protect_alias` does for each of them one at a time.
    if alias_blocks.len() > 1 {
        alias_blocks.sort_unstable();
        alias_blocks.dedup();
    }
    let mut at = 0;
    while at < alias_blocks.len() {
        let run_start = alias_blocks[at];
        let mut run_end = run_start + 1;
        at += 1;
        while at < alias_blocks.len() && alias_blocks[at] == run_end {
            run_end += 1;
            at += 1;
        }
        let n = run_end - run_start;
        let mut old = 0;
        // SAFETY: title view memory; only its protection changes.
        let set = unsafe {
            VirtualProtect(
                (run_start * BLOCK) as *const _,
                n * BLOCK,
                PAGE_READONLY,
                &mut old,
            )
        };
        if set != 0 {
            g.protected.extend(run_start..run_end);
            if view_known() {
                g.viewed.extend(run_start..run_end);
            }
        } else {
            // A run can span views the kernel only lets be guarded one
            // region at a time, leaving out the ones that fail.
            for block in run_start..run_end {
                protect_alias(g, block);
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
        if view_known() {
            g.viewed.insert(block);
        }
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
        for block in from..to {
            g.unclean.remove(&block);
        }
        if view_known() {
            g.viewed.extend(from..to);
        }
    };
    if set(from, to) {
        note(g, from, to);
        return;
    }
    for block in from..to {
        if set(block, block + 1) {
            note(g, block, block + 1);
        } else {
            #[cfg(feature = "diagnostics")]
            let error = std::io::Error::last_os_error();
            if block < uncommitted_end(block) {
                g.absent.insert(block);
                // Out of the unclean set while it stays so ([`Guards::parked`]).
                if g.unclean.contains(&block) {
                    park(g, block);
                }
            } else {
                // Diagnostics: a committed block that will not be made
                // read-only is reported written at every asking and never
                // known to the guards (one in 1,024 said).
                #[cfg(feature = "diagnostics")]
                {
                    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed).is_multiple_of(1024) {
                        tracing::info!("a block of title memory could not be made read-only (one in 1024): {:#x}: {error}", block * BLOCK);
                    }
                }
            }
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
    // The blocks we hold are sorted, so each run of them that is contiguous
    // is given back in one call: a long write (`fread`) opens hundreds of
    // blocks, and every call walks the page tables.
    let mut at = 0;
    while at < blocks.len() {
        let run_start = blocks[at];
        let mut run_end = run_start + 1;
        at += 1;
        while at < blocks.len() && blocks[at] == run_end {
            run_end += 1;
            at += 1;
        }
        for block in run_start..run_end {
            g.protected.remove(&block);
            g.clean.remove(&block);
            // A view block: not clean ([`Guards::unclean`]).
            if covered(&g.views, block, block + 1) {
                g.unclean.insert(block);
            }
        }
        let n = run_end - run_start;
        let mut old = 0;
        // SAFETY: blocks we made read-only.
        let set = unsafe {
            VirtualProtect(
                (run_start * BLOCK) as *const _,
                n * BLOCK,
                PAGE_READWRITE,
                &mut old,
            )
        };
        if set == 0 && n > 1 {
            // A run can span a placeholder and the views over it, which the
            // kernel only lets be given back one region at a time.
            for block in run_start..run_end {
                let mut old = 0;
                // SAFETY: a block we made read-only.
                unsafe {
                    VirtualProtect((block * BLOCK) as *const _, BLOCK, PAGE_READWRITE, &mut old)
                };
            }
        }
    }
    !blocks.is_empty()
}

/// Before our own write into title memory `[at, at + len)` through the
/// kernel (which does not fault into our handler): opens its guarded blocks.
/// The runtime calls it (as `kernel_will_write`) before the title's kernel
/// reads into its own memory.
pub fn writing(at: usize, len: usize) {
    // The length is the title's (`fread`'s size times count, saturated).
    open(&mut guards(), at, at.saturating_add(len));
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
static MAPPING: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// What the runtime holds while it changes what title memory shows
/// ([`title_memory_remapping`]).
pub struct Remapping(#[allow(dead_code)] std::sync::RwLockWriteGuard<'static, ()>, std::time::Instant);

impl Drop for Remapping {
    fn drop(&mut self) {
        // From asking for the lock until it is let go (the frame log).
        crate::framelog::add(crate::framelog::T::RemapUs, self.1.elapsed().as_micros() as u64);
    }
}

/// The runtime is about to map, unmap or move physical pages of title
/// memory: until what it returns is dropped, no read of title memory in
/// place is under way and none begins ([`title_memory_reading`]). Taken
/// around the whole change, the unmapping and the mapping again.
pub fn title_memory_remapping() -> Remapping {
    let asked = std::time::Instant::now();
    let held = MAPPING.write().unwrap_or_else(|e| e.into_inner());
    crate::framelog::add(crate::framelog::T::Remaps, 1);
    crate::framelog::add(crate::framelog::T::RemapWaitUs, asked.elapsed().as_micros() as u64);
    Remapping(held, asked)
}

/// What a read of title memory in place holds ([`title_memory_reading`]).
pub struct Reading(#[allow(dead_code)] std::sync::RwLockReadGuard<'static, ()>);

/// For reading title memory in place: until what it returns is dropped,
/// the runtime changes no mapping, so memory found mapped stays mapped.
/// Not to be taken twice by one thread, nor held across anything that
/// maps title memory.
pub fn title_memory_reading() -> Reading {
    Reading(MAPPING.read().unwrap_or_else(|e| e.into_inner()))
}

/// [`title_memory_reading`] if it can be had at once: `None` while the
/// runtime changes a mapping (or waits to), for a caller that has another
/// way to read and would rather not wait.
pub fn title_memory_reading_now() -> Option<Reading> {
    match MAPPING.try_read() {
        Ok(guard) => Some(Reading(guard)),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(Reading(e.into_inner())),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
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
    let absent: Vec<usize> = g.absent.range(range.clone()).copied().collect();
    for block in absent {
        g.absent.remove(&block);
    }
    let blocks: Vec<usize> = g.protected.range(range.clone()).copied().collect();
    for block in blocks {
        g.protected.remove(&block);
        g.clean.remove(&block);
    }
    let aliased: Vec<usize> = g.aliased.range(at / BLOCK..at.saturating_add(len).div_ceil(BLOCK)).copied().collect();
    for block in aliased {
        g.aliased.remove(&block);
    }
    if view_known() {
        // The runtime calls this for two things only (`xmem`: `map` and
        // `unmap`): a range it has just mapped a section's pages into,
        // and a view it is about to make a placeholder again. Either way
        // what stands there is view memory or a hole in it, which is what
        // `is_view` would answer for a range that begins there; and the
        // write watch's asking can only fail there. The title remaps some
        // 83 times a frame, and each time the blocks were forgotten, so
        // that the next asking about each range went to the kernel twice
        // (run 235: 266 to 565 failed askings a frame with the blocks
        // remembered from their guards alone, no fewer than without).
        g.viewed.extend(at / BLOCK..at.saturating_add(len).div_ceil(BLOCK));
        return;
    }
    // What the range shows is new: no longer known to be a view.
    let viewed: Vec<usize> = g.viewed.range(at / BLOCK..at.saturating_add(len).div_ceil(BLOCK)).copied().collect();
    for block in viewed {
        g.viewed.remove(&block);
    }
    // Nor parked: the views below go back to the unclean set.
    let parked: Vec<usize> = g.parked.range(range.clone()).copied().collect();
    for block in parked {
        g.parked.remove(&block);
        g.parked_ordinary.remove(&block);
    }
    // View blocks there are not clean any more.
    let from = g.views.partition_point(|r| r.1 <= range.start);
    let views: Vec<usize> = g.views[from..]
        .iter()
        .take_while(|r| r.0 < range.end)
        .flat_map(|&(a, b)| a.max(range.start)..b.min(range.end))
        .collect();
    g.unclean.extend(views);
}

/// The title committed `[at, at + len)` through the runtime
/// (`XMemVirtualAlloc`): blocks of it found uncommitted before are not
/// known so any more, and the next asking that covers them finds them
/// written. A commit no one tells of is found at the next look at the
/// parked blocks that are no placeholder ([`Guards::parked`]).
pub fn title_memory_committed(at: usize, len: usize) {
    let mut g = guards();
    let range = at / BLOCK..at.saturating_add(len).div_ceil(BLOCK);
    let parked: Vec<usize> = g.parked.range(range.clone()).copied().collect();
    for block in parked {
        g.parked.remove(&block);
        g.parked_ordinary.remove(&block);
        g.unclean.insert(block);
    }
    let absent: Vec<usize> = g.absent.range(range).copied().collect();
    for block in absent {
        g.absent.remove(&block);
    }
}

/// Registers the handler that takes writes to guarded blocks, once, ahead
/// of the crash reporter's.
fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, EXCEPTION_GUARD_PAGE, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
        EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_write(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        let r = unsafe { &*(*info).ExceptionRecord };
        const WRITE: usize = 1;
        const IN_PAGE_ERROR: NTSTATUS = 0xC000_0006_u32 as NTSTATUS;
        let fault = r.ExceptionCode == EXCEPTION_ACCESS_VIOLATION as NTSTATUS || r.ExceptionCode == IN_PAGE_ERROR;
        // A PAGE_GUARD exception is recoverable only at our copy instruction.
        // Leave guard faults outside it to the remaining Windows handlers.
        // shortcut: Windows clears a guard at its first touch, so a page our
        // copy touched has lost its guard (and the kernel's copy that follows
        // would clear it too); re-arm it if a title is found that relies on
        // its guard pages there.
        if r.ExceptionCode == EXCEPTION_GUARD_PAGE
            || (fault && (r.NumberParameters < 2 || r.ExceptionInformation[0] != WRITE))
        {
            // SAFETY: the faulting thread's context, for this exception.
            return if unsafe { copy_failed(&mut *(*info).ContextRecord) } {
                EXCEPTION_CONTINUE_EXECUTION
            } else {
                EXCEPTION_CONTINUE_SEARCH
            };
        }
        if !fault {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let at = r.ExceptionInformation[1];
        // Held across both checks: another thread may have opened the block
        // since this one faulted, and a copy may have guarded it again since
        // (then it is ours again, not read-only for some other reason).
        let mut g = guards();
        if open(&mut g, at, at + 1) {
            #[cfg(feature = "diagnostics")]
            crate::copystat::add(crate::copystat::C::GuardFaults, 1);
            crate::framelog::add(crate::framelog::T::GuardFaults, 1);
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        if writable(at) {
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        drop(g);
        // SAFETY: the faulting thread's context, for this exception.
        if unsafe { copy_failed(&mut *(*info).ContextRecord) } {
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

// Flamingo: a copy that takes an access violation where a page is not there
// returns false instead ([`try_copy`]). `rep movsb` is the only instruction
// that can fault; [`copy_failed`] sends a fault there to the false return.
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl flamingo_try_copy",
    ".def flamingo_try_copy; .scl 2; .type 32; .endef",
    ".seh_proc flamingo_try_copy",
    "flamingo_try_copy:",
    "push rdi",
    ".seh_pushreg rdi",
    "push rsi",
    ".seh_pushreg rsi",
    ".seh_endprologue",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    ".globl flamingo_try_copy_body",
    "flamingo_try_copy_body:",
    "rep movsb",
    ".globl flamingo_try_copy_end",
    "flamingo_try_copy_end:",
    "mov eax, 1",
    "pop rsi",
    "pop rdi",
    "ret",
    ".globl flamingo_try_copy_fault",
    "flamingo_try_copy_fault:",
    "xor eax, eax",
    "pop rsi",
    "pop rdi",
    "ret",
    ".seh_endproc",
);

unsafe extern "C" {
    fn flamingo_try_copy(dst: *mut u8, src: *const u8, len: usize) -> u32;
    static flamingo_try_copy_body: u8;
    static flamingo_try_copy_end: u8;
    static flamingo_try_copy_fault: u8;
}

/// If the fault is [`try_copy`]'s, sends the thread to its false return.
fn copy_failed(context: &mut windows_sys::Win32::System::Diagnostics::Debug::CONTEXT) -> bool {
    let rip = context.Rip as usize;
    let body = &raw const flamingo_try_copy_body as usize;
    let end = &raw const flamingo_try_copy_end as usize;
    if rip < body || rip >= end {
        return false;
    }
    context.Rip = &raw const flamingo_try_copy_fault as u64;
    true
}

/// Copies `len` bytes in this process with a plain copy, without the
/// kernel call (`ReadProcessMemory`, about a microsecond a call: 12,000
/// small title copies a frame took 11 ms of the submitting thread, run 37);
/// false if a page of either side is not there or not writable, and then
/// the destination may hold part of the bytes, as after a failed kernel
/// copy. A write to a block our handler made read-only is taken by it
/// first, as for the title's writes. `FLAMINGO_PLAIN_COPIES=0` always says
/// false (the kernel copies).
pub(crate) fn try_copy(dst: *mut u8, src: *const u8, len: usize) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| {
        install();
        std::env::var_os("FLAMINGO_PLAIN_COPIES").is_none_or(|v| v != "0")
    }) {
        return false;
    }
    // SAFETY: a fault in the copy returns false through our handler; the
    // caller vouches that the destination may be written where mapped.
    unsafe { flamingo_try_copy(dst, src, len) != 0 }
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
        let (_, before) = writes(start, end, false, 0).expect("watched");
        assert!(before.is_empty());
        // SAFETY: committed pages of ours.
        unsafe { ((p + PAGE + 8) as *mut u8).write(1) };
        let (now, after) = writes(start, end, false, 0).expect("watched");
        assert_eq!(after, [(p / PAGE + 1, now)]);
        // Flamingo: what an asking after the write's lists is not news.
        assert!(writes(start, end, false, now).expect("watched").1.is_empty());
        // Another asker after the kernel's bit was cleared still sees it.
        let (later, again) = writes(p + PAGE, p + 2 * PAGE, false, 0).expect("watched");
        assert!(later > now);
        assert_eq!(again, [(p / PAGE + 1, now)]);
        let mut plain = vec![0u8; PAGE];
        assert!(
            writes(
                plain.as_mut_ptr() as usize,
                plain.as_ptr() as usize + PAGE,
                true,
                0
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
        // With the view set: found at the next look at the blocks parked as
        // uncommitted that are no placeholder.
        SUBMISSIONS.fetch_add(park_epoch(), std::sync::atomic::Ordering::Relaxed);
        assert_eq!(refresh(), [early / PAGE, late / PAGE]);
    }

    /// Blocks found uncommitted are parked: not asked about again until the
    /// title commits them through the runtime (found at once) or the next
    /// look at the parked blocks that are no placeholder (a commit no one
    /// tells of).
    #[test]
    fn parked_blocks_come_back_when_committed() {
        if !view_set() {
            return;
        }
        // SAFETY: a fresh reservation of two blocks, nothing committed.
        let p = unsafe { VirtualAlloc(std::ptr::null(), 2 * BLOCK, MEM_RESERVE, PAGE_READWRITE) } as usize;
        assert_ne!(p, 0);
        let pages = |block: usize| (p + block * BLOCK) / PAGE..(p + (block + 1) * BLOCK) / PAGE;
        let all: Vec<usize> = (0..2).flat_map(pages).collect();
        // Nothing there: a view, its blocks reported once, then parked.
        assert_eq!(guard(p, p + 2 * BLOCK), Some(all));
        assert_eq!(guard(p, p + 2 * BLOCK), Some(Vec::new()));
        {
            // No placeholder: looked at again each epoch.
            let g = guards();
            assert!(g.parked.contains(&(p / BLOCK)) && g.parked_ordinary.contains(&(p / BLOCK)));
        }
        // SAFETY: the first block of our reservation.
        assert_eq!(unsafe { VirtualAlloc(p as _, BLOCK, MEM_COMMIT, PAGE_READWRITE) } as usize, p);
        title_memory_committed(p, BLOCK);
        assert_eq!(guard(p, p + 2 * BLOCK), Some(pages(0).collect()));
        assert_eq!(guard(p, p + 2 * BLOCK), Some(Vec::new()));
        // SAFETY: the second block of our reservation.
        assert_eq!(unsafe { VirtualAlloc((p + BLOCK) as _, BLOCK, MEM_COMMIT, PAGE_READWRITE) } as usize, p + BLOCK);
        SUBMISSIONS.fetch_add(park_epoch(), std::sync::atomic::Ordering::Relaxed);
        assert_eq!(guard(p, p + 2 * BLOCK), Some(pages(1).collect()));
        assert_eq!(guard(p, p + 2 * BLOCK), Some(Vec::new()));
        title_memory_remapped(p, 2 * BLOCK);
        // SAFETY: our reservation, no longer known to the guards.
        unsafe { VirtualFree(p as _, 0, MEM_RELEASE) };
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
        if view_set() {
            // Placeholders: parked until the runtime maps them, never asked
            // about again meanwhile.
            let g = guards();
            for block in [1, 2].map(|k| p / BLOCK + k) {
                assert!(g.parked.contains(&block) && !g.parked_ordinary.contains(&block));
            }
        }
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
mod try_copy_tests {
    use super::{copy_failed, try_copy};

    #[test]
    fn guard_pages_in_the_copy_report_failure() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_GUARD, PAGE_READWRITE, VirtualAlloc, VirtualFree,
            VirtualProtect,
        };

        let src = [42u8];
        let mut dst = [0u8];
        // SAFETY: the new committed page is writable until explicitly guarded, then
        // protected and released only by this test. Copies are exactly one byte.
        unsafe {
            let page = VirtualAlloc(
                std::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            ) as *mut u8;
            assert!(!page.is_null());
            page.write(17);
            let mut previous = 0;
            assert_ne!(VirtualProtect(page.cast(), 4096, PAGE_READWRITE | PAGE_GUARD, &mut previous), 0);
            assert!(!try_copy(dst.as_mut_ptr(), page, 1));
            assert_eq!(dst, [0]);
            assert!(try_copy(dst.as_mut_ptr(), page, 1));
            assert_eq!(dst, [17]);

            assert_ne!(VirtualProtect(page.cast(), 4096, PAGE_READWRITE | PAGE_GUARD, &mut previous), 0);
            assert!(!try_copy(page, src.as_ptr(), 1));
            assert_eq!(page.read(), 17);
            assert!(try_copy(page, src.as_ptr(), 1));
            assert_eq!(page.read(), 42);
            assert_ne!(VirtualFree(page.cast(), 0, MEM_RELEASE), 0);
        }
    }

    #[test]
    fn guard_page_recovery_leaves_unrelated_instruction_pointers_alone() {
        use windows_sys::Win32::System::Diagnostics::Debug::CONTEXT;

        // SAFETY: CONTEXT is a plain C record. The address check uses only Rip
        // and never executes the artificial address.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rip = (&raw const super::flamingo_try_copy_end) as u64;
        assert!(!copy_failed(&mut context));
        assert_eq!(context.Rip, (&raw const super::flamingo_try_copy_end) as u64);
    }

    /// A plain copy copies mapped memory, and says false for a page that
    /// is only reserved (the copy faults there) without the process dying.
    #[test]
    fn copies_mapped_memory_and_refuses_reserved_pages() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        let src: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
        let mut dst = vec![0u8; 10_000];
        assert!(try_copy(dst.as_mut_ptr(), src.as_ptr(), src.len()));
        assert_eq!(dst, src);
        // SAFETY: plain reservations, released below.
        unsafe {
            let p = VirtualAlloc(std::ptr::null(), 2 << 16, MEM_RESERVE, PAGE_READWRITE) as *mut u8;
            assert!(!p.is_null());
            assert!(!VirtualAlloc(p.cast(), 4096, MEM_COMMIT, PAGE_READWRITE).is_null());
            // Reading across into the reserved page.
            assert!(!try_copy(dst.as_mut_ptr(), p, 8192));
            // Writing into it.
            assert!(!try_copy(p.add(4096), src.as_ptr(), 100));
            // The committed page still copies.
            assert!(try_copy(p, src.as_ptr(), 4096));
            assert_eq!(std::slice::from_raw_parts(p, 4096), &src[..4096]);
            assert_ne!(VirtualFree(p.cast(), 0, MEM_RELEASE), 0);
        }
    }
}

#[cfg(test)]
mod cold_tests {
    use super::{COLD_AFTER, Journal, cold_note, cold_ready};
    use std::collections::BTreeMap;

    fn journal() -> Journal {
        Journal {
            asked: 0,
            written: BTreeMap::new(),
            found: Vec::new(),
            ahead: Vec::new(),
            ahead_asked: 0,
            kept: false,
            cold: BTreeMap::new(),
        }
    }

    /// A run found unwritten `COLD_AFTER` times in a row is asked once a
    /// frame; one written page found makes it asked every time again.
    #[test]
    fn cold_runs_are_asked_once_a_frame() {
        let mut j = journal();
        for _ in 0..COLD_AFTER - 1 {
            cold_note(&mut j, 10, 20, 5, false);
        }
        assert!(!cold_ready(&j, 10, 20, 5), "not cold yet");
        cold_note(&mut j, 10, 20, 5, false);
        assert!(cold_ready(&j, 10, 20, 5), "cold: the frame's other askings are left out");
        assert!(!cold_ready(&j, 10, 20, 6), "the next frame asks once");
        assert!(!cold_ready(&j, 10, 30, 5), "another run is its own");
        cold_note(&mut j, 10, 20, 6, true);
        assert!(!cold_ready(&j, 10, 20, 6), "written: asked every time again");
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::{title_memory_reading, title_memory_remapping};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    /// A change of mapping waits for a read in place that is under way, and
    /// a read in place waits for a change of mapping.
    #[test]
    fn reads_in_place_and_remappings_wait_for_each_other() {
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
}

#[cfg(test)]
mod wrote_tests {
    use super::{PAGE, ask_once, ask_once_end, asked, forget_views, writes, wrote};

    #[test]
    fn wrote_keeps_ahead_asked_at_or_above_written_timestamps() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        const MEM_WRITE_WATCH: u32 = 0x0020_0000;
        // SAFETY: private write-watched 64 KiB page owned by this test.
        let p = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                PAGE,
                MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        } as usize;
        assert_ne!(p, 0);

        forget_views(p, p + PAGE);
        // The journal is the process's: another test's submission can end
        // or replace the asking that stands between these calls (the suite
        // runs tests on several threads). An attempt nothing came between
        // is taken; a wrong journal fails every attempt.
        let mut attempt = None;
        for _ in 0..50 {
            ask_once(&[(p, p + PAGE)]);
            wrote(p, PAGE);
            let mine = writes(p, p + PAGE, false, 0).expect("write-watched page");
            let ahead = asked(p, p + PAGE);
            ask_once_end();
            if let Some(ahead) = ahead.filter(|a| a.0 == mine.0) {
                attempt = Some((mine, ahead));
                break;
            }
        }
        let ((now, written), (asked_now, asked_written)) = attempt.expect("in ask_once range, with no other asking between");

        // SAFETY: freeing our private reservation.
        unsafe { VirtualFree(p as *mut _, 0, MEM_RELEASE) };

        assert_eq!(now, asked_now);
        let page = p / PAGE;
        let &(_, w) = written
            .iter()
            .find(|&&(pg, _)| pg == page)
            .expect("page recorded by wrote()");
        assert_eq!(asked_written, written);
        assert!(
            now >= w,
            "writes() returned now={now} < written timestamp w={w} after wrote()"
        );
    }
}
