//! TUXBOX_SHADOW_READ_SET (default on, =0 refreshes every live shadow): the
//! title ranges each command list reads (vertex and index buffer views,
//! constant buffers, root descriptors, buffer views in tables, indirect
//! arguments), so a submission's refresh brings up to date only the shadows
//! those ranges touch, as ztrx/VIctory victory-gpu shadow.rs refreshes "the
//! ranges each submission reads" (GPL-3.0, credit to its authors). Shadows
//! a submission skips keep their journal stamps (TUXBOX_SHADOW_JOURNAL) or
//! hash, so the submission that next reads them copies what changed since.
//!
//! The list side calls [`begin`] when a list is opened (create or Reset),
//! [`record_read`] for each range it binds, and [`submitted`] with the
//! submission's lists just before the refresh. A submission holding a list
//! that was never opened through [`begin`] (or that overflowed) refreshes
//! every shadow, as before.

// The list side's calls land with Coder1's QUEUE_HOLD work.
#![allow(dead_code)]

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

/// Ranges past this many in one list: the list counts as reading anything.
const MOST: usize = 1 << 16;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_SHADOW_READ_SET") != Some("0"))
}

/// Per list (its pointer): the ranges it reads, None once it overflowed.
static LISTS: Mutex<Option<HashMap<usize, Option<Vec<Range<u64>>>>>> = Mutex::new(None);

/// The next refresh's read set: merged, sorted ranges, or None for all.
static PENDING: Mutex<Pending> = Mutex::new(Pending::None);

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

/// `list` was opened for recording (created or Reset): its reads start empty.
pub(crate) fn begin(list: usize) {
    if enabled() {
        lock(&LISTS).get_or_insert_with(HashMap::new).insert(list, Some(Vec::new()));
    }
}

/// `list` reads title `[address, address + bytes)`. 0 bytes: to the end of
/// whatever holds it (a root view), recorded as one byte.
pub(crate) fn record_read(list: usize, address: u64, bytes: u64) {
    if !enabled() || address == 0 {
        return;
    }
    let mut lists = lock(&LISTS);
    let Some(Some(Some(ranges))) = lists.as_mut().map(|l| l.get_mut(&list)) else { return };
    if ranges.len() >= MOST {
        lists.as_mut().unwrap().insert(list, None);
        return;
    }
    let end = address.saturating_add(bytes.max(1));
    // Consecutive binds of the same or adjoining range merge at once.
    if let Some(last) = ranges.last_mut() && last.start <= end && address <= last.end {
        last.start = last.start.min(address);
        last.end = last.end.max(end);
        return;
    }
    ranges.push(address..end);
}

/// `list` was released: forget it.
pub(crate) fn end(list: usize) {
    if let Some(lists) = lock(&LISTS).as_mut() {
        lists.remove(&list);
    }
}

/// A submission of `lists` is about to refresh: its read set is theirs.
pub(crate) fn submitted(lists: &[usize]) {
    if !enabled() {
        return;
    }
    let mut all: Option<Vec<Range<u64>>> = Some(Vec::new());
    {
        let lists_map = lock(&LISTS);
        for list in lists {
            match lists_map.as_ref().and_then(|l| l.get(list)) {
                Some(Some(ranges)) => {
                    if let Some(a) = all.as_mut() {
                        a.extend(ranges.iter().cloned());
                    }
                }
                _ => {
                    all = None;
                    break;
                }
            }
        }
    }
    let merged = all.map(merge);
    let mut pending = lock(&PENDING);
    // Two submissions before one refresh: the union; any "all" wins until taken.
    *pending = match (pending.take(), merged) {
        (_, None) => Pending::All,
        (Pending::All, _) => Pending::All,
        (Pending::None, Some(b)) => Pending::Set(b),
        (Pending::Set(mut a), Some(b)) => {
            a.extend(b);
            Pending::Set(merge(a))
        }
    };
}

enum Pending {
    None,
    All,
    Set(Vec<Range<u64>>),
}

impl Pending {
    fn take(&mut self) -> Pending {
        std::mem::replace(self, Pending::None)
    }
}

fn merge(mut ranges: Vec<Range<u64>>) -> Vec<Range<u64>> {
    ranges.sort_unstable_by_key(|r| r.start);
    let mut out: Vec<Range<u64>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// The refresh's read set, taken: None refreshes every shadow (no
/// submission said what it reads, or one could not).
pub(crate) fn take() -> Option<Vec<Range<u64>>> {
    if !enabled() {
        return None;
    }
    match lock(&PENDING).take() {
        Pending::Set(reads) => Some(reads),
        _ => None,
    }
}

/// Whether `[start, end)` meets one of the sorted, merged `reads`.
pub(crate) fn meets(reads: &[Range<u64>], start: u64, end: u64) -> bool {
    let at = reads.partition_point(|r| r.end <= start);
    reads.get(at).is_some_and(|r| r.start < end)
}
