//! Loading screen shown from launch until the game presents its first frame.
//!
//! A borderless, topmost window on its own thread with a status line and a
//! progress bar. The launcher moves it through its stages with [`status`];
//! [`close`] hides it when the title's first PresentX completes (or it exits).
//! NEONBRIDGE_SPLASH=0 turns it off.

use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WIDTH: i32 = 560;
const HEIGHT: i32 = 210;
const BACKGROUND: COLORREF = rgb(11, 11, 20);
const TRACK: COLORREF = rgb(32, 30, 48);
const PINK: COLORREF = rgb(255, 46, 151);
const CYAN: COLORREF = rgb(0, 229, 255);
const TEXT: COLORREF = rgb(200, 200, 220);

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

struct State {
    /// Stage target, 0..1; the bar eases towards it and creeps while waiting.
    target: f32,
    shown: f32,
    text: String,
    tick: u32,
}

static STATE: Mutex<State> = Mutex::new(State { target: 0.0, shown: 0.0, text: String::new(), tick: 0 });
static WINDOW: AtomicIsize = AtomicIsize::new(0);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Opens the loading screen (once; no-op when disabled).
pub fn show(text: &str) {
    if std::env::var("NEONBRIDGE_SPLASH").as_deref() == Ok("0") || WINDOW.load(Ordering::Acquire) != 0 {
        return;
    }
    state().text = text.to_owned();
    WINDOW.store(-1, Ordering::Release);
    let _ = std::thread::Builder::new().name("neonbridge splash".into()).spawn(run);
}

/// Moves the bar to `progress` (0..1) with a new status line.
pub fn status(progress: f32, text: &str) {
    let mut s = state();
    s.target = s.target.max(progress.clamp(0.0, 1.0));
    s.text = text.to_owned();
}

/// Closes the loading screen.
pub fn close() {
    let hwnd = WINDOW.swap(-2, Ordering::AcqRel);
    if hwnd > 0 {
        unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn run() {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("Neonbridge.Splash");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            hInstance: instance,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_APPSTARTING),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let title = wide("Neonbridge");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            (sw - WIDTH) / 2,
            (sh - HEIGHT) / 2,
            WIDTH,
            HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        // close() may have run before the window existed.
        if WINDOW.compare_exchange(-1, hwnd as isize, Ordering::AcqRel, Ordering::Acquire).is_err() {
            DestroyWindow(hwnd);
            return;
        }
        SetTimer(hwnd, 1, 33, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                {
                    let mut s = state();
                    s.tick = s.tick.wrapping_add(1);
                    // Ease towards the stage target; creep slowly past it so a long
                    // stage (shader prewarm, first frame) still shows movement.
                    let ceiling = (s.target + 0.12).min(0.97);
                    s.shown += if s.shown < s.target { (s.target - s.shown) * 0.15 } else { (ceiling - s.shown).max(0.0) * 0.002 };
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_ERASEBKGND => 1,
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

unsafe fn fill(dc: HDC, r: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &r, brush);
        DeleteObject(brush);
    }
}

unsafe fn font(height: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    unsafe { CreateFontW(height, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, face.as_ptr()) }
}

unsafe fn text(dc: HDC, s: &str, mut r: RECT, color: COLORREF, f: HFONT, flags: u32) {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        let old = SelectObject(dc, f);
        SetTextColor(dc, color);
        DrawTextW(dc, w.as_mut_ptr(), w.len() as i32, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

unsafe fn paint(hwnd: HWND) {
    let (shown, line, tick) = {
        let s = state();
        (s.shown, s.text.clone(), s.tick)
    };
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let screen = BeginPaint(hwnd, &mut ps);
        // Double-buffered to avoid flicker at 30 Hz.
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, WIDTH, HEIGHT);
        let old_bitmap = SelectObject(dc, bitmap);
        SetBkMode(dc, TRANSPARENT as i32);

        fill(dc, RECT { left: 0, top: 0, right: WIDTH, bottom: HEIGHT }, BACKGROUND);
        // Neon frame: pink top edge, cyan bottom edge.
        fill(dc, RECT { left: 0, top: 0, right: WIDTH, bottom: 3 }, PINK);
        fill(dc, RECT { left: 0, top: HEIGHT - 3, right: WIDTH, bottom: HEIGHT }, CYAN);

        let title = font(44, 800);
        let small = font(17, 400);
        text(dc, "NEON", RECT { left: 32, top: 28, right: WIDTH / 2, bottom: 84 }, PINK, title, DT_LEFT | DT_VCENTER);
        let mut size: windows_sys::Win32::Foundation::SIZE = std::mem::zeroed();
        let neon = wide("NEON");
        let old = SelectObject(dc, title);
        GetTextExtentPoint32W(dc, neon.as_ptr(), 4, &mut size);
        SelectObject(dc, old);
        text(dc, "BRIDGE", RECT { left: 32 + size.cx, top: 28, right: WIDTH - 32, bottom: 84 }, CYAN, title, DT_LEFT | DT_VCENTER);
        let dots = ".".repeat((tick / 12 % 4) as usize);
        text(dc, &format!("{line}{dots}"), RECT { left: 32, top: 104, right: WIDTH - 32, bottom: 130 }, TEXT, small, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);

        // Progress bar with a moving highlight.
        let bar = RECT { left: 32, top: 146, right: WIDTH - 32, bottom: 156 };
        fill(dc, bar, TRACK);
        let filled = bar.left + ((bar.right - bar.left) as f32 * shown.clamp(0.0, 1.0)) as i32;
        fill(dc, RECT { right: filled, ..bar }, PINK);
        let span = (filled - bar.left).max(1);
        let glint = bar.left + (tick as i32 * 6) % (span + 60) - 60;
        fill(dc, RECT { left: glint.max(bar.left), right: (glint + 60).min(filled), ..bar }, CYAN);
        text(dc, &format!("{:.0}%", shown * 100.0), RECT { left: 32, top: 162, right: WIDTH - 32, bottom: 186 }, TEXT, small, DT_RIGHT | DT_VCENTER);

        BitBlt(screen, 0, 0, WIDTH, HEIGHT, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old_bitmap);
        DeleteObject(bitmap);
        DeleteDC(dc);
        DeleteObject(title);
        DeleteObject(small);
        EndPaint(hwnd, &ps);
    }
}
