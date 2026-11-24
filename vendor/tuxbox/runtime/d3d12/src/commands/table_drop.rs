//! A draw or dispatch whose descriptor table cannot be made is dropped
//! (TUXBOX_TABLE_DROP=0: the list fails with E_OUTOFMEMORY, as before).
//!
//! A paged arena materializes each table a list binds into its bounded
//! shader-visible heap. When that heap is full the table has no host handle;
//! binding the title's own table address would read unrelated slots, and
//! failing the whole list loses every other draw in it. So that one draw,
//! dispatch, mesh dispatch or ExecuteIndirect is skipped and counted, and the
//! slot stays unresolved until the title binds a table that fits.
//! (Facts from VIctory 3c81420, re-derived.)
//!
//! VIctory e8d3b64 sizes a dynamic range above the mirrored title slots (2^18
//! only if title slots + 2^18 fit under the 1,000,000-view cap, else 2^16).
//! Here no title slot is mirrored into an arena's visible heap: the whole heap,
//! up to the device's view cap, holds materialized tables, and heaps that fit
//! the cap are native heaps with no dynamic range to grow.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::E_OUTOFMEMORY;
use windows::core::HRESULT;

pub(super) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TUXBOX_TABLE_DROP").is_ok_and(|v| v.trim() == "0"))
}

/// The table could not be made because the visible heap is full.
pub(super) fn exhausted(error: HRESULT) -> bool {
    error == E_OUTOFMEMORY
}

/// Whether to drop this command for `error` instead of failing the list.
pub(super) fn drops(error: HRESULT, what: &str) -> bool {
    if !enabled() || !exhausted(error) {
        return false;
    }
    note(what);
    true
}

static DROPPED: AtomicU64 = AtomicU64::new(0);

pub(super) fn note(what: &str) {
    let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "{what} dropped: its descriptor table does not fit the visible heap ({n} dropped; TUXBOX_TABLE_DROP=0 fails the list)"
        ));
    }
}

/// Bit per root slot; index 0 graphics, 1 compute.
#[derive(Default, Clone, Copy)]
pub(super) struct Unresolved([u64; 2]);

impl Unresolved {
    pub(super) fn set(&mut self, compute: bool, slot: u32, on: bool) {
        let Some(bit) = 1u64.checked_shl(slot) else { return };
        let word = &mut self.0[usize::from(compute)];
        if on { *word |= bit } else { *word &= !bit }
    }

    pub(super) fn any(&self, compute: bool) -> bool {
        self.0[usize::from(compute)] != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exhaustion_drops() {
        assert!(exhausted(E_OUTOFMEMORY));
        assert!(!exhausted(windows::Win32::Foundation::E_INVALIDARG));
    }

    #[test]
    fn unresolved_slots_are_per_kind() {
        let mut u = Unresolved::default();
        u.set(false, 3, true);
        assert!(u.any(false) && !u.any(true));
        u.set(true, 63, true);
        u.set(false, 3, false);
        assert!(!u.any(false) && u.any(true));
        u.set(true, 64, true); // out of range: ignored
        u.set(true, 63, false);
        assert!(!u.any(true));
    }
}
