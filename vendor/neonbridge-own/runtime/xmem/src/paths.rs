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
    fn CreateFileA(p: *const u8, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: H) -> H;
    fn DeleteFileA(p: *const u8) -> i32;
    fn CreateFile2(p: *const u16, access: u32, share: u32, disp: u32, params: *const c_void) -> H;
    fn FindFirstFileExW(p: *const u16, level: i32, data: *mut c_void, search: i32, filter: *mut c_void, flags: u32) -> H;
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, w: *mut u16, wn: i32) -> i32;
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
    if leader_paths() {
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        let path = unsafe { std::slice::from_raw_parts(p, len) };
        let out = leader::translate(path)?;
        static REWRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = REWRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if count <= 64 || count.is_power_of_two() {
            log(&format!("{} -> {} ({count} so far)", String::from_utf16_lossy(path), String::from_utf16_lossy(&out[..out.len() - 1])));
        }
        return Some(out);
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

/// TUXBOX_LEADER_PATHS (default on, =0 off): title paths translate as the
/// leader's do: a leading `\\?\` is kept, `X:` and `X:name` are relative to
/// the drive root, and a host path of 248 characters or more takes the `\\?\`
/// form with `.` and `..` resolved here (`..` stops at the root).
fn leader_paths() -> bool {
    std::env::var("TUXBOX_LEADER_PATHS").map_or(true, |v| v.trim() != "0")
}

/// Copied from ztrx victory-hle files.rs verbatim and translate (GPL-3.0,
/// credit to its authors); the drive table is ours (G: exe folder, D: title folder).
mod leader {
    const SEPARATORS: [u16; 2] = [b'\\' as u16, b'/' as u16];
    const VERBATIM: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const LONG: usize = 248;

    fn drives() -> Vec<(u16, Vec<u16>)> {
        let trim = |r: &[u16]| {
            let mut r = r.to_vec();
            while r.last() == Some(&u16::from(b'\\')) { r.pop(); }
            r
        };
        let mut out = Vec::new();
        if let Some(g) = super::exe_dir() { out.push((u16::from(b'G'), trim(g))); }
        if let Some(d) = super::d_root() { out.push((u16::from(b'D'), trim(d))); }
        out
    }

    fn verbatim(root: &[u16], tail: &[u16]) -> Vec<u16> {
        let backslash = u16::from(b'\\');
        let mut out = if root.starts_with(&VERBATIM) {
            root.to_vec()
        } else if let Some(unc) = root.strip_prefix(&[backslash; 2][..]) {
            let mut out = VERBATIM.to_vec();
            out.extend("UNC\\".encode_utf16());
            out.extend_from_slice(unc);
            out
        } else {
            [&VERBATIM[..], root].concat()
        };
        while out.last() == Some(&backslash) {
            out.pop();
        }
        let dot = u16::from(b'.');
        let mut parts: Vec<&[u16]> = Vec::new();
        for part in tail.split(|c| SEPARATORS.contains(c)) {
            match part {
                [] => {}
                [d] if *d == dot => {}
                [d, e] if [*d, *e] == [dot; 2] => drop(parts.pop()),
                _ => parts.push(part),
            }
        }
        for part in parts {
            out.push(backslash);
            out.extend_from_slice(part);
        }
        out
    }

    pub(super) fn translate(path: &[u16]) -> Option<Vec<u16>> {
        let (prefix, rest) = match path.strip_prefix(&VERBATIM[..]) {
            Some(rest) => (&VERBATIM[..], rest),
            None => (&[][..], path),
        };
        let &[letter, colon, ..] = rest else {
            return None;
        };
        if colon != u16::from(b':') {
            return None;
        }
        let letter = u8::try_from(letter).ok().filter(u8::is_ascii_alphabetic)?;
        let letter = u16::from(letter.to_ascii_uppercase());
        let drives = drives();
        let (_, root) = drives.iter().find(|(l, _)| *l == letter)?;
        let tail = &rest[2..];
        if prefix.is_empty() && root.len() + tail.len() >= LONG {
            let mut out = verbatim(root, tail);
            out.push(0);
            return Some(out);
        }
        let mut out = prefix.to_vec();
        out.extend_from_slice(root);
        if !tail.first().is_some_and(|c| SEPARATORS.contains(c)) {
            out.push(b'\\'.into());
        }
        let verbatim = !prefix.is_empty();
        out.extend(tail.iter().map(|&c| if verbatim && c == u16::from(b'/') { b'\\'.into() } else { c }));
        out.push(0);
        Some(out)
    }
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
/// TUXBOX_LEADER_FILES (default on, =0 off): narrow paths decode in the ANSI
/// code page (not UTF-8), and CreateFileA, DeleteFileA, CreateFile2 and
/// FindFirstFileExW map title drives too. ztrx victory-hle files.rs narrow,
/// CreateFileA, DeleteFileA, CreateFile2, FindFirstFileExW (GPL-3.0, credit to
/// its authors).
fn leader_files() -> bool {
    std::env::var("TUXBOX_LEADER_FILES").map_or(true, |v| v.trim() != "0")
}

/// A narrow path as a NUL-terminated wide one (ztrx files.rs narrow).
fn narrow(p: *const u8) -> Vec<u16> {
    let bytes = unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_bytes();
    if !leader_files() {
        return String::from_utf8_lossy(bytes).encode_utf16().chain([0]).collect();
    }
    let len = bytes.len() as i32;
    // SAFETY: sizing call, then a call with room for `n` units (CP_ACP = 0).
    let mut wide = unsafe {
        let n = MultiByteToWideChar(0, 0, bytes.as_ptr(), len, core::ptr::null_mut(), 0);
        let mut wide = vec![0u16; n.max(0) as usize];
        MultiByteToWideChar(0, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n);
        wide
    };
    wide.push(0);
    wide
}
unsafe extern "system" fn create_file_a(p: *const u8, a: u32, s: u32, sa: *mut c_void, d: u32, fl: u32, t: H) -> H {
    if p.is_null() {
        return unsafe { CreateFileA(p, a, s, sa, d, fl, t) };
    }
    match map(narrow(p).as_ptr()) {
        Some(m) => unsafe { CreateFileW(m.as_ptr(), a, s, sa, d, fl, t) },
        None => unsafe { CreateFileA(p, a, s, sa, d, fl, t) },
    }
}
unsafe extern "system" fn delete_file_a(p: *const u8) -> i32 {
    if p.is_null() {
        return unsafe { DeleteFileA(p) };
    }
    match map(narrow(p).as_ptr()) {
        Some(m) => unsafe { DeleteFileW(m.as_ptr()) },
        None => unsafe { DeleteFileA(p) },
    }
}
unsafe extern "system" fn create_file_2(p: *const u16, a: u32, s: u32, d: u32, params: *const c_void) -> H {
    with(p, |p| unsafe { CreateFile2(p, a, s, d, params) })
}
unsafe extern "system" fn find_first_file_ex_w(p: *const u16, l: i32, d: *mut c_void, se: i32, f: *mut c_void, fl: u32) -> H {
    with(p, |p| unsafe { FindFirstFileExW(p, l, d, se, f, fl) })
}
unsafe extern "system" fn get_file_attributes_a(p: *const u8) -> u32 {
    if p.is_null() {
        return unsafe { GetFileAttributesA(p) };
    }
    let w = narrow(p);
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

// CRT file calls (ztrx victory-hle files.rs _wfopen, _wfopen_s, _wfsopen,
// fopen, fopen_s, _open, _wopen, _wstat64, _stat64i32, remove, _wchmod,
// _wutime64; GPL-3.0, credit to its authors): title drives are mapped as for
// the kernel calls; narrow paths go to the wide form. TUXBOX_LEADER_CRT_FILES=0 off.
unsafe extern "C" {
    fn _wfopen(p: *const u16, m: *const u16) -> H;
    fn _wfopen_s(f: *mut H, p: *const u16, m: *const u16) -> i32;
    fn _wfsopen(p: *const u16, m: *const u16, sh: i32) -> H;
    fn fopen(p: *const u8, m: *const u8) -> H;
    fn fopen_s(f: *mut H, p: *const u8, m: *const u8) -> i32;
    fn _open(p: *const u8, flag: i32, pmode: i32) -> i32;
    fn _wopen(p: *const u16, flag: i32, pmode: i32) -> i32;
    fn _wstat64(p: *const u16, b: *mut c_void) -> i32;
    fn _stat64i32(p: *const u8, b: *mut c_void) -> i32;
    fn _wstat64i32(p: *const u16, b: *mut c_void) -> i32;
    fn remove(p: *const u8) -> i32;
    fn _wremove(p: *const u16) -> i32;
    fn _wchmod(p: *const u16, m: i32) -> i32;
    fn _wutime64(p: *const u16, t: *mut c_void) -> i32;
}
/// A narrow title path mapped to a wide host path, or None to call the narrow form.
fn narrow_mapped(p: *const u8) -> Option<Vec<u16>> {
    if p.is_null() { None } else { map(narrow(p).as_ptr()) }
}
fn wide_mode(m: *const u8) -> Vec<u16> {
    if m.is_null() { return vec![0]; }
    unsafe { std::ffi::CStr::from_ptr(m.cast()) }.to_bytes().iter().map(|&b| u16::from(b)).chain([0]).collect()
}
unsafe extern "C" fn crt_wfopen(p: *const u16, m: *const u16) -> H { with(p, |p| unsafe { _wfopen(p, m) }) }
unsafe extern "C" fn crt_wfopen_s(f: *mut H, p: *const u16, m: *const u16) -> i32 { with(p, |p| unsafe { _wfopen_s(f, p, m) }) }
unsafe extern "C" fn crt_wfsopen(p: *const u16, m: *const u16, sh: i32) -> H { with(p, |p| unsafe { _wfsopen(p, m, sh) }) }
unsafe extern "C" fn crt_fopen(p: *const u8, m: *const u8) -> H {
    match narrow_mapped(p) { Some(w) => unsafe { _wfopen(w.as_ptr(), wide_mode(m).as_ptr()) }, None => unsafe { fopen(p, m) } }
}
unsafe extern "C" fn crt_fopen_s(f: *mut H, p: *const u8, m: *const u8) -> i32 {
    match narrow_mapped(p) { Some(w) => unsafe { _wfopen_s(f, w.as_ptr(), wide_mode(m).as_ptr()) }, None => unsafe { fopen_s(f, p, m) } }
}
unsafe extern "C" fn crt_open(p: *const u8, flag: i32, pmode: i32) -> i32 {
    match narrow_mapped(p) { Some(w) => unsafe { _wopen(w.as_ptr(), flag, pmode) }, None => unsafe { _open(p, flag, pmode) } }
}
unsafe extern "C" fn crt_wopen(p: *const u16, flag: i32, pmode: i32) -> i32 { with(p, |p| unsafe { _wopen(p, flag, pmode) }) }
unsafe extern "C" fn crt_wstat64(p: *const u16, b: *mut c_void) -> i32 { with(p, |p| unsafe { _wstat64(p, b) }) }
unsafe extern "C" fn crt_stat64i32(p: *const u8, b: *mut c_void) -> i32 {
    match narrow_mapped(p) { Some(w) => unsafe { _wstat64i32(w.as_ptr(), b) }, None => unsafe { _stat64i32(p, b) } }
}
unsafe extern "C" fn crt_remove(p: *const u8) -> i32 {
    match narrow_mapped(p) { Some(w) => unsafe { _wremove(w.as_ptr()) }, None => unsafe { remove(p) } }
}
unsafe extern "C" fn crt_wchmod(p: *const u16, m: i32) -> i32 { with(p, |p| unsafe { _wchmod(p, m) }) }
unsafe extern "C" fn crt_wutime64(p: *const u16, t: *mut c_void) -> i32 { with(p, |p| unsafe { _wutime64(p, t) }) }

pub(crate) unsafe fn install() {
    if std::env::var("TUXBOX_LEADER_CRT_FILES").map_or(true, |v| v.trim() != "0") {
        let n = unsafe {
            patch_imports(
                |d| d == "ucrtbase.dll" || d.starts_with("api-ms-win-crt-stdio-") || d.starts_with("api-ms-win-crt-filesystem-")
                    || d.starts_with("api-ms-win-crt-time-"),
                &[
                    ("_wfopen", crt_wfopen as *const () as usize),
                    ("_wfopen_s", crt_wfopen_s as *const () as usize),
                    ("_wfsopen", crt_wfsopen as *const () as usize),
                    ("fopen", crt_fopen as *const () as usize),
                    ("fopen_s", crt_fopen_s as *const () as usize),
                    ("_open", crt_open as *const () as usize),
                    ("_wopen", crt_wopen as *const () as usize),
                    ("_wstat64", crt_wstat64 as *const () as usize),
                    ("_stat64i32", crt_stat64i32 as *const () as usize),
                    ("remove", crt_remove as *const () as usize),
                    ("_wchmod", crt_wchmod as *const () as usize),
                    ("_wutime64", crt_wutime64 as *const () as usize),
                ],
            )
        };
        log(&format!("{n} CRT file import(s) patched (TUXBOX_LEADER_CRT_FILES=0 off)"));
    }
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
                ("ReadFile", crate::read_commit::read_file as *const () as usize),
            ]
            .into_iter()
            .chain(leader_files().then_some([
                ("CreateFileA", create_file_a as *const () as usize),
                ("DeleteFileA", delete_file_a as *const () as usize),
                ("CreateFile2", create_file_2 as *const () as usize),
                ("FindFirstFileExW", find_first_file_ex_w as *const () as usize),
            ]).into_iter().flatten())
            .collect::<Vec<_>>(),
        )
    };
    log(&format!("{n} file import(s) patched; G:\\ maps to the exe folder, D:\\ to a per-title folder"));
}
