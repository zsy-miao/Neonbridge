//! 6ixpack: the console's `G:` drive (the title's install root, e.g. `G:/appdata0.rpf`)
//! is the folder that holds the exe. The exe's path-taking file imports are patched
//! so `G:\...` / `G:/...` resolve there; every rewrite is logged (TUXBOX_LOG).
//! The console's `D:` drive (scratch; the title writes `D:/LocalDumps`) is a
//! per-title folder under %LOCALAPPDATA%\TuxBox\D.

use std::ffi::c_void;

use crate::loader::{exe_dir, log, patch_imports};

type H = *mut c_void;

unsafe extern "system" {
    fn CreateFileW(p: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: H) -> H;
    fn CreateDirectoryW(p: *const u16, sa: *mut c_void) -> i32;
    fn DeleteFileW(p: *const u16) -> i32;
    fn RemoveDirectoryW(p: *const u16) -> i32;
    fn FindFirstFileW(p: *const u16, data: *mut c_void) -> H;
    fn GetFileAttributesW(p: *const u16) -> u32;
    fn GetFileAttributesA(p: *const u8) -> u32;
    fn GetFileAttributesExW(p: *const u16, level: i32, info: *mut c_void) -> i32;
    fn SetFileAttributesW(p: *const u16, attrs: u32) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}

const SEP: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;

/// The per-title `D:` folder, %LOCALAPPDATA%\TuxBox\D\<exe name>\, with a
/// trailing separator; created on first use.
fn d_root() -> Option<&'static [u16]> {
    static ROOT: std::sync::OnceLock<Option<Vec<u16>>> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::var("LOCALAPPDATA").ok()?;
        let exe = std::env::current_exe().ok()?.file_stem()?.to_string_lossy().into_owned();
        let root = format!(r"{base}\TuxBox\D\{exe}\");
        std::fs::create_dir_all(&root).ok()?;
        log(&format!(r"D:\ maps to {root}"));
        Some(root.encode_utf16().collect())
    })
    .as_deref()
}

/// `G:` (exe folder) / `D:` (per-title folder) path -> NUL-terminated host
/// path; `None` leaves it alone.
fn map(p: *const u16) -> Option<Vec<u16>> {
    if p.is_null() {
        return None;
    }
    let (d, c) = unsafe { (*p, *p.add(1)) };
    if c != b':' as u16 {
        return None;
    }
    let root = match d | 0x20 {
        0x67 => exe_dir()?, // g
        0x64 => d_root()?,  // d
        _ => return None,
    };
    let s = unsafe { *p.add(2) };
    if !(s == 0 || s == SEP || s == SLASH) {
        return None;
    }
    let mut n = 2;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    let rest = unsafe { std::slice::from_raw_parts(p.add(2), n - 2) };
    let rest = if rest.first().is_some_and(|&c| c == SEP || c == SLASH) { &rest[1..] } else { rest };
    let mut out = root.to_vec();
    out.extend(rest.iter().map(|&c| if c == SLASH { SEP } else { c }));
    // Streaming opens files all game long: the first rewrites show the mapping,
    // later ones only a power-of-two heartbeat.
    static REWRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = REWRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if count <= 64 || count.is_power_of_two() {
        log(&format!("{} -> {} ({count} so far)", String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) }), String::from_utf16_lossy(&out)));
    }
    out.push(0);
    Some(out)
}

/// Run `f` with the mapped path (or the original).
fn with<R>(p: *const u16, f: impl FnOnce(*const u16) -> R) -> R {
    match map(p) {
        Some(m) => f(m.as_ptr()),
        None => f(p),
    }
}

unsafe extern "system" fn create_file_w(p: *const u16, a: u32, s: u32, sa: *mut c_void, d: u32, fl: u32, t: H) -> H {
    with(p, |p| unsafe { CreateFileW(p, a, s, sa, d, fl, t) })
}
unsafe extern "system" fn create_directory_w(p: *const u16, sa: *mut c_void) -> i32 {
    with(p, |p| unsafe { CreateDirectoryW(p, sa) })
}
unsafe extern "system" fn delete_file_w(p: *const u16) -> i32 {
    with(p, |p| unsafe { DeleteFileW(p) })
}
unsafe extern "system" fn remove_directory_w(p: *const u16) -> i32 {
    with(p, |p| unsafe { RemoveDirectoryW(p) })
}
unsafe extern "system" fn find_first_file_w(p: *const u16, d: *mut c_void) -> H {
    with(p, |p| unsafe { FindFirstFileW(p, d) })
}
unsafe extern "system" fn get_file_attributes_w(p: *const u16) -> u32 {
    with(p, |p| unsafe { GetFileAttributesW(p) })
}
unsafe extern "system" fn get_file_attributes_a(p: *const u8) -> u32 {
    if p.is_null() {
        return unsafe { GetFileAttributesA(p) };
    }
    let w: Vec<u16> = unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().encode_utf16().chain([0]).collect();
    match map(w.as_ptr()) {
        Some(m) => unsafe { GetFileAttributesW(m.as_ptr()) },
        None => unsafe { GetFileAttributesA(p) },
    }
}
unsafe extern "system" fn get_file_attributes_ex_w(p: *const u16, l: i32, i: *mut c_void) -> i32 {
    with(p, |p| unsafe { GetFileAttributesExW(p, l, i) })
}
unsafe extern "system" fn set_file_attributes_w(p: *const u16, a: u32) -> i32 {
    with(p, |p| unsafe { SetFileAttributesW(p, a) })
}
unsafe extern "system" fn move_file_ex_w(from: *const u16, to: *const u16, f: u32) -> i32 {
    with(from, |from| with(to, |to| unsafe { MoveFileExW(from, to, f) }))
}

pub(crate) unsafe fn install() {
    let n = unsafe {
        patch_imports(
            |d| d.starts_with("api-ms-win-core-file-") || d == "kernel32.dll" || d == "kernelbase.dll",
            &[
                ("CreateFileW", create_file_w as *const () as usize),
                ("CreateDirectoryW", create_directory_w as *const () as usize),
                ("DeleteFileW", delete_file_w as *const () as usize),
                ("RemoveDirectoryW", remove_directory_w as *const () as usize),
                ("FindFirstFileW", find_first_file_w as *const () as usize),
                ("GetFileAttributesW", get_file_attributes_w as *const () as usize),
                ("GetFileAttributesA", get_file_attributes_a as *const () as usize),
                ("GetFileAttributesExW", get_file_attributes_ex_w as *const () as usize),
                ("SetFileAttributesW", set_file_attributes_w as *const () as usize),
                ("MoveFileExW", move_file_ex_w as *const () as usize),
            ],
        )
    };
    log(&format!("{n} file import(s) patched; G:\\ maps to the exe folder, D:\ to a per-title folder"));
}
