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

thread_local! {
    /// This thread holds GUARDS: a fault it takes cannot wait on the lock.
    static HOLDING: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

struct Held(std::sync::MutexGuard<'static, Option<Guards>>);

impl core::ops::Deref for Held {
    type Target = Option<Guards>;
    fn deref(&self) -> &Option<Guards> { &self.0 }
}
impl core::ops::DerefMut for Held {
    fn deref_mut(&mut self) -> &mut Option<Guards> { &mut self.0 }
}
impl Drop for Held {
    fn drop(&mut self) { HOLDING.with(|h| h.set(false)); }
}

/// GUARDS is held only for short bookkeeping and one block's protection
/// changes at a time, never a whole query walk: the write handler waits on
/// it, and test174's query held it while guarding Bink's 8 MiB frame
/// buffer, so the handler's spin gave up and two writes died as AVs.
fn lock() -> Held {
    let g = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
    HOLDING.with(|h| h.set(true));
    Held(g)
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
/// Census (sampled at queries #1,2,4,... and every 4096): why blocks are
/// reported written or left unguarded, for test176's "guards skip nothing".
mod census {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub(super) static NOT_VIEW: AtomicU64 = AtomicU64::new(0);
    pub(super) static QUERIES: AtomicU64 = AtomicU64::new(0);
    pub(super) static CLEAN: AtomicU64 = AtomicU64::new(0);
    pub(super) static REPORTED: AtomicU64 = AtomicU64::new(0);
    pub(super) static GUARDED: AtomicU64 = AtomicU64::new(0);
    pub(super) static PINNED: AtomicU64 = AtomicU64::new(0);
    pub(super) static DEVICE: AtomicU64 = AtomicU64::new(0);
    pub(super) static NOT_COMMITTED: AtomicU64 = AtomicU64::new(0);
    pub(super) static NOT_RW: AtomicU64 = AtomicU64::new(0);
    pub(super) static LAST_PROTECT: AtomicU64 = AtomicU64::new(0);
    pub(super) static PROTECT_FAILED: AtomicU64 = AtomicU64::new(0);
    pub(super) static OPENED: AtomicU64 = AtomicU64::new(0);
    pub(super) fn add(c: &AtomicU64) { c.fetch_add(1, Relaxed); }
    pub(super) fn log() {
        let n = QUERIES.load(Relaxed);
        if !(n.is_power_of_two() || n % 4096 == 0) {
            return;
        }
        let g = |c: &AtomicU64| c.load(Relaxed);
        crate::loader::log(&format!(
            "section watch census: {n} queries ({} not a view), blocks {} clean (skipped) / {} reported; reported blocks {} guarded, unguarded: {} pinned, {} device pages, {} not committed, {} not read-write (last protect {:#x}), {} VirtualProtect failed; {} opened by a write",
            g(&NOT_VIEW), g(&CLEAN), g(&REPORTED), g(&GUARDED), g(&PINNED), g(&DEVICE), g(&NOT_COMMITTED), g(&NOT_RW), g(&LAST_PROTECT), g(&PROTECT_FAILED), g(&OPENED)));
    }
}

pub(crate) fn query(start: usize, end: usize, cap: usize) -> Option<Vec<usize>> {
    if enabled() && start < end && !is_view(start) {
        census::add(&census::NOT_VIEW);
    }
    if !enabled() || start >= end || !is_view(start) {
        return None;
    }
    census::add(&census::QUERIES);
    install();
    let mut out = Vec::new();
    // Blocks to report and guard, picked under a short hold.
    let mut taken = Vec::new();
    {
        let mut slot = lock();
        let g = slot.get_or_insert_with(Guards::default);
        let mut block = start & !(BLOCK - 1);
        while block < end {
            if g.clean.contains_key(&block) {
                census::add(&census::CLEAN);
            } else {
                census::add(&census::REPORTED);
                let from = block.max(start & !0xfff);
                let to = (block + BLOCK).min(end);
                let pages = (to - from).div_ceil(0x1000);
                if out.len() + pages > cap {
                    // Left unguarded: reported by the next query.
                    break;
                }
                out.extend((0..pages).map(|i| from + i * 0x1000));
                taken.push(block);
            }
            block += BLOCK;
        }
    }
    // Each block is guarded under its own hold; aliases are found and
    // filtered without it. A write between the report and the guard was
    // made before the caller reads the block, so it is not lost.
    for block in taken {
        if crate::device_pages::covers(block) {
            census::add(&census::DEVICE);
            continue;
        }
        if pinned(block) {
            census::add(&census::PINNED);
            continue;
        }
        let aliases: Vec<usize> = crate::physical_pages::aliases(block).into_iter()
            .map(|a| a & !(BLOCK - 1))
            .filter(|&a| a == block || !(crate::device_pages::covers(a) || pinned(a)))
            .collect();
        let mut slot = lock();
        let g = slot.get_or_insert_with(Guards::default);
        if !g.clean.contains_key(&block) {
            protect(g, block, &aliases);
        }
    }
    census::log();
    Some(out)
}

/// Makes `block` and its aliases read-only and notes it clean; a block
/// whose protection change fails stays unguarded (reported written again).
fn protect(g: &mut Guards, block: usize, aliases: &[usize]) {
    let mut done = Vec::new();
    // device_pages' and pinned aliases were left out by the caller.
    for &a in aliases {
        if g.alias_of.contains_key(&a) && a != block {
            continue;
        }
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(a as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0
            || info.State != MEM_COMMIT
            || info.Protect & 0xff != PAGE_READWRITE
        {
            if a == block {
                if info.State != MEM_COMMIT {
                    census::add(&census::NOT_COMMITTED);
                } else {
                    census::add(&census::NOT_RW);
                    census::LAST_PROTECT.store(u64::from(info.Protect), std::sync::atomic::Ordering::Relaxed);
                }
                break;
            }
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(a as *const c_void, BLOCK, (info.Protect & !0xff) | PAGE_READONLY, &mut old) } != 0 {
            done.push((a, old));
        } else if a == block {
            census::add(&census::PROTECT_FAILED);
            break;
        }
    }
    if done.iter().any(|&(a, _)| a == block) {
        for &(a, _) in &done {
            g.alias_of.insert(a, block);
        }
        g.clean.insert(block, done);
        census::add(&census::GUARDED);
    } else {
        for (a, old) in done {
            let mut was = 0;
            unsafe { VirtualProtect(a as *const c_void, BLOCK, old, &mut was) };
        }
    }
}

/// Ranges a kernel write may still be filling (overlapped ReadFile and
/// DirectStorage complete after the call returns): not guarded for 2 s.
static PINNED: Mutex<Vec<(usize, usize, std::time::Instant)>> = Mutex::new(Vec::new());

fn pinned(block: usize) -> bool {
    let mut p = PINNED.lock().unwrap_or_else(|e| e.into_inner());
    p.retain(|&(_, _, at)| at.elapsed() < std::time::Duration::from_secs(2));
    p.iter().any(|&(s, e, _)| s < block + BLOCK && block < e)
}

/// Before the kernel writes `[at, at + len)` (ReadFile, DirectStorage):
/// opens its guards and keeps them open for 2 s.
pub(crate) fn writing(at: usize, len: usize) {
    if !enabled() {
        return;
    }
    {
        let mut p = PINNED.lock().unwrap_or_else(|e| e.into_inner());
        p.retain(|&(_, _, t)| t.elapsed() < std::time::Duration::from_secs(2));
        if p.len() < 4096 {
            p.push((at, at.saturating_add(len), std::time::Instant::now()));
        }
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
        if let Some(aliases) = g.clean.remove(&owner) {
            // Every alias still read-only by our hand gets its protection
            // back, the remapped ones too: a remap that kept the old view
            // (a re-commit, a failed map) keeps our read-only, and with its
            // guard forgotten a write there was an unhandled AV (test174).
            for (a, old) in aliases {
                g.alias_of.remove(&a);
                let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
                let ours = unsafe { VirtualQuery(a as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0
                    && info.State == MEM_COMMIT
                    && info.Protect & 0xff == PAGE_READONLY;
                if ours {
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
        // Blocks on the lock: no holder writes guarded memory (a query or
        // remap only protects), so the holder always finishes. The 10k-yield
        // try_lock spin it replaces gave up while a long query held the lock
        // and passed a guarded write on as an AV (test174).
        if HOLDING.with(|h| h.get()) {
            // Faulted while holding GUARDS (it writes nothing guarded): not ours.
            return CONTINUE_SEARCH;
        }
        let mut slot = lock();
        let ours = slot.as_mut().is_some_and(|g| open(g, at));
        drop(slot);
        if ours {
            census::add(&census::OPENED);
        }
        if !ours && enabled() && is_view(at) {
            // A write AV on a view page we do not guard: log the first few.
            static LOST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if LOST.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
                crate::loader::log(&format!("section watch: write AV at {at:#x} on a view block we do not guard (passed on)"));
            }
        }
        if ours { CONTINUE_EXECUTION } else { CONTINUE_SEARCH }
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_write));
        crate::loader::log("section watch: guarding taken 64 KiB blocks of title views read-only to find CPU writes (TUXBOX_SECTION_WATCH=0 off)");
    });
}
