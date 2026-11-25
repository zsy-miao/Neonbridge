//! Live bytes of the Rust heap, kept by the binary's global allocator for the resident line
//! (`victory-gpu`'s `ramline`). Here because `apps/victory` and `victory-gpu` both depend on this crate.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// Live bytes, and the peak of live bytes.
pub static RUST_HEAP: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Whether the counting is on (`VICTORY_HEAP_COUNT=1`, set once by `main`). Off by default:
/// two atomics on one shared line per allocation slowed loading, with many threads allocating.
static ON: AtomicBool = AtomicBool::new(false);

/// Turns the counting on or off; called by `main` before the runtime starts (the allocator
/// itself must not read the environment: that allocates).
pub fn enable(on: bool) {
    ON.store(on, Relaxed);
}

/// `bytes` more are live.
pub fn grow(bytes: usize) {
    if !ON.load(Relaxed) {
        return;
    }
    let live = RUST_HEAP[0].fetch_add(bytes as u64, Relaxed) + bytes as u64;
    RUST_HEAP[1].fetch_max(live, Relaxed);
}

/// `bytes` are released.
pub fn shrink(bytes: usize) {
    if !ON.load(Relaxed) {
        return;
    }
    // Saturating: blocks made before `main` turned the counting on were never added.
    let _ = RUST_HEAP[0].fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(bytes as u64)));
}
