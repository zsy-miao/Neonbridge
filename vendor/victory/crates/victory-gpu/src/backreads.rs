//! Diagnostics: who touches what the copy-back stored
//! (`VICTORY_BACK_READS`; off unless set; diagnostics builds only).
//!
//! What shaders wrote is fetched from the card and merged into title
//! memory after every submission (`Shadow::card_merge_relay`,
//! `Shadow::relay_merge`), 41 to 59 MB a frame in the first room, whether
//! or not anything reads those bytes there afterwards. This module counts,
//! per frame and per host copy, what happens to each stored 64 KiB page of
//! title memory between one store and the next: which reader of ours took
//! it, whether the title wrote it, and (for one page in [`SAMPLE`], view
//! memory stored through a view of our own only) whether ANYTHING touched
//! it through the title's addresses.
//!
//! The touch is seen without changing a byte or a protection. After the
//! store the 4 KiB pages of the stored stretch are taken out of the
//! process's working set at every title address that shows them
//! (`VirtualUnlock` of pages that are not locked: the documented way, as
//! `crate::pipeline::library::trim` does for the pipeline file). The bytes
//! stay in physical memory (our own view of the section holds the same
//! pages valid). The next access through a title address, by the title, by
//! a reader of ours or by the kernel on someone's behalf, is a soft fault
//! that Windows resolves by itself and that puts the page back in the
//! working set, where `QueryWorkingSetEx` shows it. Nothing faults into a
//! handler, a kernel call that reads or writes the page succeeds as
//! before, a read-only guard stays read-only and still takes the title's
//! writes. Measured on this PC (a probe outside the game): 5.4
//! us to trim a 64 KiB piece beside six running threads, 2.8 us to ask
//! about its 16 pages, 0.89 us for each 4 KiB page somebody then reads.
//!
//! Values: `1` counts only (stores, readers of ours, title writes; no
//! page leaves the working set); `2` also looks at one page in [`SAMPLE`];
//! `alt` is twenty seconds of `2` and twenty of `1` in turn, each change
//! said in the log, so that the look's own cost shows in the frame log.
//!
//! What a look cannot tell: WHO touched. A reader of ours is told apart by
//! the hooks in `crate::host` (`read_into`, `copy_mapped`, `write_mapped`);
//! a touch with no hook and no write is "something else", which is the
//! title's own reads at most (it also holds readers of ours that read in
//! place with no hook: a texture's refresh in place, the rectangle
//! vertices of `crate::draw`, and host libraries called by the title).

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::watch::PAGE;

const KERNEL_PAGE: usize = 4096;
/// One stored page in this many is looked at (`2`, `alt`).
const SAMPLE: u64 = 8;
/// A stored page not stored again within this many frames is closed.
const KEEP: u64 = 8;
/// Frames between two summary lines; the buffers' table follows every
/// eighth of them.
const EVERY: f64 = 256.0;

/// Readers of ours, as bits of `Seen::layer`.
pub(crate) const MERGE: u8 = 0;
pub(crate) const REFRESH: u8 = 1;
const REFRESH_OTHER: u8 = 2;
pub(crate) const UNDER: u8 = 3;
pub(crate) const TITLE_COPY: u8 = 4;
const STORE: u8 = 5;
const OTHER: u8 = 6;
const WHO: usize = 7;

/// 0 off, 1 counted only, 2 with looks, 3 in turn.
fn setting() -> u8 {
    static MODE: OnceLock<u8> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_BACK_READS").as_deref().map(str::trim) {
        Ok("1") => 1,
        Ok("2") => 2,
        Ok("alt") => 3,
        _ => 0,
    })
}

/// Whether anything is counted at all.
pub(crate) fn on() -> bool {
    setting() != 0
}

/// Whether stored pages are looked at now.
fn looking() -> bool {
    match setting() {
        2 => true,
        3 => {
            static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
            let on = (EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs() / 20) % 2 == 1;
            static LAST: AtomicU8 = AtomicU8::new(2);
            if LAST.swap(u8::from(on), Relaxed) != u8::from(on) {
                tracing::info!(
                    "pages that came back are looked at for a touch from now (VICTORY_BACK_READS=alt): {}",
                    if on { "ON" } else { "off, counted only" }
                );
            }
            on
        }
        _ => false,
    }
}

/// Whether `page` is one of the pages looked at in `frame`: one in
/// [`SAMPLE`] by its number, another set every 512 frames.
fn sampled(page: usize, frame: u64) -> bool {
    ((page as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) % SAMPLE == (frame / 512) % SAMPLE
}

/// Sums, of one host copy or of all.
#[derive(Default, Clone, Copy)]
struct Tally {
    /// Stretches stored, their bytes, those stored through a view of our
    /// own and their bytes, pages stored for the first time in their
    /// frame, and stores of a page stored a frame before or in the same.
    stores: u64,
    bytes: u64,
    own: u64,
    own_bytes: u64,
    pages: u64,
    again: u64,
    /// Stores followed to their end, by which reader of ours took them
    /// meanwhile ([`MERGE`]..), those whose guard was clean right after
    /// the store, and of those the ones the title wrote.
    ended: u64,
    who: [u64; WHO],
    clean: u64,
    written: u64,
    /// Stores whose pages were taken out of the working set, and those
    /// looked at afterwards.
    trimmed: u64,
    looked: u64,
    /// Of the looked at: nothing touched them (and their bytes); the
    /// title wrote; readers of ours and no write; something else (and
    /// their bytes).
    untouched: u64,
    untouched_bytes: u64,
    wrote: u64,
    ours: u64,
    other: u64,
    other_bytes: u64,
    /// Their 4 KiB pages: touched, asked about.
    small: [u64; 2],
    /// Ends: no store of the page again within [`KEEP`] frames; the
    /// runtime remapped the page; no look because a mapping was changing.
    late: u64,
    remapped: u64,
    busy: u64,
}

/// A stored page, from its store to the next one.
struct Seen {
    /// The copy that stored: its number, and its title buffer's start and
    /// bytes.
    id: u64,
    copy: (u64, u64),
    /// The page's number at the address stored at.
    page: usize,
    frame: u64,
    bytes: u64,
    /// The stretch as whole 4 KiB pages: offsets in the 64 KiB page.
    from: usize,
    to: usize,
    /// The page's address in every title view that shows it.
    shown: Vec<usize>,
    /// Not ended yet.
    open: bool,
    /// Stored through a view of our own.
    own: bool,
    trimmed: bool,
    /// The guard of the page was clean right after the store, and the
    /// journal's number for the page then.
    clean: bool,
    stamp: u64,
    /// Readers of ours since the store.
    layer: u8,
}

/// What an ended store was found to be.
struct Judged {
    written: bool,
    /// 4 KiB pages touched, asked about.
    looked: Option<(u32, u32)>,
    busy: bool,
}

#[derive(Default)]
struct State {
    /// By page number (the address of the last store).
    seen: HashMap<usize, Seen>,
    /// Every title page number of a seen page: its key in `seen`.
    names: HashMap<usize, usize>,
    /// By copy (start, bytes), since the start.
    buffers: HashMap<(u64, u64), Tally>,
    since: Tally,
    all: Tally,
    frames: f64,
    lines: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A bit per title page number (its low 18 bits): set while the page may
/// be a seen one. What the hooks look at first, without a lock.
const BITS: usize = 1 << 18;
static TRACKED: [AtomicU64; BITS / 64] = [const { AtomicU64::new(0) }; BITS / 64];

fn bit(page: usize) -> (usize, u64) {
    let b = page & (BITS - 1);
    (b / 64, 1u64 << (b % 64))
}

fn mark(page: usize) {
    let (word, b) = bit(page);
    TRACKED[word].fetch_or(b, Relaxed);
}

fn marked(page: usize) -> bool {
    let (word, b) = bit(page);
    TRACKED[word].load(Relaxed) & b != 0
}

thread_local! {
    /// Which reader of ours this thread is now, and for which copy.
    static AS: Cell<(u8, u64)> = const { Cell::new((OTHER, 0)) };
}

/// Puts back what the thread was reading as.
pub(crate) struct Scope((u8, u64));

impl Drop for Scope {
    fn drop(&mut self) {
        AS.with(|a| a.set(self.0));
    }
}

/// Until what it returns is dropped, this thread's reads of title memory
/// count as `who`'s, for copy number `copy`.
pub(crate) fn reading_as(who: u8, copy: u64) -> Option<Scope> {
    if !on() {
        return None;
    }
    Some(Scope(AS.with(|a| a.replace((who, copy)))))
}

fn each(s: &mut State, copy: (u64, u64), f: impl Fn(&mut Tally)) {
    f(&mut s.since);
    f(&mut s.all);
    f(s.buffers.entry(copy).or_default());
}

/// The 4 KiB pages of the stretch found in the working set at any of the
/// page's title addresses, and how many there are; `None` if Windows did
/// not answer.
fn valid(seen: &Seen) -> Option<(u32, u32)> {
    use windows_sys::Win32::System::ProcessStatus::{K32QueryWorkingSetEx, PSAPI_WORKING_SET_EX_INFORMATION};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let pages = ((seen.to - seen.from) / KERNEL_PAGE).min(16);
    let mut touched = [false; 16];
    for &base in &seen.shown {
        // SAFETY: plain data.
        let mut info: [PSAPI_WORKING_SET_EX_INFORMATION; 16] = unsafe { std::mem::zeroed() };
        for (i, entry) in info[..pages].iter_mut().enumerate() {
            entry.VirtualAddress = (base + seen.from + i * KERNEL_PAGE) as *mut _;
        }
        // SAFETY: `pages` entries of the size given; our own process. The
        // call reads page table state and touches no page.
        let asked = unsafe {
            K32QueryWorkingSetEx(
                GetCurrentProcess(),
                info.as_mut_ptr().cast(),
                (pages * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32,
            )
        } != 0;
        if !asked {
            return None;
        }
        for (i, entry) in info[..pages].iter().enumerate() {
            // Bit 0 of the answer: the page is in the working set.
            // SAFETY: plain data the query filled in.
            if unsafe { entry.VirtualAttributes.Flags } & 1 != 0 {
                touched[i] = true;
            }
        }
    }
    Some((touched[..pages].iter().filter(|&&t| t).count() as u32, pages as u32))
}

/// Takes the stretch's 4 KiB pages out of the working set at every title
/// address of the page; false, with nothing done, while the runtime
/// changes a mapping.
fn trim(shown: &[usize], from: usize, to: usize) -> bool {
    use windows_sys::Win32::System::Memory::VirtualUnlock;
    // No mapping changes until the addresses have been used.
    let Some(_still) = crate::watch::title_memory_reading_now() else {
        return false;
    };
    for &base in shown {
        // SAFETY: only the working set changes. Unlocking pages that are
        // not locked fails, as expected, and takes them out of it; an
        // address with nothing mapped fails and does nothing. Each range
        // lies inside one 64 KiB page, so inside one view.
        unsafe { VirtualUnlock((base + from) as *const _, to - from) };
    }
    true
}

/// Looks at an ended store, with no lock of ours held: whether the title
/// wrote the page (a store through our own view that left the guard
/// clean: the guard is open now, or the journal found the page written
/// since), and which of its trimmed pages are back in the working set.
fn judge(seen: &Seen) -> Judged {
    let written = seen.own
        && seen.clean
        && (!crate::watch::clean(seen.page) || crate::watch::stamp(seen.page) > seen.stamp);
    let looked = if seen.trimmed {
        match crate::watch::title_memory_reading_now() {
            Some(_still) => valid(seen),
            None => None,
        }
    } else {
        None
    };
    Judged { written, looked, busy: seen.trimmed && looked.is_none() }
}

/// Counts an ended store. `how`: 0 the page's next store, 1 none within
/// [`KEEP`] frames, 2 a remapping.
fn tally(s: &mut State, seen: &Seen, judged: &Judged, how: u8) {
    let (layer, bytes, guarded) = (seen.layer, seen.bytes, seen.own && seen.clean);
    let (written, looked, busy) = (judged.written, judged.looked, judged.busy);
    each(s, seen.copy, |t| {
        t.ended += 1;
        for (who, count) in t.who.iter_mut().enumerate() {
            if layer & (1 << who) != 0 {
                *count += 1;
            }
        }
        if guarded {
            t.clean += 1;
        }
        if written {
            t.written += 1;
        }
        match how {
            1 => t.late += 1,
            2 => t.remapped += 1,
            _ => {}
        }
        if busy {
            t.busy += 1;
        }
        if let Some((touched, asked)) = looked {
            t.looked += 1;
            t.small[0] += u64::from(touched);
            t.small[1] += u64::from(asked);
            if touched == 0 {
                t.untouched += 1;
                t.untouched_bytes += bytes;
            } else if written {
                t.wrote += 1;
            } else if layer != 0 {
                t.ours += 1;
            } else {
                t.other += 1;
                t.other_bytes += bytes;
            }
        }
    });
}

/// A merge is about to read title memory at `at` and may store there: the
/// page's last store ends here, before our own read touches the page.
/// Called by the thread that copies back, ahead of the merge's read.
pub(crate) fn before(at: usize) {
    if !on() {
        return;
    }
    let page = at / PAGE;
    if !marked(page) {
        return;
    }
    let (key, mut seen) = {
        let mut guard = state();
        let Some(s) = guard.as_mut() else {
            return;
        };
        let Some(&key) = s.names.get(&page) else {
            return;
        };
        if !s.seen.get(&key).is_some_and(|seen| seen.open) {
            return;
        }
        let Some(seen) = s.seen.remove(&key) else {
            return;
        };
        (key, seen)
    };
    let judged = judge(&seen);
    let mut guard = state();
    let Some(s) = guard.as_mut() else {
        return;
    };
    tally(s, &seen, &judged, 0);
    seen.open = false;
    // Kept, closed, for "stored again within a frame".
    s.seen.entry(key).or_insert(seen);
}

/// A merge of copy `id` (title buffer `copy`: start, bytes) stored
/// `[at, at + n)` of title memory, inside one 64 KiB page; `own`: through
/// a view of our own (`Shadow::put_back_own`), else through the kernel at
/// the title's address.
pub(crate) fn stored(id: u64, copy: (u64, u64), at: usize, n: usize, own: bool) {
    if !on() || n == 0 {
        return;
    }
    let page = at / PAGE;
    let base = page * PAGE;
    let from = (at - base) & !(KERNEL_PAGE - 1);
    let to = (at - base + n).min(PAGE).next_multiple_of(KERNEL_PAGE);
    let frame = crate::frame::current();
    // Asked with no lock of ours held.
    let mut shown = crate::inplace::pages_of(base);
    let many = shown.len() > crate::watch::MANY_VIEWS;
    if shown.is_empty() || many {
        shown = vec![base];
    }
    let (clean, stamp) = if own { (crate::watch::clean(page), crate::watch::stamp(page)) } else { (false, 0) };
    // Only what went through our own view: its pages stay in physical
    // memory whatever happens to the title's addresses (our view holds
    // them), and the guard tells the title's writes there.
    let trimmed = own && !many && looking() && sampled(page, frame) && trim(&shown, from, to);
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    let key = s.names.get(&page).copied().unwrap_or(page);
    let old = s.seen.remove(&key);
    let (again, first) = match &old {
        Some(old) => (old.frame + 1 >= frame, old.frame != frame),
        None => (false, true),
    };
    if let Some(old) = old {
        if old.open {
            // A store with no `before` ahead of it: ended unlooked.
            tally(s, &old, &Judged { written: false, looked: None, busy: old.trimmed }, 0);
        }
        for a in &old.shown {
            s.names.remove(&(a / PAGE));
        }
    }
    each(s, copy, |t| {
        t.stores += 1;
        t.bytes += n as u64;
        if own {
            t.own += 1;
            t.own_bytes += n as u64;
        }
        if first {
            t.pages += 1;
        }
        if again {
            t.again += 1;
        }
        if trimmed {
            t.trimmed += 1;
        }
    });
    for a in &shown {
        s.names.insert(a / PAGE, page);
        mark(a / PAGE);
    }
    s.seen.insert(
        page,
        Seen { id, copy, page, frame, bytes: n as u64, from, to, shown, open: true, own, trimmed, clean, stamp, layer: 0 },
    );
}

/// A reader of ours reads title memory `[at, at + len)` (`store`: it
/// writes there through the kernel, as a merge does the old way). The
/// seen pages whose stored stretch it meets are noted read by it.
pub(crate) fn touched(at: usize, len: usize, store: bool) {
    if !on() || len == 0 {
        return;
    }
    let end = at.saturating_add(len);
    let (first, last) = (at / PAGE, (end - 1) / PAGE);
    if last - first > 1 << 16 || !(first..=last).any(marked) {
        return;
    }
    let (who, copy) = AS.with(|a| a.get());
    let mut guard = state();
    let Some(s) = guard.as_mut() else {
        return;
    };
    for page in first..=last {
        let Some(&key) = s.names.get(&page) else {
            continue;
        };
        let Some(seen) = s.seen.get_mut(&key) else {
            continue;
        };
        if !seen.open {
            continue;
        }
        let base = page * PAGE;
        let (a, b) = (at.max(base) - base, end.min(base + PAGE) - base);
        if a >= seen.to || b <= seen.from {
            continue;
        }
        let who = if store {
            STORE
        } else if who == REFRESH && copy != seen.id {
            REFRESH_OTHER
        } else {
            who
        };
        seen.layer |= 1 << who;
    }
}

/// The runtime mapped or unmapped title memory `[at, at + len)`
/// (`crate::watch::title_memory_remapped`): the seen pages there end,
/// unlooked (their addresses show something else from now on).
pub(crate) fn remapped(at: usize, len: usize) {
    if !on() || len == 0 {
        return;
    }
    let (first, last) = (at / PAGE, (at.saturating_add(len) - 1) / PAGE);
    let mut guard = state();
    let Some(s) = guard.as_mut() else {
        return;
    };
    if s.names.is_empty() {
        return;
    }
    let keys: Vec<usize> = if last - first < s.names.len() {
        (first..=last).filter_map(|page| s.names.get(&page).copied()).collect()
    } else {
        s.names.iter().filter(|(page, _)| (first..=last).contains(*page)).map(|(_, key)| *key).collect()
    };
    for key in keys {
        // (A key may be named twice: two addresses of one page.)
        let Some(seen) = s.seen.remove(&key) else {
            continue;
        };
        if seen.open {
            tally(s, &seen, &Judged { written: false, looked: None, busy: false }, 2);
        }
        for a in &seen.shown {
            s.names.remove(&(a / PAGE));
        }
    }
}

/// The sums as text, each divided by `per` (frames, or 1 for totals).
fn words(t: &Tally, per: f64) -> String {
    let n = |v: u64| v as f64 / per;
    let mb = |v: u64| v as f64 / per / (1 << 20) as f64;
    let stores = format!(
        "{:.1} stretches {:.2} MB stored in {:.1} pages ({:.1} stretches {:.2} MB through views of our own; the rest the \
         old way, counted and never looked at); {:.1} were of a page stored a frame before or in the same frame",
        n(t.stores),
        mb(t.bytes),
        n(t.pages),
        n(t.own),
        mb(t.own_bytes),
        n(t.again),
    );
    let readers = format!(
        "{:.1} stores followed to their end, taken meanwhile by: a refresh of the copy that stored {:.1}, a refresh of \
         another copy {:.1}, another merge {:.1}, the look for title bytes under flights {:.1}, a title copy or a fill \
         {:.1}, a store the old way {:.1}, another reader of ours {:.1}; the guard was clean after {:.1} stores and the \
         title wrote {:.1} of those",
        n(t.ended),
        n(t.who[REFRESH as usize]),
        n(t.who[REFRESH_OTHER as usize]),
        n(t.who[MERGE as usize]),
        n(t.who[UNDER as usize]),
        n(t.who[TITLE_COPY as usize]),
        n(t.who[STORE as usize]),
        n(t.who[OTHER as usize]),
        n(t.clean),
        n(t.written),
    );
    let looks = format!(
        "looked at {:.1} of {:.1} taken out of the working set (one page in {SAMPLE}): untouched {:.1} ({:.2} MB), \
         written by the title {:.1}, touched with readers of ours and no write {:.1}, touched by something else (the \
         title's reads at most) {:.1} ({:.2} MB); of their 4 KiB pages {:.1} of {:.1} were touched; ended with no store \
         of the page again in {KEEP} frames {:.1}, by a remapping {:.1}, with no look because a mapping was changing {:.1}",
        n(t.looked),
        n(t.trimmed),
        n(t.untouched),
        mb(t.untouched_bytes),
        n(t.wrote),
        n(t.ours),
        n(t.other),
        mb(t.other_bytes),
        n(t.small[0]),
        n(t.small[1]),
        n(t.late),
        n(t.remapped),
        n(t.busy),
    );
    format!("{stores}; {readers}; {looks}")
}

/// The ten copies with the most bytes stored, and everything, since the
/// start.
fn table(s: &State) {
    let mut rows: Vec<(&(u64, u64), &Tally)> = s.buffers.iter().collect();
    rows.sort_by_key(|(_, t)| std::cmp::Reverse(t.bytes));
    tracing::info!(
        "what came back from the card and who touched it, since the start (VICTORY_BACK_READS), totals not per frame: {} \
         buffers; all of them: {}",
        rows.len(),
        words(&s.all, 1.0)
    );
    for ((start, len), t) in rows.into_iter().take(10) {
        tracing::info!("  came back to {start:#x}+{len:#x}: {}", words(t, 1.0));
    }
}

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

/// At process exit (not reached when the process is ended from outside:
/// the table is also logged with every eighth line).
extern "C" fn at_exit() {
    if let Ok(guard) = STATE.try_lock()
        && let Some(s) = guard.as_ref()
    {
        table(s);
    }
}

/// With the frame-rate line, over `frames` more frames: stores not
/// followed by another within [`KEEP`] frames end here, and every
/// [`EVERY`] frames the sums are said.
pub(crate) fn line(frames: f64) {
    if !on() {
        return;
    }
    static AT_EXIT: std::sync::Once = std::sync::Once::new();
    // SAFETY: registers a function with the host CRT's exit table.
    AT_EXIT.call_once(|| unsafe {
        atexit(at_exit);
    });
    let frame = crate::frame::current();
    let old: Vec<Seen> = {
        let mut guard = state();
        let s = guard.get_or_insert_with(State::default);
        let keys: Vec<usize> = s.seen.iter().filter(|(_, seen)| seen.frame + KEEP < frame).map(|(key, _)| *key).collect();
        let old: Vec<Seen> = keys.into_iter().filter_map(|key| s.seen.remove(&key)).collect();
        old
    };
    // Looked at with no lock of ours held.
    let judged: Vec<Option<Judged>> = old.iter().map(|seen| seen.open.then(|| judge(seen))).collect();
    let mut guard = state();
    let s = guard.get_or_insert_with(State::default);
    for (seen, judged) in old.iter().zip(&judged) {
        if let Some(judged) = judged {
            tally(s, seen, judged, 1);
        }
    }
    // The names of what is gone, and the hooks' filter anew (a hook that
    // looks while it is rebuilt may miss one read).
    let seen = &s.seen;
    s.names.retain(|_, key| seen.contains_key(key));
    for word in &TRACKED {
        word.store(0, Relaxed);
    }
    for page in s.names.keys() {
        mark(*page);
    }
    s.frames += frames;
    if s.frames < EVERY {
        return;
    }
    let (since, over) = (std::mem::take(&mut s.since), std::mem::replace(&mut s.frames, 0.0));
    tracing::info!(
        "what came back from the card and who touched it, per frame over {over:.0} frames (VICTORY_BACK_READS={}): {}",
        std::env::var("VICTORY_BACK_READS").unwrap_or_default(),
        words(&since, over)
    );
    s.lines += 1;
    if s.lines.is_multiple_of(8) {
        table(s);
    }
}
