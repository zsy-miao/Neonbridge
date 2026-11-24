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
//!
//! Borderless windowed fullscreen (opt-in, TUXBOX_FULLSCREEN=1): the title's main window (an
//! explicit size of at least 1280x720) is created as a popup covering the whole
//! monitor it asked for, and d3d12 letterboxes the image to keep its aspect.
//! The default is the 16:9 window inside the work area. Alt+Enter or F11 toggles between the two at run time
//! (TUXBOX_ALT_ENTER=0 leaves both keys to the title).
//!
//! TUXBOX_MONITOR=primary|nearest|N picks the monitor (default: the primary
//! for a default position, else the one nearest the requested position; N is
//! the 0-based enumeration index). The process is made per-monitor DPI aware
//! (v2) before any window exists so rcMonitor is in real pixels
//! (TUXBOX_DPI_AWARE=0 skips it; rebox 49e3f2a). TUXBOX_CLIP_CURSOR=1 confines
//! the cursor to the client area while the window is active.

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
    fn MonitorFromPoint(pt: Point, flags: u32) -> H;
    fn MonitorFromWindow(hwnd: H, flags: u32) -> H;
    fn GetMonitorInfoW(monitor: H, info: *mut MonitorInfo) -> i32;
    fn GetWindowLongPtrW(hwnd: H, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: H, index: i32, value: isize) -> isize;
    fn SetWindowPos(hwnd: H, after: H, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn EnumDisplayMonitors(dc: H, clip: *const Rect, callback: unsafe extern "system" fn(H, H, *mut Rect, isize) -> i32, data: isize) -> i32;
    fn GetClientRect(hwnd: H, rect: *mut Rect) -> i32;
    fn ClientToScreen(hwnd: H, point: *mut Point) -> i32;
    fn ClipCursor(rect: *const Rect) -> i32;
    fn ShowWindow(hwnd: H, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: H) -> i32;
}

const SW_SHOW: i32 = 5;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_SHOWWINDOW: u32 = 0x0040;

/// The monitor TUXBOX_MONITOR names, if it names one: `primary`, or a 0-based
/// index into EnumDisplayMonitors. `nearest` (or unset) returns None.
unsafe fn chosen_monitor() -> Option<H> {
    let choice = std::env::var("TUXBOX_MONITOR").ok()?;
    let choice = choice.trim().to_ascii_lowercase();
    if choice == "primary" {
        return Some(unsafe { MonitorFromPoint(Point { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) });
    }
    let index: usize = choice.parse().ok()?;
    unsafe extern "system" fn collect(monitor: H, _: H, _: *mut Rect, data: isize) -> i32 {
        unsafe { (*(data as *mut Vec<usize>)).push(monitor as usize) };
        1
    }
    let mut all: Vec<usize> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), collect, &mut all as *mut Vec<usize> as isize) };
    let found = all.get(index).map(|&m| m as H);
    if found.is_none() {
        log(&format!("window: TUXBOX_MONITOR={index} names no monitor ({} found); using the default", all.len()));
    }
    found
}

/// The monitor a top-level window at (x, y) goes on.
unsafe fn target_monitor(x: i32, y: i32) -> H {
    unsafe {
        chosen_monitor().unwrap_or_else(|| {
            if x == i32::MIN {
                MonitorFromPoint(Point { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
            } else {
                MonitorFromPoint(Point { x, y }, MONITOR_DEFAULTTONEAREST)
            }
        })
    }
}

/// Per-monitor DPI awareness v2, set before the title creates a window.
unsafe fn make_dpi_aware() {
    if std::env::var("TUXBOX_DPI_AWARE").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let user32: Vec<u16> = "user32.dll\0".encode_utf16().collect();
    unsafe {
        let module = LoadLibraryW(user32.as_ptr());
        let set = GetProcAddress(module, c"SetProcessDpiAwarenessContext".as_ptr().cast());
        if module.is_null() || set.is_null() {
            return;
        }
        let set: unsafe extern "system" fn(isize) -> i32 = std::mem::transmute(set);
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2; fails harmlessly when the
        // manifest or the title already chose a mode.
        let ok = set(-4) != 0;
        log(&format!("window: per-monitor DPI awareness v2 {} (TUXBOX_DPI_AWARE=0 skips)", if ok { "set" } else { "already chosen" }));
    }
}

const WM_ACTIVATE: u32 = 0x0006;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_ENTERSIZEMOVE: u32 = 0x0231;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const WM_SIZE: u32 = 0x0005;

/// TUXBOX_CLIP_CURSOR=1: confine the cursor to the client area while active;
/// released on deactivation, focus loss and while moving or sizing.
unsafe fn clip_cursor(hwnd: H, msg: u32, w: usize) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("TUXBOX_CLIP_CURSOR").is_ok_and(|v| v.trim() == "1")) {
        return;
    }
    unsafe {
        match msg {
            WM_ACTIVATE if w & 0xFFFF != 0 => {}
            WM_EXITSIZEMOVE | WM_SIZE => {}
            WM_ACTIVATE | WM_KILLFOCUS | WM_ENTERSIZEMOVE => {
                ClipCursor(std::ptr::null());
                return;
            }
            _ => return,
        }
        let mut client = Rect::default();
        let mut origin = Point { x: 0, y: 0 };
        if GetClientRect(hwnd, &mut client) != 0 && ClientToScreen(hwnd, &mut origin) != 0 && client.right > 0 {
            let screen = Rect { left: origin.x, top: origin.y, right: origin.x + client.right, bottom: origin.y + client.bottom };
            ClipCursor(&screen);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}

const MONITOR_DEFAULTTONEAREST: u32 = 2;
const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const WS_POPUP: u32 = 0x8000_0000;
/// Caption, frame, system menu, minimize and maximize boxes (WS_OVERLAPPEDWINDOW).
const WS_FRAME_BITS: u32 = 0x00CF_0000;
/// WS_EX_DLGMODALFRAME | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE.
const WS_EX_FRAME_BITS: u32 = 0x0002_0301;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;
const WM_SYSKEYDOWN: u32 = 0x0104;
const VK_RETURN: usize = 0x0D;

/// Borderless windowed fullscreen at creation: opt-in with TUXBOX_FULLSCREEN=1
/// or NEONBRIDGE_WINDOWED=0 while the default popup showed no picture (game
/// audible, window not seen). Alt+Enter/F11 still toggle at run time.
fn fullscreen_default() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("TUXBOX_FULLSCREEN").is_ok_and(|v| v.trim() == "1")
            || std::env::var("NEONBRIDGE_WINDOWED").is_ok_and(|v| v.trim() == "0")
    })
}

unsafe fn monitor_rects(monitor: H) -> Option<(Rect, Rect)> {
    let mut info = MonitorInfo { size: size_of::<MonitorInfo>() as u32, ..Default::default() };
    (!monitor.is_null() && unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some((info.monitor, info.work))
}

/// The largest 16:9 window with `style` that fits `work`, centred.
unsafe fn windowed_bounds(work: Rect, style: u32, ex: u32) -> (i32, i32, i32, i32) {
    let (aw, ah) = (work.right - work.left, work.bottom - work.top);
    let mut frame = Rect::default();
    unsafe { AdjustWindowRectEx(&mut frame, style, 0, ex) };
    let (bw, bh) = (frame.right - frame.left, frame.bottom - frame.top);
    let client_w = i64::from(aw - bw).min(i64::from(ah - bh) * 16 / 9).max(16);
    let client_h = client_w * 9 / 16;
    let (ow, oh) = (client_w as i32 + bw, client_h as i32 + bh);
    (work.left + (aw - ow) / 2, work.top + (ah - oh) / 2, ow, oh)
}

/// Alt+Enter: switch the window between borderless fullscreen on its monitor
/// and a 16:9 framed window in that monitor's work area.
unsafe fn toggle_fullscreen(hwnd: H) {
    unsafe {
        let Some((monitor, work)) = monitor_rects(chosen_monitor().unwrap_or_else(|| MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST))) else { return };
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if style & WS_POPUP == 0 {
            let style = (style & !WS_FRAME_BITS) | WS_POPUP;
            SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_FRAME_BITS) as isize);
            SetWindowPos(hwnd, std::ptr::null_mut(), monitor.left, monitor.top, monitor.right - monitor.left,
                monitor.bottom - monitor.top, SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            log("window: Alt+Enter -> borderless fullscreen");
        } else {
            let style = (style & !WS_POPUP) | WS_FRAME_BITS;
            SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
            let (x, y, w, h) = windowed_bounds(work, style, ex);
            SetWindowPos(hwnd, std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            log("window: Alt+Enter -> windowed 16:9");
        }
    }
}

const WM_KEYDOWN: u32 = 0x0100;
const WM_SYSCHAR: u32 = 0x0106;
const VK_F11: usize = 0x7A;

fn toggle_keys_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_ALT_ENTER").map_or(true, |v| v.trim() != "0"))
}

/// Alt+Enter (WM_SYSKEYDOWN, VK_RETURN, context bit 29) or F11 (WM_KEYDOWN),
/// not a repeat. TUXBOX_ALT_ENTER=0 leaves both keys to the title.
fn alt_enter(msg: u32, w: usize, l: isize) -> bool {
    let fresh = (l >> 30) & 1 == 0;
    let alt_return = msg == WM_SYSKEYDOWN && w == VK_RETURN && (l >> 29) & 1 != 0;
    let f11 = msg == WM_KEYDOWN && w == VK_F11;
    (alt_return || f11) && fresh && toggle_keys_enabled()
}

/// The WM_SYSCHAR that follows Alt+Enter would beep in DefWindowProc.
fn alt_enter_char(msg: u32, w: usize) -> bool {
    msg == WM_SYSCHAR && w == 0x0D && toggle_keys_enabled()
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
    if alt_enter(msg, w, l) {
        unsafe { toggle_fullscreen(hwnd) };
        return 0;
    }
    if alt_enter_char(msg, w) {
        return 0;
    }
    unsafe { clip_cursor(hwnd, msg, w) };
    if drop_key_menu(msg, w) {
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| log("window: dropped the Alt/F10 key menu (SC_KEYMENU; TUXBOX_KEY_MENU=1 allows it)"));
        return 0;
    }
    unsafe { DefWindowProcA(hwnd, msg, w, l) }
}

unsafe extern "system" fn def_window_proc_w(hwnd: H, msg: u32, w: usize, l: isize) -> isize {
    if alt_enter(msg, w, l) {
        unsafe { toggle_fullscreen(hwnd) };
        return 0;
    }
    if alt_enter_char(msg, w) {
        return 0;
    }
    unsafe { clip_cursor(hwnd, msg, w) };
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
                // Bring it to the front: launched from another window, a
                // monitor-sized popup can stay behind it, heard but not seen.
                ShowWindow(hwnd as H, SW_SHOW);
                SetWindowPos(hwnd as H, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
                let front = SetForegroundWindow(hwnd as H) != 0;
                log(&format!("window: uncloaked {hwnd:#x} (brought to the front: {front})"));
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

/// The window's size and position fitted 16:9 into the target monitor's work
/// area (TUXBOX_MONITOR, else the primary for a default position or the one
/// nearest the requested position), when the requested outer size does not fit.
unsafe fn fitted(ex: u32, style: u32, menu: bool, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let mut work = Rect::default();
    unsafe {
        if let Some((_, area)) = monitor_rects(target_monitor(x, y)) {
            work = area;
        } else if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut Rect).cast(), 0) == 0 {
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
    // The main window: an explicit size of at least 1280x720.
    let main = top && w != i32::MIN && h != i32::MIN && w >= 1280 && h >= 720;
    let mut fullscreen = None;
    if main && fullscreen_default() {
        fullscreen = unsafe { monitor_rects(target_monitor(x, y)) }.map(|(monitor, _)| monitor);
    }
    let (ex, style) = if fullscreen.is_some() { (ex & !WS_EX_FRAME_BITS, (style & !WS_FRAME_BITS) | WS_POPUP) } else { (ex, style) };
    let (fx, fy, fw, fh) = match fullscreen {
        Some(m) => (m.left, m.top, m.right - m.left, m.bottom - m.top),
        None if top => unsafe { fitted(ex, style, !menu.is_null(), x, y, w, h) },
        None => (x, y, w, h),
    };
    if fullscreen.is_some() {
        log(&format!("window: created borderless fullscreen at {fw}x{fh}+{fx}+{fy} instead of {w}x{h}+{x}+{y} (TUXBOX_FULLSCREEN=1; Alt+Enter toggles)"));
    } else if (fx, fy, fw, fh) != (x, y, w, h) {
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
    unsafe { make_dpi_aware() };
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
    log(&format!("window: {n} window import(s) patched (black class brush, borderless fullscreen or one-monitor fit, cloak until the first present, no Alt/F10 key menu, Alt+Enter/F11 toggle)"));
}
