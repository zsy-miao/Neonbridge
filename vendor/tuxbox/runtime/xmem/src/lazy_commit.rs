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
    MEM_COMMIT, MEM_MAPPED, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc, VirtualQuery,
};

const PAGE: usize = 0x1_0000;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// Title view ranges mapped without commit (start -> end), merged; never shrunk
/// (an entry only lets a reserved *mapped* page inside it be committed).
static VIEWS: RwLock<BTreeMap<usize, usize>> = RwLock::new(BTreeMap::new());
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
    !unsafe { VirtualAlloc(page as *const c_void, PAGE, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    let r = unsafe { &*(*info).ExceptionRecord };
    if r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS || r.NumberParameters < 2 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let page = r.ExceptionInformation[1] & !(PAGE - 1);
    if !unsafe { commit_page(page) } {
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

/// For other modules (d3d12 before a GPU import, xcrdapi before a file read):
/// commits the lazy pages of `[address, address + bytes)`; returns how many.
#[unsafe(export_name = "TuxBoxCommitRange")]
unsafe extern "system" fn commit_range(address: usize, bytes: usize) -> usize {
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
            UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: a as _ });
            UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: b as _ });
            windows_sys::Win32::Foundation::CloseHandle(section);
        }
    }
}
