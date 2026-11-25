//! TUXBOX_PLACED_DEVICE (default on, =0 off), xmem half: title pages whose
//! GPU copy lives in a d3d12 device buffer (GPU-only placements in the title
//! GPU page pool) instead of an import of the pages themselves. The pages
//! stay uncommitted until the CPU touches one (lazy_commit), and a page d3d12
//! has handed to the GPU is kept PAGE_NOACCESS (protect), so the next CPU
//! touch faults here: the page is committed if reserved, d3d12's callback
//! brings the GPU's bytes back if the GPU may have written them, and the
//! page turns read-write and CPU-owned until d3d12 uploads and protects it
//! again before a submit that uses it. Every protection change applies to
//! all title views of the same physical page (physical_pages::aliases): the
//! title writes its pool through a CPU alias (test155 mapped 0x37f10 at both
//! 0x480000000 and 0xdac000000).
//! Model: LMIII / VItoriaBr / leonida device-heap shadows by title VA
//! (victory-gpu resource.rs create_at_address, shadow.rs; GPL-3.0, credit),
//! with copy-back moved to the CPU's first touch so GPU-only pages are never
//! committed.
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::System::Memory::{PAGE_NOACCESS, PAGE_READWRITE, VirtualProtect};

const PAGE: usize = 0x1_0000;

/// start -> end of device-backed title ranges.
static RANGES: RwLock<BTreeMap<usize, usize>> = RwLock::new(BTreeMap::new());
/// d3d12's callback: (page, write) -> whether the access may go on.
static CALLBACK: AtomicUsize = AtomicUsize::new(0);
type Callback = unsafe extern "system" fn(usize, u32) -> u32;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_PLACED_DEVICE").map_or(true, |v| v.trim() != "0"))
}

/// The covered alias of `page` (itself or another view of its physical page).
fn covered_alias(page: usize) -> Option<usize> {
    if covers(page) {
        return Some(page);
    }
    super::physical_pages::aliases(page).into_iter().find(|&a| a != page && covers(a))
}

fn set_all(page: usize, protection: u32) -> bool {
    let mut ok = true;
    for at in super::physical_pages::aliases(page) {
        let mut old = 0;
        ok &= unsafe { VirtualProtect(at as *const c_void, PAGE, protection, &mut old) } != 0;
    }
    ok
}

pub(crate) fn covers(page: usize) -> bool {
    for _ in 0..1000 {
        if let Ok(ranges) = RANGES.try_read() {
            return ranges.range(..=page).next_back().is_some_and(|(_, &end)| page < end);
        }
        std::thread::yield_now();
    }
    false
}

/// The fault handler's part: true when the access at `page` may resume.
pub(crate) unsafe fn fault(page: usize, access: usize) -> bool {
    if !enabled() || RANGES.try_read().is_ok_and(|r| r.is_empty()) {
        return false;
    }
    let Some(primary) = covered_alias(page) else { return false };
    // A reserved page is committed first (it reads as zeros, a fresh page).
    let committed = unsafe { super::lazy_commit::commit_page_any(page) };
    if !committed {
        return false;
    }
    let callback = CALLBACK.load(Ordering::Acquire);
    if callback != 0 {
        let f: Callback = unsafe { std::mem::transmute(callback) };
        if unsafe { f(primary, u32::from(access == 1)) } == 0 {
            return false;
        }
    }
    set_all(page, PAGE_READWRITE)
}

/// A kernel call is about to read or write `[address, address + bytes)`
/// (TuxBoxCommitRange): its device-backed pages are opened as a CPU touch
/// would open them, since a kernel access to a no-access page fails.
pub(crate) fn open(address: usize, bytes: usize) {
    if !enabled() || bytes == 0 || RANGES.read().map_or(true, |r| r.is_empty()) {
        return;
    }
    let (mut at, end) = (address & !(PAGE - 1), address.saturating_add(bytes));
    while at < end {
        unsafe { fault(at, 1) };
        at += PAGE;
    }
}

/// Registers (`on` 1) or forgets (`on` 0) a device-backed title range.
#[unsafe(export_name = "TuxBoxDeviceRange")]
unsafe extern "system" fn device_range(start: usize, bytes: usize, on: u32) -> u32 {
    if !enabled() || bytes == 0 {
        return 0;
    }
    let mut ranges = RANGES.write().unwrap_or_else(|e| e.into_inner());
    if on != 0 {
        ranges.insert(start, start.saturating_add(bytes));
    } else {
        ranges.remove(&start);
    }
    1
}

/// Sets d3d12's CPU-touch callback.
#[unsafe(export_name = "TuxBoxDeviceFaultCallback")]
unsafe extern "system" fn device_fault_callback(callback: usize) {
    CALLBACK.store(callback, Ordering::Release);
}

/// Hands the committed pages of `[start, start + bytes)` to the GPU: they
/// become PAGE_NOACCESS so the next CPU touch faults. Returns pages protected.
#[unsafe(export_name = "TuxBoxProtectDevicePages")]
unsafe extern "system" fn protect_device_pages(start: usize, bytes: usize) -> usize {
    let (mut at, end) = (start & !(PAGE - 1), start.saturating_add(bytes));
    let mut n = 0;
    while at < end {
        if set_all(at, PAGE_NOACCESS) {
            n += 1;
        }
        at += PAGE;
    }
    n
}

/// Sets `protection` on `[start, start + bytes)` and every alias of its
/// pages; returns 1 when all changes took.
#[unsafe(export_name = "TuxBoxProtectDeviceAliases")]
unsafe extern "system" fn protect_device_aliases(start: usize, bytes: usize, protection: u32) -> u32 {
    let (mut at, end) = (start & !(PAGE - 1), start.saturating_add(bytes));
    let mut ok = true;
    while at < end {
        ok &= set_all(at, protection);
        at += PAGE;
    }
    u32::from(ok)
}
