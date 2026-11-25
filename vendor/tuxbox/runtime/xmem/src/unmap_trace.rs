//! The last 4096 ranges xmem unmapped or decommitted, so a consumer that
//! finds title memory gone (xcrdapi's destination check; test154 failed a
//! store into 0x4f0950000) can name who took it and how long ago, instead
//! of guessing a title tile-pool unmap (rebox f61bbc8). A size of 0 means a
//! whole view or allocation starting at the address.
//! TUXBOX_XMEM_UNMAP_TRACE=0 records nothing.
use std::sync::Mutex;
use std::time::Instant;

/// Who unmapped a range (TuxBoxUnmapTrace's site word).
pub(crate) const PHYSICAL_UNMAP: u32 = 1;
pub(crate) const VIEW_UNMAP: u32 = 2;
pub(crate) const POOL_DECOMMIT: u32 = 3;
pub(crate) const RELEASE: u32 = 4;

const KEEP: usize = 4096;

struct Ring {
    entries: Vec<(usize, usize, u32, Instant)>,
    next: usize,
}

static RING: Mutex<Ring> = Mutex::new(Ring { entries: Vec::new(), next: 0 });

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_XMEM_UNMAP_TRACE").map_or(true, |v| v.trim() != "0"))
}

pub(crate) fn record(address: usize, bytes: usize, site: u32) {
    if !on() {
        return;
    }
    let mut ring = RING.lock().unwrap_or_else(|e| e.into_inner());
    let entry = (address, bytes, site, Instant::now());
    if ring.entries.len() < KEEP {
        ring.entries.push(entry);
    } else {
        let at = ring.next;
        ring.entries[at] = entry;
    }
    ring.next = (ring.next + 1) % KEEP;
}

/// In-crate form of TuxBoxUnmapTrace.
pub(crate) unsafe fn newest(address: usize, bytes: usize, out: &mut [u64; 4]) -> bool {
    unsafe { unmap_trace_export(address, bytes, out) != 0 }
}

/// The newest recorded unmap overlapping `[address, address + bytes)`:
/// returns 1 and writes its site, start, size and age in ms; 0 when none.
#[unsafe(export_name = "TuxBoxUnmapTrace")]
unsafe extern "system" fn unmap_trace_export(address: usize, bytes: usize, out: *mut [u64; 4]) -> u32 {
    let end = address.saturating_add(bytes.max(1));
    let ring = RING.lock().unwrap_or_else(|e| e.into_inner());
    let found = ring.entries.iter()
        .filter(|&&(start, size, _, _)| {
            let stop = if size == 0 { start + 1 } else { start.saturating_add(size) };
            start < end && address < stop
        })
        .max_by_key(|e| e.3);
    let Some(&(start, size, site, at)) = found else { return 0 };
    if !out.is_null() {
        unsafe { out.write([u64::from(site), start as u64, size as u64, at.elapsed().as_millis() as u64]) };
    }
    1
}
