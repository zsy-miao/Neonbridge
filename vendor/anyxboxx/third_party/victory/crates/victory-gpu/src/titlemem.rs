//! Where the title's own memory is, for the resident line (`ramline`): ranges the
//! runtime tells us about when it allocates, maps, unmaps or frees for the title.
//! Not per-frame: a lock and a map insert per call, and only large heap blocks.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// `XMemVirtualAlloc` reservations (the title frees them itself, so entries can go stale).
pub const VIRTUAL: u8 = 1;
/// Views of physical pages (pool views, page mappings).
pub const VIEW: u8 = 2;
/// Large `XMemAlloc` heap blocks (small ones are not listed).
pub const HEAP: u8 = 3;
/// The title's exe and DLLs.
pub const IMAGE: u8 = 4;

static RANGES: Mutex<BTreeMap<usize, (usize, u8)>> = Mutex::new(BTreeMap::new());

fn ranges() -> std::sync::MutexGuard<'static, BTreeMap<usize, (usize, u8)>> {
    RANGES.lock().unwrap_or_else(|e| e.into_inner())
}

/// `[base, base + size)` is title memory of `kind` (replaces a range at `base`).
pub fn title_range(base: usize, size: usize, kind: u8) {
    if size != 0 {
        ranges().insert(base, (size, kind));
    }
}

/// The range that began at `base` is gone.
pub fn title_range_free(base: usize) {
    ranges().remove(&base);
}

/// A copy of the ranges as (base, size, kind), by base. The lock is not held after.
pub(crate) fn snapshot() -> Vec<(usize, usize, u8)> {
    ranges().iter().map(|(&b, &(s, k))| (b, s, k)).collect()
}

/// Forgets a `VIRTUAL` range the title freed behind our back (its base is no longer an allocation base).
pub(crate) fn forget_if_stale(base: usize) {
    let mut r = ranges();
    if r.get(&base).is_some_and(|e| e.1 == VIRTUAL) {
        r.remove(&base);
    }
}
