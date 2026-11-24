//! The title's first window: no white flash, no spanning both monitors.
//!
//! The title registers its class with no usable background and creates a
//! 3840x2160 top-level window, which shows white across every monitor until
//! the swap chain's first frame. The exe's RegisterClassExA and CreateWindowExA
//! imports are patched so that:
//! - the class background is the black stock brush;
//! - a top-level window larger than the primary monitor's work area is created
//!   16:9 inside it instead;
//! - the window is cloaked (DWM keeps it off screen) from creation until d3d12
//!   uncloaks it at its first present (`TuxBoxUncloakWindows`), or after 30 s.
//! TUXBOX_CLOAK_WINDOW=0 turns the cloak off; the class brush and the fit stay
//! (the launcher's curtain remains a second guard).

use std::ffi::c_void;
use std::sync::Mutex;

use crate::loader::{log, patch_imports};

type H = *mut c_void;

unsafe extern "system" {
    fn CreateWindowExA(ex: u32, class: *const u8, name: *const u8, style: u32, x: i32, y: i32, w: i32, h: i32, parent: H, menu: H, inst: H, param: *mut c_void) -> H;
    fn RegisterClassExA(class: *const c_void) -> u16;
    fn GetStockObject(index: i32) -> H;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut c_void, ini: u32) -> i32;
    fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, ex: u32) -> i32;
    fn LoadLibraryW(name: *const u16) -> H;
    fn GetProcAddress(module: H, name: *const u8) -> *const c_void;
    fn IsWindow(hwnd: H) -> i32;
    fn DefWindowProcA(hwnd: H, msg: u32, w: usize, l: isize) -> isize;
    fn DefWindowProcW(hwnd: H, msg: u32, w: usize, l: isize) -> isize;
}

const WM_SYSCOMMAND: u32 = 0x0112;
const SC_KEYMENU: usize = 0xF100;

/// Alt or F10 sends WM_SYSCOMMAND SC_KEYMENU, and DefWindowProc then runs a
/// modal menu loop on the title's window thread: the title stops presenting
/// and reading input, and its watchdog aborts it about 30 s later. The key
/// menu is dropped; the mouse-opened window menu (SC_MOUSEMENU) still works.
/// TUXBOX_KEY_MENU=1 lets it through (old behaviour). From VIctory ff8694a.
fn drop_key_menu(msg: u32, w: usize) -> bool {
    static ALLOW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    msg == WM_SYSCOMMAND
        && w & 0xFFF0 == SC_KEYMENU
        && !*ALLOW.get_or_init(|| std::env::var("TUXBOX_KEY_MENU").is_ok_and(|v| v.trim() == "1"))
}

unsafe extern "system" fn def_window_proc_a(hwnd: H, msg: u32, w: usize, l: isize) -> isize {
    if drop_key_menu(msg, w) {
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| log("window: dropped the Alt/F10 key menu (SC_KEYMENU; TUXBOX_KEY_MENU=1 allows it)"));
        return 0;
    }
    unsafe { DefWindowProcA(hwnd, msg, w, l) }
}

unsafe extern "system" fn def_window_proc_w(hwnd: H, msg: u32, w: usize, l: isize) -> isize {
    if drop_key_menu(msg, w) {
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

const WS_CHILD: u32 = 0x4000_0000;
const BLACK_BRUSH: i32 = 4;
const SPI_GETWORKAREA: u32 = 0x30;
const DWMWA_CLOAK: u32 = 13;
/// WNDCLASSEXA.hbrBackground: cbSize, style, lpfnWndProc, cbClsExtra, cbWndExtra, hInstance, hIcon, hCursor.
const BRUSH_OFFSET: usize = 48;
const WNDCLASSEXA_SIZE: usize = 80;

static CLOAKED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn cloak_enabled() -> bool {
    std::env::var("TUXBOX_CLOAK_WINDOW").map_or(true, |v| v.trim() != "0")
}

unsafe fn cloak(hwnd: H, on: bool) -> bool {
    type Set = unsafe extern "system" fn(H, u32, *const c_void, u32) -> i32;
    static SET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let f = *SET.get_or_init(|| unsafe {
        let name: Vec<u16> = "dwmapi.dll".encode_utf16().chain([0]).collect();
        let dwm = LoadLibraryW(name.as_ptr());
        if dwm.is_null() { 0 } else { GetProcAddress(dwm, b"DwmSetWindowAttribute\0".as_ptr()) as usize }
    });
    if f == 0 {
        return false;
    }
    let set: Set = unsafe { std::mem::transmute(f) };
    let value: i32 = on.into();
    unsafe { set(hwnd, DWMWA_CLOAK, (&value as *const i32).cast(), 4) >= 0 }
}

/// Uncloak every window this module cloaked. d3d12 calls it at the first present.
#[unsafe(export_name = "TuxBoxUncloakWindows")]
pub extern "system" fn uncloak_windows() {
    let windows = std::mem::take(&mut *CLOAKED.lock().unwrap_or_else(|e| e.into_inner()));
    for hwnd in windows {
        unsafe {
            if IsWindow(hwnd as H) != 0 && cloak(hwnd as H, false) {
                log(&format!("window: uncloaked {hwnd:#x}"));
            }
        }
    }
}

unsafe extern "system" fn register_class_ex_a(class: *const c_void) -> u16 {
    if class.is_null() {
        return unsafe { RegisterClassExA(class) };
    }
    let mut copy = [0u8; WNDCLASSEXA_SIZE];
    unsafe {
        std::ptr::copy_nonoverlapping(class.cast::<u8>(), copy.as_mut_ptr(), WNDCLASSEXA_SIZE);
        let brush = copy.as_mut_ptr().add(BRUSH_OFFSET).cast::<usize>();
        if brush.read_unaligned() == 0 || brush.read_unaligned() <= 31 {
            // No brush or a system colour index (COLOR_WINDOW + 1 is white).
            brush.write_unaligned(GetStockObject(BLACK_BRUSH) as usize);
        }
        RegisterClassExA(copy.as_ptr().cast())
    }
}

/// The window's size and position fitted 16:9 into the primary work area, when
/// the requested outer size does not fit there.
unsafe fn fitted(ex: u32, style: u32, menu: bool, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let mut work = Rect::default();
    unsafe {
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut Rect).cast(), 0) == 0 {
            return (x, y, w, h);
        }
    }
    let (aw, ah) = (work.right - work.left, work.bottom - work.top);
    // CW_USEDEFAULT (0x80000000) leaves the size to Windows; only fit an explicit one.
    if w == i32::MIN || h == i32::MIN || (w <= aw && h <= ah) {
        return (x, y, w, h);
    }
    let mut frame = Rect::default();
    unsafe { AdjustWindowRectEx(&mut frame, style, menu.into(), ex) };
    let (bw, bh) = (frame.right - frame.left, frame.bottom - frame.top);
    let client_w = i64::from(aw - bw).min(i64::from(ah - bh) * 16 / 9).max(16);
    let client_h = client_w * 9 / 16;
    let (ow, oh) = (client_w as i32 + bw, client_h as i32 + bh);
    (work.left + (aw - ow) / 2, work.top + (ah - oh) / 2, ow, oh)
}

unsafe extern "system" fn create_window_ex_a(ex: u32, class: *const u8, name: *const u8, style: u32, x: i32, y: i32, w: i32, h: i32, parent: H, menu: H, inst: H, param: *mut c_void) -> H {
    let top = parent.is_null() && style & WS_CHILD == 0;
    let (fx, fy, fw, fh) = if top { unsafe { fitted(ex, style, !menu.is_null(), x, y, w, h) } } else { (x, y, w, h) };
    if (fx, fy, fw, fh) != (x, y, w, h) {
        log(&format!("window: created at {fw}x{fh}+{fx}+{fy} instead of {w}x{h}+{x}+{y} (one monitor, 16:9)"));
    }
    let hwnd = unsafe { CreateWindowExA(ex, class, name, style, fx, fy, fw, fh, parent, menu, inst, param) };
    if top && !hwnd.is_null() && cloak_enabled() && unsafe { cloak(hwnd, true) } {
        CLOAKED.lock().unwrap_or_else(|e| e.into_inner()).push(hwnd as usize);
        log(&format!("window: cloaked {:#x} until the first present (TUXBOX_CLOAK_WINDOW=0 turns this off)", hwnd as usize));
        // Never leave the title invisible: uncloak after 30 s whatever happens.
        let _ = std::thread::Builder::new().name("tuxbox uncloak".into()).spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(30));
            uncloak_windows();
        });
    }
    hwnd
}

pub(crate) unsafe fn install() {
    let n = unsafe {
        patch_imports(
            |d| d.contains("ntuser-window") || d == "user32.dll",
            &[
                ("CreateWindowExA", create_window_ex_a as *const () as usize),
                ("RegisterClassExA", register_class_ex_a as *const () as usize),
                ("DefWindowProcA", def_window_proc_a as *const () as usize),
                ("DefWindowProcW", def_window_proc_w as *const () as usize),
            ],
        )
    };
    log(&format!("window: {n} window import(s) patched (black class brush, one-monitor fit, cloak until the first present, no Alt/F10 key menu)"));
}
