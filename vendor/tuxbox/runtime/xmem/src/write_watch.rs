//! Two readers of one write-watch list: the title (XMemGetWriteWatch, or
//! kernel32 GetWriteWatch/ResetWriteWatch from its imports) and the d3d12
//! runtime (TuxBoxTakeWriteWatch, to skip unwritten placed textures). A reset
//! by either would hide writes from the other, so every page one side resets
//! is kept for the other until that side reads it.
//!
//! The 4 KiB granularity and the OS's own ordering (ascending) are kept.

use core::ffi::c_void;
use std::collections::BTreeSet;
use std::sync::Mutex;

use windows_sys::Win32::System::Memory::{GetWriteWatch, ResetWriteWatch};

const RESET: u32 = 1;

/// Pages the runtime reset that the title has not read yet.
static OWED_TITLE: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());
/// Pages the title reset that the runtime has not read yet.
static OWED_TUXBOX: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());
/// One query-and-hand-over at a time, so no page falls between the two sets.
static QUERY: Mutex<()> = Mutex::new(());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// OS pages written in [base, base+bytes), resetting them when `reset`.
unsafe fn os_pages(reset: bool, base: *const c_void, bytes: usize, granularity: &mut u32) -> Result<Vec<usize>, u32> {
    let mut pages = vec![core::ptr::null_mut::<c_void>(); bytes.div_ceil(0x1000).max(1)];
    let mut count = pages.len();
    let failed = unsafe { GetWriteWatch(if reset { RESET } else { 0 }, base, bytes, pages.as_mut_ptr(), &mut count, granularity) };
    if failed != 0 {
        return Err(failed);
    }
    pages.truncate(count);
    Ok(pages.into_iter().map(|p| p as usize).collect())
}

/// Shared body: `mine` holds pages owed to the caller, `theirs` to the other side.
unsafe fn query(
    reset: bool, base: *const c_void, bytes: usize, out: *mut *mut c_void, count: *mut usize, granularity: *mut u32,
    mine: &Mutex<BTreeSet<usize>>, theirs: &Mutex<BTreeSet<usize>>,
) -> u32 {
    if count.is_null() || out.is_null() || granularity.is_null() {
        return unsafe { GetWriteWatch(if reset { RESET } else { 0 }, base, bytes, out, count, granularity) };
    }
    let _one = lock(&QUERY);
    let mut gran = 0;
    let os = match unsafe { os_pages(reset, base, bytes, &mut gran) } {
        Ok(p) => p,
        Err(e) => return e,
    };
    let range = base as usize..(base as usize).saturating_add(bytes);
    if reset {
        lock(theirs).extend(os.iter().copied());
    }
    let mut owed = lock(mine);
    let mut all: BTreeSet<usize> = os.into_iter().collect();
    all.extend(owed.range(range.clone()).copied());
    let capacity = unsafe { count.read() };
    let reported: Vec<usize> = all.into_iter().take(capacity).collect();
    if reset {
        for p in &reported {
            owed.remove(p);
        }
    }
    for (i, p) in reported.iter().enumerate() {
        unsafe { out.add(i).write(*p as *mut c_void) };
    }
    unsafe {
        count.write(reported.len());
        granularity.write(gran);
    }
    0
}

/// The title's view (XMemGetWriteWatch and its kernel32 GetWriteWatch import).
pub(crate) unsafe extern "system" fn title_get(flags: u32, base: *const c_void, bytes: usize, out: *mut *mut c_void, count: *mut usize, granularity: *mut u32) -> u32 {
    unsafe { query(flags & RESET != 0, base, bytes, out, count, granularity, &OWED_TITLE, &OWED_TUXBOX) }
}

/// The title's ResetWriteWatch import: pages it drops stay owed to the runtime.
pub(crate) unsafe extern "system" fn title_reset(base: *const c_void, bytes: usize) -> u32 {
    let _one = lock(&QUERY);
    let mut gran = 0;
    match unsafe { os_pages(true, base, bytes, &mut gran) } {
        Ok(pages) => {
            lock(&OWED_TUXBOX).extend(pages);
            let range = base as usize..(base as usize).saturating_add(bytes);
            let mut owed = lock(&OWED_TITLE);
            let gone: Vec<usize> = owed.range(range).copied().collect();
            for p in gone {
                owed.remove(&p);
            }
            0
        }
        Err(_) => unsafe { ResetWriteWatch(base, bytes) },
    }
}

/// d3d12: pages written in [base, base+bytes) since its last take (reset).
/// Returns 0 on success, else the GetWriteWatch error (not a watched range).
#[unsafe(export_name = "TuxBoxTakeWriteWatch")]
pub unsafe extern "system" fn tuxbox_take(base: *const c_void, bytes: usize, out: *mut *mut c_void, count: *mut usize, granularity: *mut u32) -> u32 {
    unsafe { query(true, base, bytes, out, count, granularity, &OWED_TUXBOX, &OWED_TITLE) }
}

/// Called from loader::install: the exe's own write-watch imports share the list.
pub(crate) unsafe fn install() {
    let patched = unsafe {
        crate::loader::patch_imports(
            |d| d == "kernel32.dll" || d == "kernelbase.dll" || d.starts_with("api-ms-win-core-memory"),
            &[("GetWriteWatch", title_get as *const () as usize), ("ResetWriteWatch", title_reset as *const () as usize)],
        )
    };
    crate::loader::log(&format!("write watch: {patched} GetWriteWatch/ResetWriteWatch import(s) share the list with the d3d12 runtime"));
}
