//! TUXBOX_SECTION_WATCH (default on, =0 off): which 64 KiB blocks of title
//! section views the CPU wrote, where the OS write watch cannot answer.
//! test173: every runtime write-watch query was refused (6167 in 10 s,
//! 1947 ms in the OS) because placed textures live in physical-page views
//! (MapViewOfFile3), which carry no write watch, so no mip was ever skipped.
//!
//! Ported from ztrx victory-gpu watch.rs (guard / protect / open / install /
//! title_memory_remapped, GPL-3.0, credit to its authors): a block the
//! runtime has taken is left read-only, with every alias view of its
//! physical page; the first CPU write faults into our vectored handler,
//! which notes the block written and gives write access back. The next query
//! reports the written blocks (all their pages) and protects them again.
//! A block never seen, or remapped since (map/unmap reset protection), is
//! reported written.
//!
//! What it does not see, as in ztrx: kernel writes (ReadFile into a guarded
//! block is opened first, read_commit.rs) and writes into the section
//! through a view of our own (write_from) while the page is unmapped; the
//! page is reported written when it is mapped again.
use core::ffi::c_void;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_MAPPED, MEMORY_BASIC_INFORMATION, PAGE_READONLY, PAGE_READWRITE, VirtualProtect, VirtualQuery,
};

const BLOCK: usize = 0x1_0000;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_SECTION_WATCH").map_or(true, |v| v.trim() != "0"))
}

#[derive(Default)]
struct Guards {
    /// Clean (guarded, unwritten) block -> its aliases (itself included)
    /// and the protection each had.
    clean: BTreeMap<usize, Vec<(usize, u32)>>,
    /// Any guarded alias block -> the block it was taken for.
    alias_of: BTreeMap<usize, usize>,
}

static GUARDS: Mutex<Option<Guards>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Guards>> {
    GUARDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Gives write access back to `block`'s guard (looked up through any alias);
/// true when it was ours.
fn open(g: &mut Guards, at: usize) -> bool {
    let Some(&owner) = g.alias_of.get(&(at & !(BLOCK - 1))) else { return false };
    if let Some(aliases) = g.clean.remove(&owner) {
        for (a, old) in aliases {
            g.alias_of.remove(&a);
            let mut was = 0;
            unsafe { VirtualProtect(a as *const c_void, BLOCK, old, &mut was) };
        }
    }
    true
}

/// Is `[start, end)` view memory (its first committed region mapped)?
fn is_view(start: usize) -> bool {
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    let n = unsafe { VirtualQuery(start as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    n != 0 && info.Type == MEM_MAPPED
}

/// Written 4 KiB pages of `[start, end)` since the last query, up to `cap`;
/// the blocks reported are guarded again. None when the range is no view.
pub(crate) fn query(start: usize, end: usize, cap: usize) -> Option<Vec<usize>> {
    if !enabled() || start >= end || !is_view(start) {
        return None;
    }
    install();
    let mut out = Vec::new();
    let mut slot = lock();
    let g = slot.get_or_insert_with(Guards::default);
    let mut block = start & !(BLOCK - 1);
    while block < end {
        if !g.clean.contains_key(&block) {
            let from = block.max(start & !0xfff);
            let to = (block + BLOCK).min(end);
            let pages = (to - from).div_ceil(0x1000);
            if out.len() + pages > cap {
                // Left unguarded: reported by the next query.
                break;
            }
            out.extend((0..pages).map(|i| from + i * 0x1000));
            if !crate::device_pages::covers(block) {
                protect(g, block);
            }
        }
        block += BLOCK;
    }
    Some(out)
}

/// Makes `block` and its aliases read-only and notes it clean; a block
/// whose protection change fails stays unguarded (reported written again).
fn protect(g: &mut Guards, block: usize) {
    let mut done = Vec::new();
    for a in crate::physical_pages::aliases(block) {
        let a = a & !(BLOCK - 1);
        if g.alias_of.contains_key(&a) && a != block {
            continue;
        }
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(a as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0
            || info.State != MEM_COMMIT
            || info.Protect & 0xff != PAGE_READWRITE
        {
            if a == block {
                break;
            }
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(a as *const c_void, BLOCK, (info.Protect & !0xff) | PAGE_READONLY, &mut old) } != 0 {
            done.push((a, old));
        } else if a == block {
            break;
        }
    }
    if done.iter().any(|&(a, _)| a == block) {
        for &(a, _) in &done {
            g.alias_of.insert(a, block);
        }
        g.clean.insert(block, done);
    } else {
        for (a, old) in done {
            let mut was = 0;
            unsafe { VirtualProtect(a as *const c_void, BLOCK, old, &mut was) };
        }
    }
}

/// Before the kernel writes `[at, at + len)` (ReadFile): opens its guards.
pub(crate) fn writing(at: usize, len: usize) {
    if !enabled() {
        return;
    }
    let mut slot = lock();
    let Some(g) = slot.as_mut() else { return };
    let blocks: BTreeSet<usize> = g.alias_of.range(at & !(BLOCK - 1)..at.saturating_add(len)).map(|(&b, _)| b).collect();
    for b in blocks {
        open(g, b);
    }
}

/// xmem mapped or unmapped `[at, at + len)`: what shows there is new, its
/// protection the map's; forget the guards there (reported written next).
pub(crate) fn remapped(at: usize, len: usize) {
    let mut slot = lock();
    let Some(g) = slot.as_mut() else { return };
    let blocks: Vec<usize> = g.alias_of.range(at & !(BLOCK - 1)..at.saturating_add(len)).map(|(&b, _)| b).collect();
    for b in blocks {
        let Some(owner) = g.alias_of.remove(&b) else { continue };
        if let Some(mut aliases) = g.clean.remove(&owner) {
            // The other aliases are still read-only by our hand: give them back.
            aliases.retain(|&(a, _)| a != b);
            for (a, old) in aliases {
                if g.alias_of.remove(&a).is_some() && !(at..at.saturating_add(len)).contains(&a) {
                    let mut was = 0;
                    unsafe { VirtualProtect(a as *const c_void, BLOCK, old, &mut was) };
                }
            }
        }
    }
}

/// Unmapping the view at `address`: forget the guards over its region.
pub(crate) fn unmapping(address: usize) {
    if !enabled() {
        return;
    }
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    if unsafe { VirtualQuery(address as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0 {
        remapped(address, (info.BaseAddress as usize + info.RegionSize).saturating_sub(address).max(BLOCK));
    }
}

fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    unsafe extern "system" fn on_write(info: *mut EXCEPTION_POINTERS) -> i32 {
        let r = unsafe { &*(*info).ExceptionRecord };
        if r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS || r.NumberParameters < 2 || r.ExceptionInformation[0] != 1 {
            return CONTINUE_SEARCH;
        }
        let at = r.ExceptionInformation[1];
        // try_lock spin: the faulting thread may hold the lock (a query's own
        // write cannot fault: it writes nothing guarded).
        for _ in 0..10_000 {
            if let Ok(mut slot) = GUARDS.try_lock() {
                let ours = slot.as_mut().is_some_and(|g| open(g, at));
                if ours {
                    static FAULTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    FAULTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                return if ours { CONTINUE_EXECUTION } else { CONTINUE_SEARCH };
            }
            std::thread::yield_now();
        }
        CONTINUE_SEARCH
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_write));
        crate::loader::log("section watch: guarding taken 64 KiB blocks of title views read-only to find CPU writes (TUXBOX_SECTION_WATCH=0 off)");
    });
}
