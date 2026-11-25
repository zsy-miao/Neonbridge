//! The switches of the second round of memory work (`VICTORY_MEM2_*`, all
//! off by default) and their counters, logged with the memory line by pool
//! ([`line`]).
//!
//! - `VICTORY_MEM2_GPU_ONLY_NOVIEW=1`: `crate::translate`.
//! - `VICTORY_MEM2_PAGE_REHASH=1`: a guarded texture found written is
//!   hashed page by page, only the pages written (`crate::texture`), not
//!   whole; it is detiled when one of them changed.
//! - `VICTORY_MEM2_GPU_ONLY_NOSNAP=1`: a host copy of memory in a GPU-only
//!   reservation that shaders write takes no snapshot; after the flight its
//!   bytes are compared with title memory itself, which nothing the CPU can
//!   do changes meanwhile (`crate::shadow`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// `VICTORY_MEM2_PAGE_REHASH=1`.
pub(crate) fn page_rehash() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = victory_core::mem2::switch("VICTORY_MEM2_PAGE_REHASH");
        if on {
            tracing::warn!("VICTORY_MEM2_PAGE_REHASH: written textures are hashed by page");
        }
        on
    })
}

/// `VICTORY_MEM2_GPU_ONLY_NOSNAP=1`; only on the copy-back path that
/// waits for each submission (no relay, no late waits, no kept snapshots,
/// no wide copy-back): elsewhere it says why, once, and stays off.
pub(crate) fn nosnap() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        if !victory_core::mem2::switch("VICTORY_MEM2_GPU_ONLY_NOSNAP") {
            return false;
        }
        let other = if crate::host::relay() || crate::shadow::relay() {
            Some("VICTORY_RELAY")
        } else if crate::host::late_on() {
            Some("VICTORY_LATE_WAIT")
        } else if crate::shadow::keeps_snapshots() {
            Some("VICTORY_KEEP_SNAPSHOTS")
        } else if crate::shadow::wide_back() {
            Some("VICTORY_WIDE_BACK")
        } else {
            None
        };
        if let Some(other) = other {
            tracing::warn!("VICTORY_MEM2_GPU_ONLY_NOSNAP is left off: it does not go with {other}");
            return false;
        }
        tracing::warn!("VICTORY_MEM2_GPU_ONLY_NOSNAP: GPU-only memory shaders write is compared with title memory, no snapshot");
        true
    })
}

/// With [`nosnap`]: copies declared without a snapshot, the snapshot bytes
/// not taken for them, bytes kept as pre-images all the same (pieces where
/// the copy differed from title memory when declared, or that the runtime
/// wrote during the flight), 64 KiB pieces written with nothing kept (the
/// title's bytes win there), copies given a snapshot after all (an unknown
/// write of title memory before the flight), bytes copied back.
pub(crate) static NOSNAP: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// With [`page_rehash`]: checks by page, bytes hashed by them, bytes of
/// the textures they did not hash, checks that found a page changed.
pub(crate) static REHASH: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// `victory-hle` xmem's answer to: does any page shown in `[at, at + len)`
/// show where the CPU can reach it too?
static CPU_FREE: OnceLock<fn(usize, usize) -> bool> = OnceLock::new();

pub fn use_cpu_free(f: fn(usize, usize) -> bool) {
    let _ = CPU_FREE.set(f);
}

/// Whether no page shown in `[at, at + len)` is also shown where the CPU
/// can reach it (false if xmem did not say how to ask).
pub(crate) fn cpu_free(at: usize, len: usize) -> bool {
    CPU_FREE.get().is_some_and(|f| f(at, len))
}

/// The counters, with the memory line by pool.
pub(crate) fn line() {
    let t = crate::translate::COUNTS.each_ref().map(|a| a.load(Relaxed));
    let n = NOSNAP.each_ref().map(|a| a.load(Relaxed));
    let r = REHASH.each_ref().map(|a| a.load(Relaxed));
    let mb = |b: u64| b as f64 / 1e6;
    tracing::info!(
        "mem2 since the start: GPU-only pages skipped {} (VICTORY_MEM2_GPU_ONLY_NOVIEW={}; {} translated now, {} made views after all, \
         {:.0} MB read and {:.0} MB written through the table); bytes not shadowed {:.0} MB net (VICTORY_MEM2_GPU_ONLY_NOSNAP={}; {} copies \
         without a snapshot, {:.1} MB kept as pre-images, {} pieces the title won, {} given a snapshot after all, {:.0} MB copied back); \
         rehash bytes saved {:.0} MB (VICTORY_MEM2_PAGE_REHASH={}; {} checks by page hashed {:.0} MB, {} found a change)",
        t[0],
        u8::from(crate::translate::enabled()),
        crate::translate::live(),
        t[1],
        mb(t[2]),
        mb(t[3]),
        mb(n[1].saturating_sub(n[2])),
        u8::from(nosnap()),
        n[0],
        mb(n[2]),
        n[3],
        n[4],
        mb(n[5]),
        mb(r[2]),
        u8::from(page_rehash()),
        r[0],
        mb(r[1]),
        r[3],
    );
}
