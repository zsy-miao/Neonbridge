//! 6ixpack: app-local console libraries win over System32 copies.
//!
//! On Windows with Gaming Services installed, the title's GDK loader stub
//! loads `xgameruntime.dll` from System32 (the real Gaming Services runtime),
//! which fails outside a packaged Xbox title (0x80070424) and the title stops
//! on a fatal `int 3`. xmem is a static import of the exe, so its DllMain runs
//! after the exe's imports are bound: patch the exe's LoadLibrary* IAT slots so
//! a console library that sits beside the exe is loaded from there. Every
//! LoadLibrary* call is logged (TUXBOX_LOG=stderr|1) to show what the title loads.

use std::{ffi::c_void, ptr, sync::OnceLock};

type Hmod = *mut c_void;

unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hmod;
    fn GetModuleFileNameW(module: Hmod, buf: *mut u16, len: u32) -> u32;
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> Hmod;
    fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    fn OutputDebugStringA(s: *const u8);
    fn GetLastError() -> u32;
}

/// Console libraries the title may load at run time; System32 must not win for these.
const CONSOLE: &[&str] = &["xgameruntime.dll", "xgameruntime", "d3d12_xs.dll", "xg_xs.dll", "xmem.dll", "xapu.dll", "chad.dll",
    "acphal.dll", "gameinput.dll", "xcrdapi.dll", "xboxbvhbuilder.dll", "dxcompiler_xs.dll"];

static DIR: OnceLock<Vec<u16>> = OnceLock::new();
static LOG: OnceLock<u8> = OnceLock::new(); // 0 off, 1 debug string, 2 stderr

pub(crate) fn log(msg: &str) {
    match *LOG.get_or_init(|| match std::env::var("TUXBOX_LOG").as_deref() {
        Ok("stderr") => 2,
        Ok("1") => 1,
        _ => 0,
    }) {
        2 => eprintln!("TuxBox xmem: {msg}"),
        1 => {
            let s = format!("TuxBox xmem: {msg}\n\0");
            unsafe { OutputDebugStringA(s.as_ptr()) };
        }
        _ => {}
    }
}

fn wide_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

fn ansi_to_string(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned()
}

/// Full path beside the exe when `name` is a console library present there.
fn local_override(name: &str) -> Option<Vec<u16>> {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name).to_ascii_lowercase();
    if !CONSOLE.contains(&base.as_str()) {
        return None;
    }
    let file = if base.ends_with(".dll") { base } else { format!("{base}.dll") };
    let dir = DIR.get()?;
    let mut path: Vec<u16> = dir.clone();
    path.extend(file.encode_utf16());
    let s = String::from_utf16_lossy(&path);
    std::path::Path::new(&s).is_file().then(|| {
        path.push(0);
        path
    })
}

thread_local! {
    /// Set while a late-load patch runs, so loads it causes are not patched again.
    static PATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A module the title loaded after start: when it is the title's own (it sits
/// in the exe's folder, not a console library), block its network imports and
/// answer ConsoleMode in its registry imports, as for the exe. Reentrant loads
/// (from the patch itself, or nested LoadLibrary calls) are skipped.
/// TUXBOX_LATE_PATCH=0 leaves late modules alone.
fn late_patch(module: Hmod, name: &str) {
    static OFF: OnceLock<bool> = OnceLock::new();
    if module.is_null() || *OFF.get_or_init(|| std::env::var("TUXBOX_LATE_PATCH").is_ok_and(|v| v.trim() == "0")) {
        return;
    }
    if PATCHING.with(|p| p.replace(true)) {
        return;
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = &buf[..n];
    let in_exe_dir = DIR.get().is_some_and(|dir| {
        path.len() > dir.len()
            && String::from_utf16_lossy(&path[..dir.len()]).eq_ignore_ascii_case(&String::from_utf16_lossy(dir))
            && !path[dir.len()..].contains(&(b'\\' as u16))
    });
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name).to_ascii_lowercase();
    if in_exe_dir && !CONSOLE.contains(&base.as_str()) {
        let n = unsafe { crate::net::patch_module(module, name) + crate::console_mode::patch_module(module)
            + crate::xaudio::patch_module(module.cast(), name) + crate::affinity::patch_module(module.cast()) };
        if n > 0 {
            log(&format!("late load {name}: {n} import(s) patched (TUXBOX_LATE_PATCH=0 skips this)"));
        }
    }
    PATCHING.with(|p| p.set(false));
}

fn load(name: String, flags: u32, original: impl FnOnce() -> Hmod) -> Hmod {
    let h = load_inner(&name, flags, original);
    late_patch(h, &name);
    h
}

fn load_inner(name: &str, flags: u32, original: impl FnOnce() -> Hmod) -> Hmod {
    let name = name.to_owned();
    if let Some(path) = local_override(&name) {
        // Drop search-path flags (e.g. LOAD_LIBRARY_SEARCH_SYSTEM32): the path is absolute.
        let h = unsafe { LoadLibraryExW(path.as_ptr(), ptr::null_mut(), flags & !0x0000_7F00) };
        log(&format!("LoadLibrary {name} (flags {flags:#x}) -> redirected to {} = {h:?}", String::from_utf16_lossy(&path[..path.len() - 1])));
        return h;
    }
    let h = original();
    let err = if h.is_null() { unsafe { GetLastError() } } else { 0 };
    log(&format!("LoadLibrary {name} (flags {flags:#x}) = {h:?}{}", if h.is_null() { format!(" error {err}") } else { String::new() }));
    h
}

unsafe extern "system" fn hook_ex_w(name: *const u16, file: *mut c_void, flags: u32) -> Hmod {
    load(wide_to_string(name), flags, || unsafe { LoadLibraryExW(name, file, flags) })
}
unsafe extern "system" fn hook_w(name: *const u16) -> Hmod {
    load(wide_to_string(name), 0, || unsafe { LoadLibraryExW(name, ptr::null_mut(), 0) })
}
unsafe extern "system" fn hook_ex_a(name: *const u8, file: *mut c_void, flags: u32) -> Hmod {
    let s = ansi_to_string(name);
    let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
    load(s, flags, || unsafe { LoadLibraryExW(w.as_ptr(), file, flags) })
}
unsafe extern "system" fn hook_a(name: *const u8) -> Hmod {
    let s = ansi_to_string(name);
    let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
    load(s, 0, || unsafe { LoadLibraryExW(w.as_ptr(), ptr::null_mut(), 0) })
}

/// The exe's directory with a trailing separator (set by `install`).
pub(crate) fn exe_dir() -> Option<&'static [u16]> {
    DIR.get().map(|v| v.as_slice())
}

/// Patch the exe's LoadLibrary* imports, then its file imports (`G:` drive).
/// Called from xmem's DllMain (process attach).
pub(crate) unsafe fn install() {
    unsafe {
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) as usize;
        let full = &buf[..n];
        let cut = full.iter().rposition(|&c| c == b'\\' as u16 || c == b'/' as u16).map(|i| i + 1).unwrap_or(0);
        let _ = DIR.set(full[..cut].to_vec());
        let patched = patch_imports(
            |d| d.contains("libraryloader") || d == "kernel32.dll" || d == "kernelbase.dll",
            &[
                ("LoadLibraryExW", hook_ex_w as *const () as usize),
                ("LoadLibraryW", hook_w as *const () as usize),
                ("LoadLibraryExA", hook_ex_a as *const () as usize),
                ("LoadLibraryA", hook_a as *const () as usize),
            ],
        );
        log(&format!("{patched} LoadLibrary import(s) redirected; console libraries load from {}", String::from_utf16_lossy(&full[..cut])));
        crate::paths::install();
        crate::xaudio::install();
        crate::affinity::install();
        crate::write_watch::install();
        crate::net::install();
        crate::window::install();
        crate::console_mode::install();
    }
}

/// A module's file path into `buf`; returns its length.
pub(crate) unsafe fn module_file_name(module: *mut core::ffi::c_void, buf: &mut [u16]) -> usize {
    unsafe { GetModuleFileNameW(module.cast(), buf.as_mut_ptr(), buf.len() as u32) as usize }
}

/// Point the exe's imports named in `hooks` at the replacements, for every
/// imported DLL that `dll` accepts (lower-case name). Returns the slots patched.
pub(crate) unsafe fn patch_imports(dll: impl Fn(&str) -> bool, hooks: &[(&str, usize)]) -> usize {
    unsafe { patch_module_imports(GetModuleHandleW(ptr::null()), dll, hooks, &[]) }
}

/// [`patch_imports`] for any loaded `module`; `ordinals` hooks imports made by
/// ordinal (ws2_32 is mostly imported that way).
pub(crate) unsafe fn patch_module_imports(module: Hmod, dll: impl Fn(&str) -> bool, hooks: &[(&str, usize)], ordinals: &[(u16, usize)]) -> usize {
    unsafe {
        let exe = module as *mut u8;
        if exe.is_null() {
            return 0;
        }
        let rd32 = |o: usize| ptr::read_unaligned(exe.add(o) as *const u32) as usize;
        let pe = rd32(0x3c);
        let imports = rd32(pe + 0x18 + 0x70 + 8); // PE32+ optional header, data directory 1 (import)
        if imports == 0 {
            return 0;
        }
        let mut patched = 0;
        let mut desc = imports;
        loop {
            let (int_rva, name_rva, iat_rva) = (rd32(desc), rd32(desc + 12), rd32(desc + 16));
            if name_rva == 0 {
                break;
            }
            let name = ansi_to_string(exe.add(name_rva)).to_ascii_lowercase();
            if dll(&name) {
                let lookup = if int_rva != 0 { int_rva } else { iat_rva };
                let mut i = 0;
                loop {
                    let entry = ptr::read_unaligned(exe.add(lookup + i * 8) as *const u64);
                    if entry == 0 {
                        break;
                    }
                    let found = if entry >> 63 == 0 {
                        let func = ansi_to_string(exe.add(entry as usize + 2));
                        hooks.iter().find(|(n, _)| *n == func).map(|&(_, h)| (func, h))
                    } else {
                        let ordinal = entry as u16;
                        ordinals.iter().find(|(o, _)| *o == ordinal).map(|&(_, h)| (format!("#{ordinal}"), h))
                    };
                    {
                        if let Some((func, h)) = found {
                            let slot = exe.add(iat_rva + i * 8) as *mut usize;
                            let mut old = 0u32;
                            if VirtualProtect(slot.cast(), 8, 0x04, &mut old) != 0 {
                                ptr::write_volatile(slot, h);
                                VirtualProtect(slot.cast(), 8, old, &mut old);
                                patched += 1;
                                log(&format!("hooked {name}!{func}"));
                            }
                        }
                    }
                    i += 1;
                }
            }
            desc += 20;
        }
        patched
    }
}
