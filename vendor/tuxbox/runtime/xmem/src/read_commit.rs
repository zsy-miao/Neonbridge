//! A file read into title memory that fails with ERROR_NOACCESS (998) gets
//! its buffer made writable and is read again (leonida 779f9010, re-derived).
//! The kernel writes the buffer without raising a user-mode fault, so a page
//! that is only reserved, guarded, or briefly unmapped by a split
//! (physical_pages::split_retry) fails the read, and the retail file reader
//! treats 998 as transient: Sleep(1) and retry forever. Before the second
//! read, reserved private pages are committed read-write and guard bits are
//! cleared. TUXBOX_READ_COMMIT=0 answers the first failure.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{ERROR_NOACCESS, GetLastError, SetLastError};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_PRIVATE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE,
    VirtualAlloc, VirtualProtect, VirtualQuery,
};

unsafe extern "system" {
    fn ReadFile(file: *mut c_void, buffer: *mut c_void, bytes: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
}

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_READ_COMMIT").map_or(true, |v| v.trim() != "0"))
}

/// What to do with one region before the second read.
#[derive(Debug, PartialEq)]
enum Fix {
    Commit,
    Unguard(u32),
    Leave,
}

fn fix(region: &MEMORY_BASIC_INFORMATION) -> Fix {
    if region.State == MEM_RESERVE && region.Type == MEM_PRIVATE && region.AllocationProtect != PAGE_NOACCESS {
        Fix::Commit
    } else if region.State == MEM_COMMIT && region.Protect & PAGE_GUARD != 0 {
        Fix::Unguard(region.Protect & !PAGE_GUARD)
    } else {
        Fix::Leave
    }
}

/// Makes [buffer, buffer + bytes) writable where it can; returns the bytes
/// changed.
unsafe fn prepare(buffer: usize, bytes: usize) -> usize {
    let end = buffer.saturating_add(bytes);
    let mut at = buffer;
    let mut changed = 0;
    while at < end {
        let mut region: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { VirtualQuery(at as _, &mut region, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        let next = region.BaseAddress as usize + region.RegionSize;
        let from = at.max(region.BaseAddress as usize);
        let len = next.min(end) - from;
        match fix(&region) {
            Fix::Commit => {
                if !unsafe { VirtualAlloc(from as _, len, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
                    changed += len;
                }
            }
            Fix::Unguard(protect) => {
                let mut old = 0;
                if unsafe { VirtualProtect(from as _, len, protect, &mut old) } != 0 {
                    changed += len;
                }
            }
            Fix::Leave => {}
        }
        at = next;
    }
    changed
}

pub(crate) unsafe extern "system" fn read_file(
    file: *mut c_void, buffer: *mut c_void, bytes: u32, read: *mut u32, overlapped: *mut c_void,
) -> i32 {
    // The kernel writes the buffer without faulting, so a reserved page of a
    // lazy title view (lazy_commit.rs) fails the read: commit them first. An
    // overlapped read cannot be retried below, since it returns IO_PENDING and
    // reports the 998 at completion, so this is its only chance.
    if !buffer.is_null() && enabled() {
        let pages = crate::lazy_commit::ensure(buffer as usize, bytes as usize);
        static FIRST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = if pages > 0 { FIRST.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1 } else { 0 };
        if n != 0 && (n <= 8 || n.is_power_of_two()) {
            crate::loader::log(&format!(
                "ReadFile into {buffer:p}+{bytes:#x}: committed {pages} lazy 64 KiB page(s) before the read ({n} reads so far)"));
        }
    }
    let ok = unsafe { ReadFile(file, buffer, bytes, read, overlapped) };
    if ok != 0 || buffer.is_null() || !enabled() || unsafe { GetLastError() } != ERROR_NOACCESS {
        return ok;
    }
    // Lazy title views (lazy_commit.rs) are reserved mapped pages: commit them too.
    let lazy = crate::lazy_commit::ensure(buffer as usize, bytes as usize) * 0x1_0000;
    let changed = lazy + unsafe { prepare(buffer as usize, bytes as usize) };
    // A split's gap closes within microseconds; let it finish.
    std::thread::yield_now();
    let again = unsafe { ReadFile(file, buffer, bytes, read, overlapped) };
    let error = unsafe { GetLastError() };
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::loader::log(&format!(
            "ReadFile into {buffer:p}+{bytes:#x} failed 998: made {changed:#x} bytes writable, read again: {} ({n} so far; TUXBOX_READ_COMMIT=0 off)",
            if again != 0 { "ok".to_owned() } else { format!("error {error}") }));
    }
    unsafe { SetLastError(error) };
    again
}

/// Routes a title DLL's own ReadFile imports through `read_file`.
/// Bink2Scarlett.dll imports ReadFile from api-ms-win-core-file-l1-1-0 and
/// reads its movie stream into xmem memory; only the exe's import was hooked,
/// so since lazy commit (49702da) its reads into title views no CPU had
/// touched failed in the kernel (998), and the decoder ran on a stream with
/// holes: the intro movies' block smear from test137 on.
pub(crate) unsafe fn patch_module(module: *mut c_void) -> usize {
    if module.is_null() || !enabled() {
        return 0;
    }
    unsafe {
        crate::loader::patch_module_imports(
            module.cast(),
            |d| d.starts_with("api-ms-win-core-file-") || d == "kernel32.dll" || d == "kernelbase.dll",
            &[("ReadFile", read_file as *const () as usize)],
            &[],
        )
    }
}

/// Routes ucrtbase's ReadFile through `read_file`. The exe reads through the
/// CRT too (it imports api-ms-win-crt-stdio: fread, _read), and ucrtbase's
/// own ReadFile import was not hooked, so a CRT read into a lazy title view
/// no CPU had touched failed in the kernel (998) and came back short (video
/// atlas gap 1: the hook covered the exe's and title DLLs' imports only; xcl
/// inline-hooks kernelbase!ReadFile, vfs_hooks.rs:529-537, GPL-3.0, credit
/// to its authors). TUXBOX_READ_COMMIT_CRT=0 leaves ucrtbase alone.
pub(crate) unsafe fn install_crt() {
    if !enabled() || std::env::var("TUXBOX_READ_COMMIT_CRT").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    let name: Vec<u16> = "ucrtbase.dll".encode_utf16().chain([0]).collect();
    let ucrt = unsafe { GetModuleHandleW(name.as_ptr()) };
    let n = unsafe { patch_module(ucrt) };
    crate::loader::log(&format!("read commit: {n} ucrtbase ReadFile import(s) commit lazy title pages first (TUXBOX_READ_COMMIT_CRT=0 skips)"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(state: u32, kind: u32, allocation: u32, protect: u32) -> MEMORY_BASIC_INFORMATION {
        let mut r: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        (r.State, r.Type, r.AllocationProtect, r.Protect) = (state, kind, allocation, protect);
        r
    }

    #[test]
    fn reserved_private_pages_are_committed_but_placeholders_are_left() {
        assert_eq!(fix(&region(MEM_RESERVE, MEM_PRIVATE, PAGE_READWRITE, 0)), Fix::Commit);
        assert_eq!(fix(&region(MEM_RESERVE, MEM_PRIVATE, PAGE_NOACCESS, 0)), Fix::Leave);
        assert_eq!(fix(&region(MEM_COMMIT, MEM_PRIVATE, PAGE_READWRITE, PAGE_READWRITE)), Fix::Leave);
    }

    #[test]
    fn guarded_pages_lose_only_the_guard_bit() {
        assert_eq!(fix(&region(MEM_COMMIT, MEM_PRIVATE, PAGE_READWRITE, PAGE_READWRITE | PAGE_GUARD)), Fix::Unguard(PAGE_READWRITE));
    }

    #[test]
    fn a_reserved_buffer_becomes_readable_after_prepare() {
        let base = unsafe { VirtualAlloc(std::ptr::null(), 0x2000, MEM_RESERVE, PAGE_READWRITE) } as usize;
        assert_ne!(base, 0);
        assert_eq!(unsafe { prepare(base + 0x10, 0x100) }, 0x1000);
        unsafe { (base as *mut u8).add(0x10).write_volatile(7) };
    }
}
