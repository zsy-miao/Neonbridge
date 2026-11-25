//! Frame-rate overlay: a small topmost, click-through box in the game
//! window's top-left corner with fps, ms/frame and a frame-time graph, for
//! every runtime. It is fed from the log lines the launcher already reads:
//! the victory family's "12.3 fps, 81.3 ms/frame" and TuxBox's heartbeat
//! "completed PresentX #N (12.3 fps". F3 shows or hides it; off at start
//! unless NEONBRIDGE_FPS=1.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const BACKGROUND: COLORREF = rgb(17, 18, 20);
const BORDER: COLORREF = rgb(42, 43, 48);
const TEXT: COLORREF = rgb(232, 232, 234);
const SUB: COLORREF = rgb(139, 141, 148);
const CYAN: COLORREF = rgb(0, 229, 255);
pub const WIDTH: i32 = 200;
pub const HEIGHT: i32 = 92;
/// Samples the graph keeps (one per log line, about one a second).
const KEEP: usize = 60;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// (fps, ms/frame) samples, oldest first.
static SAMPLES: Mutex<Vec<(f64, f64)>> = Mutex::new(Vec::new());
static SHOWN: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);

/// Whether the box is on screen now (the controls panel moves below it).
/// The runtime draws its own fps overlay (toml own_overlay): ours starts hidden.
pub static OWN_OVERLAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn shown() -> bool {
    SHOWN.load(Ordering::Relaxed)
}

/// (fps, ms/frame) of a frame-rate or heartbeat line.
fn parse(line: &str) -> Option<(f64, f64)> {
    let num = |s: &str| s.rsplit([' ', '(']).next()?.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
    let fps = if let Some(at) = line.find(" fps, ") {
        num(&line[..at])?
    } else {
        let rest = line.split("completed PresentX #").nth(1)?;
        let at = rest.find(" fps")?;
        num(&rest[..at])?
    };
    let ms = line
        .split(" ms/frame")
        .next()
        .filter(|head| head.len() < line.len())
        .and_then(|head| head.rsplit([' ', ',']).next()?.parse::<f64>().ok())
        .unwrap_or(if fps > 0.0 { 1000.0 / fps } else { 0.0 });
    Some((fps, ms))
}

/// Takes one log line; frame-rate lines become samples.
pub fn feed(line: &str) {
    if let Some(s) = parse(line) {
        let mut v = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
        v.push(s);
        if v.len() > KEEP {
            v.remove(0);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Puts the box over the windows of the process named `name` (lowercase
/// file name) for the rest of the run. `window` finds the game's window.
pub fn show(name: String, window: fn(&str) -> Option<HWND>) {
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new().name("neonbridge fps".into()).spawn(move || run(name, window));
}

fn run(name: String, window: fn(&str) -> Option<HWND>) {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("Neonbridge.Fps");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
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
        SetLayeredWindowAttributes(hwnd, 0, 215, LWA_ALPHA);
        // A runtime with its own fps overlay (toml own_overlay) starts with
        // ours hidden, whatever NEONBRIDGE_FPS says; F3 still shows it.
        let own = OWN_OVERLAY.load(Ordering::Relaxed);
        let mut wanted = !own && std::env::var("NEONBRIDGE_FPS").is_ok_and(|v| v.trim() == "1");
        let mut f3 = false;
        let mut count = 0usize;
        let mut msg: MSG = std::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let game = window(&name);
            let focused = game.is_some_and(|g| GetForegroundWindow() == g);
            let f3_now = GetAsyncKeyState(0x72) as u16 & 0x8000 != 0;
            if focused && f3_now && !f3 {
                wanted = !wanted;
            }
            f3 = f3_now;
            let show = wanted && crate::splash::closed() && focused;
            if let (true, Some(g)) = (show, game) {
                let mut r: RECT = std::mem::zeroed();
                GetWindowRect(g, &mut r);
                // Top-right: runtimes draw their own overlays top-left (Flamingo's perf panel).
                let mut me: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut me);
                SetWindowPos(hwnd, HWND_TOPMOST, r.right - 24 - (me.right - me.left), r.top + 48, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                let n = SAMPLES.lock().map(|v| v.len()).unwrap_or(0);
                if n != count || !SHOWN.load(Ordering::Relaxed) {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    count = n;
                }
            } else if SHOWN.load(Ordering::Relaxed) {
                ShowWindow(hwnd, SW_HIDE);
            }
            SHOWN.store(show, Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg != WM_PAINT {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut r);
        let fill = |r: RECT, c: COLORREF| {
            let b = CreateSolidBrush(c);
            FillRect(dc, &r, b);
            DeleteObject(b as _);
        };
        fill(r, BACKGROUND);
        let frame = CreateSolidBrush(BORDER);
        FrameRect(dc, &r, frame);
        DeleteObject(frame as _);
        SetBkMode(dc, TRANSPARENT as _);
        let face = wide("Segoe UI Variable Text");
        let font = |size: i32, weight: i32| CreateFontW(size, 0, 0, 0, weight, 0, 0, 0, 0, 0, 0, CLEARTYPE_QUALITY as _, 0, face.as_ptr());
        let (big, small) = (font(24, 600), font(14, 400));
        let text = |s: &str, x: i32, y: i32, color: COLORREF, f: HFONT| {
            SelectObject(dc, f as _);
            SetTextColor(dc, color);
            let w: Vec<u16> = s.encode_utf16().collect();
            TextOutW(dc, x, y, w.as_ptr(), w.len() as i32);
        };
        let samples = SAMPLES.lock().map(|v| v.clone()).unwrap_or_default();
        match samples.last() {
            Some(&(fps, ms)) => {
                text(&format!("{fps:.1}"), 12, 8, TEXT, big);
                text("fps", 12 + 8 + 13 * format!("{fps:.1}").len() as i32, 16, SUB, small);
                text(&format!("{ms:.1} ms"), WIDTH - 72, 16, SUB, small);
            }
            None => text("Waiting for frames", 12, 14, SUB, small),
        }
        // Frame times, newest on the right; the scale is the window's worst.
        let graph = RECT { left: 12, top: 44, right: WIDTH - 12, bottom: HEIGHT - 12 };
        fill(graph, rgb(26, 27, 30));
        let worst = samples.iter().map(|s| s.1).fold(1.0f64, f64::max);
        let bar = (graph.right - graph.left) / KEEP as i32;
        let h = (graph.bottom - graph.top) as f64;
        for (i, &(_, ms)) in samples.iter().rev().enumerate() {
            let x = graph.right - (i as i32 + 1) * bar;
            let top = graph.bottom - ((ms / worst) * h).round().max(1.0) as i32;
            fill(RECT { left: x, top, right: x + bar.max(1) - (bar > 2) as i32, bottom: graph.bottom }, CYAN);
        }
        DeleteObject(big as _);
        DeleteObject(small as _);
        EndPaint(hwnd, &ps);
        0
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn victory_and_tuxbox_lines() {
        assert_eq!(parse("[victory] 12.5 fps, 80.0 ms/frame (gpu 3.1)"), Some((12.5, 80.0)));
        assert_eq!(parse("[tuxbox] heartbeat: completed PresentX #120 (25.0 fps)"), Some((25.0, 40.0)));
        assert_eq!(parse("completed PresentX #9 (2.5 fps; ms/frame: record 1, close 2)"), Some((2.5, 400.0)));
        assert_eq!(parse("nothing here"), None);
    }
}
