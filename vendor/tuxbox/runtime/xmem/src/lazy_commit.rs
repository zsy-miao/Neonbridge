//! Title views of physical pages are committed on first CPU use, not when mapped.
//!
//! Ported from ivall/xcl crates/xcl/src/hle/kernelx/lazy.rs (38b05b7, with the
//! race fix of 77905f6 by Logan; GPL-2.0-only, credited in docs/DETAILS.md). A
//! title maps its physical pages wherever it reserved address space and places
//! resources there, render targets and depth buffers too, which here live on
//! video-memory pages (d3d12 placement::pages); the CPU never touches those
//! pages. Committed at map time they doubled that memory: xcl's retail game had
//! 5.4 GB of mapped pages beside its video-memory targets (commit 28.7 -> 16 GB
//! with this), and test132/133 grew title commit 7.5 GB until the system had 301
//! MiB of commit left at present #3166.
//!
//! Re-derived for xmem's model: physical pages are SEC_RESERVE pagefile
//! sections, and a section page committed through any view is committed in all
//! of them, so a new view is mapped without committing (an uncommitted section
//! page reads as zeros, as a fresh physical page holds). The first CPU access to
//! such a page faults; the handler commits that 64 KiB page through the view and
//! the access goes on. A thread that loses the race finds the page committed and
//! goes on too (77905f6). The handler takes no lock xmem's map paths hold (the
//! view list is read with try_read), so a fault inside them cannot deadlock.
//! Kernel calls do not fault, they fail, so kernel-mode readers and writers of
//! title memory commit first: ReadFile's 998 retry (read_commit.rs), the
//! DirectStorage reads (xcrdapi) and GPU imports (d3d12 heap.rs), through
//! TuxBoxCommitRange. TUXBOX_LAZY_COMMIT=0 commits at map time as before.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Once, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_MAPPED, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY, VirtualAlloc, VirtualQuery,
};

const PAGE: usize = 0x1_0000;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// Title view ranges mapped without commit (start -> end), merged; never shrunk
/// (an entry only lets a reserved *mapped* page inside it be committed).
static VIEWS: RwLock<BTreeMap<usize, usize>> = RwLock::new(BTreeMap::new());
/// Each lazy view as mapped (start -> (end, generation)), exact: removed when
/// unmapped (`note_unmap`). The generation tells a remap at the same address
/// from the view a walk started on.
static MAPPED: RwLock<BTreeMap<usize, (usize, u64)>> = RwLock::new(BTreeMap::new());
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// test137: every new buffer view, vertex buffer and import walked its range
/// with VirtualQuery, and a VirtualQuery scans the page table to the end of
/// the region it lands in: measured 16 ms per GiB of a committed section view
/// (5 ms per GiB of committed private memory). The GPU page pool at
/// 0xdac000000 was one 5.4 GiB view, so each walk cost tens of ms per call,
/// several calls a bind, on every job thread, and the render thread waited 15
/// s on them. One bit per 64 KiB page of the title window says the page of a
/// current lazy view is known committed; set by a walk that saw it committed
/// (one VirtualQuery per run, then never again) and cleared when a view is
/// mapped or unmapped there. Unset means unknown, never "reserved".
/// TUXBOX_COMMIT_BITS=0 walks with VirtualQuery every time, as before.
const LIMIT: usize = 0x20_0000_0000;
static KNOWN: [AtomicU64; LIMIT / PAGE / 64] = [const { AtomicU64::new(0) }; LIMIT / PAGE / 64];

fn bits_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_COMMIT_BITS").map_or(true, |v| v.trim() != "0"))
}

fn known(page: usize) -> bool {
    let i = page / PAGE;
    page < LIMIT && KNOWN[i / 64].load(Ordering::Acquire) & (1 << (i % 64)) != 0
}

fn set_bits(lo: usize, hi: usize, on: bool) {
    let mut p = lo & !(PAGE - 1);
    while p < hi.min(LIMIT) {
        let i = p / PAGE;
        if on {
            KNOWN[i / 64].fetch_or(1 << (i % 64), Ordering::AcqRel);
        } else {
            KNOWN[i / 64].fetch_and(!(1 << (i % 64)), Ordering::AcqRel);
        }
        p += PAGE;
    }
}

/// Marks `[lo, hi)` known committed if the view `start` of generation `generation` is still mapped.
fn mark(start: usize, generation: u64, lo: usize, hi: usize) {
    let views = MAPPED.read().unwrap_or_else(|e| e.into_inner());
    if views.get(&start).is_some_and(|&(end, g)| g == generation && hi <= end) {
        set_bits(lo, hi, true);
    }
}

/// The view at `address` was unmapped (shared_pages::unmap).
pub(crate) fn note_unmap(address: usize) {
    let mut views = MAPPED.write().unwrap_or_else(|e| e.into_inner());
    if let Some((end, _)) = views.remove(&address) {
        set_bits(address, end, false);
    }
}

static FAULTED: AtomicU64 = AtomicU64::new(0);
static ENSURED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_LAZY_COMMIT").map_or(true, |v| v.trim() != "0"))
}

fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(handler));
        crate::loader::log("lazy commit: title views of physical pages commit on first use (xcl 38b05b7; TUXBOX_LAZY_COMMIT=0 commits at map)");
    });
}

/// `[start, start + bytes)` was mapped as a view without committing it.
pub(crate) fn note_view(start: usize, bytes: usize) {
    install();
    {
        let mut mapped = MAPPED.write().unwrap_or_else(|e| e.into_inner());
        set_bits(start, start + bytes, false);
        mapped.insert(start, (start + bytes, GENERATION.fetch_add(1, Ordering::Relaxed)));
    }
    let mut views = VIEWS.write().unwrap_or_else(|e| e.into_inner());
    let (mut lo, mut hi) = (start, start + bytes);
    let overlapping: Vec<(usize, usize)> = views.range(..=hi).rev().take_while(|&(_, &e)| e >= lo).map(|(&s, &e)| (s, e)).collect();
    for (s, e) in overlapping {
        views.remove(&s);
        lo = lo.min(s);
        hi = hi.max(e);
    }
    views.insert(lo, hi);
}

/// Whether `at` is inside a lazy view, without blocking a writer.
fn in_views(at: usize) -> bool {
    for _ in 0..1000 {
        if let Ok(views) = VIEWS.try_read() {
            return views.range(..=at).next_back().is_some_and(|(_, &end)| at < end);
        }
        std::thread::yield_now();
    }
    false
}

fn query(at: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    (unsafe { VirtualQuery(at as *const c_void, &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0).then_some(m)
}

/// Commits the 64 KiB page at `page` if it is a reserved page of a lazy view;
/// true if it is committed now.
unsafe fn commit_page(page: usize) -> bool {
    let Some(m) = query(page) else { return false };
    if m.State == MEM_COMMIT {
        // Another thread won the race (77905f6).
        return m.Protect & (PAGE_GUARD | PAGE_NOACCESS) == 0 && m.Protect != 0;
    }
    if m.State != MEM_RESERVE || m.Type != MEM_MAPPED || !in_views(page) {
        return false;
    }
    if !unsafe { VirtualAlloc(page as *const c_void, PAGE, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
        return true;
    }
    // The handler then passes the fault on and the title dies of an access
    // violation with nothing said: commit exhausted is the usual cause
    // (memory atlas gap 2; VIctory xmem.rs:841-883 logs it, GPL-3.0, credit
    // to its authors).
    static FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = FAILED.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::loader::log(&format!(
            "lazy commit: the title page {page:#x} could not be committed on first use ({}); the access faults ({n} so far)",
            std::io::Error::last_os_error()));
    }
    false
}

fn allows_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_LAZY_ALLOWS").map_or(true, |v| v.trim() != "0"))
}

/// commit_page for device_pages: also true for a committed page whatever its
/// protection (a device-backed page d3d12 protected).
pub(crate) unsafe fn commit_page_any(page: usize) -> bool {
    let Some(m) = query(page) else { return false };
    if m.State == MEM_COMMIT {
        return true;
    }
    unsafe { commit_page(page) }
}

/// Whether the committed page at `at` permits `access` (ExceptionInformation[0]:
/// 0 read, 1 write, 8 execute); copied from xcl lazy.rs `allows`.
unsafe fn allows(access: usize, at: usize) -> bool {
    let Some(m) = query(at) else { return false };
    if m.State != MEM_COMMIT || m.Protect & PAGE_GUARD != 0 {
        return false;
    }
    let writes = PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    let reads = writes | PAGE_READONLY | PAGE_EXECUTE_READ;
    match access {
        0 => m.Protect & reads != 0,
        1 => m.Protect & writes != 0,
        _ => false,
    }
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    let r = unsafe { &*(*info).ExceptionRecord };
    if r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS || r.NumberParameters < 2 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let page = r.ExceptionInformation[1] & !(PAGE - 1);
    if unsafe { crate::device_pages::fault(page, r.ExceptionInformation[0]) } {
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    if !unsafe { commit_page(page) } {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // Resume only when the page's protection now permits this access: a write
    // to a read-only page or an execute (DEP) fault on a committed page would
    // re-fault forever, so it is passed on (xcl lazy.rs `allows`, 77905f6,
    // GPL-2.0-only, credit to ivall/Logan). TUXBOX_LAZY_ALLOWS=0 resumes on any
    // committed page as before.
    if allows_on() && !unsafe { allows(r.ExceptionInformation[0], r.ExceptionInformation[1]) } {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let n = FAULTED.fetch_add(1, Ordering::Relaxed) + 1;
    if n.is_power_of_two() && n >= 256 {
        crate::loader::log(&format!("lazy commit: {n} title pages committed on first CPU use ({} MiB), {} by kernel-call commits",
            n * PAGE as u64 >> 20, ENSURED.load(Ordering::Relaxed)));
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// Commits the reserved lazy-view pages of `[address, address + bytes)`: memory a
/// kernel call or the GPU is about to read or write. Returns the pages committed.
pub(crate) fn ensure(address: usize, bytes: usize) -> usize {
    if bytes == 0 || VIEWS.read().map_or(true, |v| v.is_empty()) {
        return 0;
    }
    if bits_on() {
        return ensure_known(address, bytes);
    }
    let (mut at, end) = (address & !(PAGE - 1), address.saturating_add(bytes));
    let mut committed = 0;
    while at < end {
        let Some(m) = query(at) else { break };
        let next = (m.BaseAddress as usize).saturating_add(m.RegionSize).max(at + PAGE);
        if m.State == MEM_RESERVE && m.Type == MEM_MAPPED {
            let mut p = at;
            while p < next.min(end) {
                if unsafe { commit_page(p) } {
                    committed += 1;
                }
                p += PAGE;
            }
        }
        at = next;
    }
    ENSURED.fetch_add(committed as u64, Ordering::Relaxed);
    committed
}

/// The current lazy views overlapping `[lo, hi)`, as (start, end, generation).
fn overlapping(lo: usize, hi: usize) -> Vec<(usize, usize, u64)> {
    let views = MAPPED.read().unwrap_or_else(|e| e.into_inner());
    let mut found: Vec<_> = views.range(..hi).rev().take_while(|&(_, &(end, _))| end > lo)
        .map(|(&s, &(e, g))| (s, e, g)).collect();
    found.reverse();
    found
}

/// `ensure` with the known-committed bits: pages already known committed cost
/// a bit test, and a run seen committed costs one VirtualQuery for all of it.
/// Only pages of current lazy views are touched, as before (commit_page
/// required in_views); other memory is left to the caller's own check.
fn ensure_known(address: usize, bytes: usize) -> usize {
    let end = address.saturating_add(bytes);
    let mut committed = 0;
    for (start, view_end, generation) in overlapping(address, end) {
        let (mut p, hi) = (address.max(start) & !(PAGE - 1), end.min(view_end));
        while p < hi {
            if known(p) {
                p += PAGE;
                continue;
            }
            let Some(m) = query(p) else { break };
            let region_end = (m.BaseAddress as usize).saturating_add(m.RegionSize).min(view_end).max(p + PAGE);
            if m.State == MEM_COMMIT {
                if m.Protect & (PAGE_GUARD | PAGE_NOACCESS) == 0 && m.Protect != 0 {
                    mark(start, generation, p, region_end);
                }
                p = region_end;
            } else if m.State == MEM_RESERVE && m.Type == MEM_MAPPED {
                // Only the pages asked for are committed; the rest of the run stays reserved.
                let stop = region_end.min(hi.next_multiple_of(PAGE));
                while p < stop {
                    if !unsafe { VirtualAlloc(p as *const c_void, PAGE, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
                        mark(start, generation, p, p + PAGE);
                        committed += 1;
                    }
                    p += PAGE;
                }
            } else {
                p = region_end;
            }
        }
    }
    ENSURED.fetch_add(committed as u64, Ordering::Relaxed);
    committed
}

/// Flags for TuxBoxViewState.
const COVERED: u32 = 1;
const COMMITTED: u32 = 2;
const ONE_VIEW: u32 = 4;

/// What xmem knows of `[address, address + bytes)` without a VirtualQuery:
/// COVERED when current lazy views (read-write section views) cover every
/// byte, COMMITTED when every page of it is known committed, ONE_VIEW when one
/// view holds it all (its start and size are written then, the range's OS
/// allocation). 0: not lazy-view memory, ask the OS.
pub(crate) fn view_state(address: usize, bytes: usize, base: &mut usize, size: &mut usize) -> u32 {
    if bytes == 0 || !bits_on() {
        return 0;
    }
    let end = address.saturating_add(bytes);
    let views = MAPPED.read().unwrap_or_else(|e| e.into_inner());
    let Some((&first, &(first_end, _))) = views.range(..=address).next_back() else { return 0 };
    if first_end <= address {
        return 0;
    }
    let (mut at, mut count) = (first, 0usize);
    let mut covered = false;
    for (&s, &(e, _)) in views.range(first..) {
        if s != at {
            break;
        }
        count += 1;
        at = e;
        if at >= end {
            covered = true;
            break;
        }
    }
    if !covered {
        return 0;
    }
    let mut flags = COVERED;
    let mut p = address & !(PAGE - 1);
    let mut all = true;
    while p < end {
        if !known(p) {
            all = false;
            break;
        }
        p += PAGE;
    }
    if all {
        flags |= COMMITTED;
    }
    if count == 1 {
        flags |= ONE_VIEW;
        (*base, *size) = (first, first_end - first);
    }
    flags
}

/// Why a title page is unreadable, for a consumer that found it so (d3d12's
/// shadow fill): 1 = inside a current lazy title view and not yet committed
/// (the CPU never touched it; its content is zeros); 2 = xmem unmapped or
/// decommitted it (out = site, start, size, age ms, as TuxBoxUnmapTrace);
/// 0 = neither (out[0] = region state, out[1] = type, out[2] = allocation
/// base from VirtualQuery).
#[unsafe(export_name = "TuxBoxPageWhy")]
unsafe extern "system" fn page_why(address: usize, out: *mut [u64; 4]) -> u32 {
    let mut o = [0u64; 4];
    let page = address & !(PAGE - 1);
    let (mut b, mut s) = (0, 0);
    let flags = view_state(page, PAGE, &mut b, &mut s);
    let why = if flags & COVERED != 0 && flags & COMMITTED == 0 {
        1
    } else if unsafe { crate::unmap_trace::newest(page, PAGE, &mut o) } {
        2
    } else {
        if let Some(m) = query(page) {
            o = [m.State as u64, m.Type as u64, m.AllocationBase as u64, 0];
        }
        0
    };
    if !out.is_null() {
        unsafe { out.write(o) };
    }
    why
}

#[unsafe(export_name = "TuxBoxViewState")]
unsafe extern "system" fn view_state_export(address: usize, bytes: usize, base: *mut usize, size: *mut usize) -> u32 {
    let (mut b, mut s) = (0, 0);
    let flags = view_state(address, bytes, &mut b, &mut s);
    if !base.is_null() {
        unsafe { base.write(b) };
    }
    if !size.is_null() {
        unsafe { size.write(s) };
    }
    flags
}

/// For other modules (d3d12 before a GPU import, xcrdapi before a file read):
/// commits the lazy pages of `[address, address + bytes)`; returns how many.
#[unsafe(export_name = "TuxBoxCommitRange")]
unsafe extern "system" fn commit_range(address: usize, bytes: usize) -> usize {
    crate::device_pages::open(address, bytes);
    ensure(address, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, SEC_RESERVE, UnmapViewOfFile};

    /// xcl 38b05b7/77905f6's tests on a SEC_RESERVE view: a lazy page reads as
    /// zeros on first touch and keeps what is written, a second view sees it, and
    /// eight threads first-touching the same pages all go on.
    #[test]
    fn a_lazy_view_commits_on_first_use_and_threads_race_safely() {
        const PAGES: usize = 64;
        unsafe {
            let section = CreateFileMappingW(INVALID_HANDLE_VALUE, ptr::null(), PAGE_READWRITE | SEC_RESERVE, 0, (PAGES * PAGE) as u32, ptr::null());
            assert!(!section.is_null());
            let a = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, PAGES * PAGE).Value as usize;
            let b = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, PAGES * PAGE).Value as usize;
            note_view(a, PAGES * PAGE);
            note_view(b, PAGES * PAGE);
            assert_eq!(ptr::read_volatile((a + 8) as *const u32), 0);
            ptr::write_volatile((a + 8) as *mut u32, 7);
            assert_eq!(ptr::read_volatile((b + 8) as *const u32), 7, "committed through one view, seen in the other");
            assert_eq!(ensure(b + PAGE + 4, 4), 1);
            let (mut base, mut size) = (0, 0);
            assert_eq!(view_state(b + PAGE + 4, 4, &mut base, &mut size), COVERED | COMMITTED | ONE_VIEW);
            assert_eq!((base, size), (b, PAGES * PAGE));
            assert_eq!(view_state(b + 3 * PAGE, 4, &mut base, &mut size) & COMMITTED, 0, "an untouched page is not known committed");
            let threads: Vec<_> = (0..8usize).map(|t| std::thread::spawn(move || {
                for i in 2..PAGES {
                    ptr::write_volatile((a + i * PAGE + t * 64) as *mut u64, (i * 8 + t) as u64);
                }
            })).collect();
            for t in threads {
                t.join().unwrap();
            }
            for i in 2..PAGES {
                for t in 0..8 {
                    assert_eq!(ptr::read_volatile((b + i * PAGE + t * 64) as *const u64), (i * 8 + t) as u64);
                }
            }
            note_unmap(b);
            assert_eq!(view_state(b + PAGE + 4, 4, &mut base, &mut size), 0, "an unmapped view is forgotten");
            UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: a as _ });
            UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: b as _ });
            windows_sys::Win32::Foundation::CloseHandle(section);
        }
    }
}
