//! The console model answer.
//!
//! GDK code linked into the title computes XSystemGetDeviceType from
//! HKLM\Software\Microsoft\Windows NT\CurrentVersion\OEM, value ConsoleMode
//! (REG_DWORD); a missing value means a PC. Values: 0x64 Xbox Series X, 0x60
//! Series S. The exe's (and XCurl's) RegOpenKeyW / RegOpenKeyExW /
//! RegQueryValueExW / RegCloseKey imports answer that one key with a private
//! handle; every other key goes to the real registry.
//! TUXBOX_CONSOLE=x (default) | s | pc; pc leaves the registry alone.

use std::ffi::c_void;

use crate::loader::{log, patch_module_imports};

type Hkey = usize;

unsafe extern "system" {
    fn RegOpenKeyW(root: Hkey, path: *const u16, out: *mut Hkey) -> u32;
    fn RegOpenKeyExW(root: Hkey, path: *const u16, options: u32, sam: u32, out: *mut Hkey) -> u32;
    fn RegQueryValueExW(key: Hkey, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, size: *mut u32) -> u32;
    fn RegCloseKey(key: Hkey) -> u32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

const HKLM: Hkey = 0x8000_0002;
const OEM: &str = r"software\microsoft\windows nt\currentversion\oem";
/// The answered key's handle; never reaches the real registry.
const OEM_HANDLE: Hkey = 0x5442_4F45; // "TBOE"
const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_MORE_DATA: u32 = 234;
const REG_DWORD: u32 = 4;

static MODE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x64);

fn wide(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    Some(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) }))
}

fn is_oem(root: Hkey, path: *const u16) -> bool {
    // HKEY values are sign-extended 32-bit constants.
    (root as u32 as usize) == HKLM && wide(path).is_some_and(|p| p.trim_matches('\\').eq_ignore_ascii_case(OEM))
}

unsafe extern "system" fn reg_open_key_w(root: Hkey, path: *const u16, out: *mut Hkey) -> u32 {
    if is_oem(root, path) && !out.is_null() {
        unsafe { out.write(OEM_HANDLE) };
        return ERROR_SUCCESS;
    }
    unsafe { RegOpenKeyW(root, path, out) }
}

unsafe extern "system" fn reg_open_key_ex_w(root: Hkey, path: *const u16, options: u32, sam: u32, out: *mut Hkey) -> u32 {
    if is_oem(root, path) && !out.is_null() {
        unsafe { out.write(OEM_HANDLE) };
        return ERROR_SUCCESS;
    }
    unsafe { RegOpenKeyExW(root, path, options, sam, out) }
}

unsafe extern "system" fn reg_query_value_ex_w(key: Hkey, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, size: *mut u32) -> u32 {
    if key != OEM_HANDLE {
        return unsafe { RegQueryValueExW(key, name, reserved, kind, data, size) };
    }
    if !wide(name).is_some_and(|n| n.eq_ignore_ascii_case("ConsoleMode")) {
        return ERROR_FILE_NOT_FOUND;
    }
    let mode = MODE.load(std::sync::atomic::Ordering::Relaxed);
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| log(&format!("console mode: answered ConsoleMode = {mode:#x} (TUXBOX_CONSOLE=x|s|pc)")));
    unsafe {
        if !kind.is_null() {
            kind.write(REG_DWORD);
        }
        if size.is_null() {
            return if data.is_null() { ERROR_SUCCESS } else { 87 }; // ERROR_INVALID_PARAMETER
        }
        let room = size.read();
        size.write(4);
        if data.is_null() {
            return ERROR_SUCCESS;
        }
        if room < 4 {
            return ERROR_MORE_DATA;
        }
        data.cast::<u32>().write_unaligned(mode);
    }
    ERROR_SUCCESS
}

unsafe extern "system" fn reg_close_key(key: Hkey) -> u32 {
    if key == OEM_HANDLE {
        return ERROR_SUCCESS;
    }
    unsafe { RegCloseKey(key) }
}

pub(crate) unsafe fn install() {
    let mode = match std::env::var("TUXBOX_CONSOLE").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "pc" => {
            log("console mode: TUXBOX_CONSOLE=pc, the registry answers as on a PC");
            return;
        }
        "s" | "series s" | "lockhart" => 0x60,
        _ => 0x64,
    };
    MODE.store(mode, std::sync::atomic::Ordering::Relaxed);
    let hooks: &[(&str, usize)] = &[
        ("RegOpenKeyW", reg_open_key_w as *const () as usize),
        ("RegOpenKeyExW", reg_open_key_ex_w as *const () as usize),
        ("RegQueryValueExW", reg_query_value_ex_w as *const () as usize),
        ("RegCloseKey", reg_close_key as *const () as usize),
    ];
    let registry = |d: &str| d == "advapi32.dll" || d.contains("registry") || d == "kernelbase.dll";
    let mut total = 0;
    for name in ["", "XCurl.dll"] {
        let module = if name.is_empty() {
            unsafe { GetModuleHandleW(std::ptr::null()) }
        } else {
            let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
            unsafe { GetModuleHandleW(w.as_ptr()) }
        };
        if !module.is_null() {
            total += unsafe { patch_module_imports(module, registry, hooks, &[]) };
        }
    }
    log(&format!("console mode: {total} registry import(s) patched; ConsoleMode = {mode:#x}"));
}
