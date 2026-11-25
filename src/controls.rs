//! Controls overlay: once the game's window is up and the loading screen is
//! gone, a topmost, click-through panel over its corner shows which key or
//! mouse button plays which gamepad button, for the runtime that runs it. The
//! map is each runtime's own: the [controls] table of its NEONBRIDGE.toml,
//! read from the leader's input code; tuxbox's is [`TUXBOX`].
//!
//! F1 shows or hides it, F2 hides it: keys no runtime's [controls] map uses
//! (Esc is the victory family's Menu and xcl's B). Both are read without
//! taking them from the game. NEONBRIDGE_CONTROLS=0: no overlay.

use std::sync::Mutex;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Pad buttons in the order the panel lists them: (toml key, label).
pub const BUTTONS: [(&str, &str); 15] = [
    ("left_stick", "Left stick"),
    ("right_stick", "Right stick"),
    ("a", "A"),
    ("b", "B"),
    ("x", "X"),
    ("y", "Y"),
    ("lb", "LB"),
    ("rb", "RB"),
    ("lt", "LT"),
    ("rt", "RT"),
    ("ls", "L-stick click"),
    ("rs", "R-stick click"),
    ("dpad", "D-pad"),
    ("view", "View"),
    ("menu", "Menu"),
];

/// TuxBox's keyboard and mouse pad (vendor/neonbridge-own/runtime/gameinput/src/
/// sample.rs, DesktopPad::merge: "MNZXCVIKJLQE" on Menu, View, A, B, X, Y,
/// D-pad up, down, left, right, LB, RB; Enter and the left mouse button also
/// A, Esc also B; R or the right mouse button LT, F or right+left mouse RT;
/// W A S D the left stick, the arrows the right, the right mouse button held
/// turns the mouse into the right stick). It has no stick clicks.
pub const TUXBOX: [(&str, &str); 15] = [
    ("left_stick", "W A S D"),
    ("right_stick", "Arrows, or hold right mouse and move"),
    ("a", "Z / Enter / Left mouse"),
    ("b", "X / Esc"),
    ("x", "C"),
    ("y", "V"),
    ("lb", "Q"),
    ("rb", "E"),
    ("lt", "R / Right mouse"),
    ("rt", "F / Right + left mouse"),
    ("ls", "-"),
    ("rs", "-"),
    ("dpad", "I K J L"),
    ("view", "N"),
    ("menu", "M"),
];

const BACKGROUND: COLORREF = rgb(11, 11, 20);
const PINK: COLORREF = rgb(255, 46, 151);
const CYAN: COLORREF = rgb(0, 229, 255);
const TEXT: COLORREF = rgb(200, 200, 220);
const DIM: COLORREF = rgb(130, 130, 160);
const ROW: i32 = 22;
const WIDTH: i32 = 400;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

struct Panel {
    title: String,
    rows: Vec<(String, String)>,
}

static PANEL: Mutex<Option<Panel>> = Mutex::new(None);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Shows the overlay over the windows of process `exe` for the rest of the
/// run, with `rows` (pad label, keys) under the title `runtime`.
pub fn show(runtime: &str, exe: &std::path::Path, rows: Vec<(String, String)>) {
    if let Some(name) = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()) {
        crate::fps::show(name, game_window);
    }
    if std::env::var("NEONBRIDGE_CONTROLS").is_ok_and(|v| v.trim() == "0") || rows.is_empty() {
        return;
    }
    let Some(name) = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()) else { return };
    *PANEL.lock().unwrap_or_else(|e| e.into_inner()) = Some(Panel { title: format!("Controls: {runtime}"), rows });
    let _ = std::thread::Builder::new().name("neonbridge controls".into()).spawn(move || run(name));
}

/// `--controls-preview <runtime>`: the overlay over a plain 1280x720 window
/// of our own, without a game (for checking a map, and the README's shot).
/// Closing the window ends it.
pub fn preview(runtime: &str, rows: Vec<(String, String)>) {
    unsafe extern "system" fn plain(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_PAINT => {
                    let mut ps: PAINTSTRUCT = std::mem::zeroed();
                    let dc = BeginPaint(hwnd, &mut ps);
                    let mut r: RECT = std::mem::zeroed();
                    GetClientRect(hwnd, &mut r);
                    let bg = CreateSolidBrush(0x0018_1010);
                    FillRect(dc, &r, bg);
                    DeleteObject(bg as _);
                    EndPaint(hwnd, &ps);
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
    let Ok(exe) = std::env::current_exe() else { return };
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("Neonbridge.ControlsPreview");
        let wc = WNDCLASSW { lpfnWndProc: Some(plain), hInstance: instance, lpszClassName: class.as_ptr(), hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let title = wide(&format!("Controls preview: {runtime}"));
        let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
        let mut r = RECT { left: 0, top: 0, right: 1280, bottom: 720 };
        AdjustWindowRect(&mut r, style, 0);
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style, 80, 80, r.right - r.left, r.bottom - r.top, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null());
        if hwnd.is_null() {
            return;
        }
        SetForegroundWindow(hwnd);
        // Made-up frame times so the fps box has something to draw.
        std::thread::spawn(|| {
            for i in 0u32.. {
                let fps = 28.0 + 3.0 * ((i as f64) * 0.7).sin() - if i % 9 == 0 { 9.0 } else { 0.0 };
                crate::fps::feed(&format!("[preview] {fps:.1} fps, {:.1} ms/frame", 1000.0 / fps));
                std::thread::sleep(std::time::Duration::from_millis(if i < 60 { 10 } else { 1000 }));
            }
        });
        show(runtime, &exe, rows);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The rows of a NEONBRIDGE.toml [controls] table, in [`BUTTONS`] order.
pub fn rows(get: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    BUTTONS.iter().filter_map(|(k, label)| get(k).map(|v| ((*label).to_owned(), v))).collect()
}

pub fn tuxbox_rows() -> Vec<(String, String)> {
    rows(|k| TUXBOX.iter().find(|(t, _)| *t == k).map(|(_, v)| (*v).to_owned()))
}

fn height(rows: usize) -> i32 {
    44 + rows as i32 * ROW + 30
}

fn run(name: String) {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("Neonbridge.Controls");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let rows = PANEL.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |p| p.rows.len());
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            WIDTH,
            height(rows),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        SetLayeredWindowAttributes(hwnd, 0, 225, LWA_ALPHA);
        let mut wanted = true;
        let (mut f1, mut f2) = (false, false);
        let mut shown = false;
        let mut msg: MSG = std::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let game = game_window(&name);
            let focused = game.is_some_and(|g| GetForegroundWindow() == g);
            let (f1_now, f2_now) = (GetAsyncKeyState(0x70) as u16 & 0x8000 != 0, GetAsyncKeyState(0x71) as u16 & 0x8000 != 0);
            if focused && f1_now && !f1 {
                wanted = !wanted;
            }
            if focused && f2_now && !f2 && shown {
                wanted = false;
            }
            (f1, f2) = (f1_now, f2_now);
            let show = wanted && crate::splash::closed() && focused;
            if let (true, Some(g)) = (show, game) {
                let mut r: RECT = std::mem::zeroed();
                GetWindowRect(g, &mut r);
                // Below the fps box when that is up.
                let below = if crate::fps::shown() { crate::fps::HEIGHT + 8 } else { 0 };
                SetWindowPos(hwnd, HWND_TOPMOST, r.left + 24, r.top + 48 + below, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            } else if shown {
                ShowWindow(hwnd, SW_HIDE);
            }
            shown = show;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// The largest visible, unowned top-level window of the process named `name`.
pub fn game_window(name: &str) -> Option<HWND> {
    unsafe extern "system" fn visit(hwnd: HWND, out: LPARAM) -> BOOL {
        unsafe { (*(out as *mut Vec<HWND>)).push(hwnd) };
        1
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(visit), &mut all as *mut Vec<HWND> as LPARAM) };
    all.into_iter()
        .filter(|&h| unsafe { IsWindowVisible(h) != 0 && GetWindow(h, GW_OWNER).is_null() })
        .filter(|&h| {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(h, &mut pid) };
            image_name(pid).is_some_and(|n| n == name)
        })
        .max_by_key(|&h| {
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetWindowRect(h, &mut r) };
            (r.right - r.left) as i64 * (r.bottom - r.top) as i64
        })
}

fn image_name(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buffer[..len as usize]).rsplit('\\').next().unwrap_or_default().to_lowercase())
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
        let bg = CreateSolidBrush(BACKGROUND);
        FillRect(dc, &r, bg);
        DeleteObject(bg as _);
        // A pink frame, like the loading screen.
        let frame = CreateSolidBrush(PINK);
        FrameRect(dc, &r, frame);
        DeleteObject(frame as _);
        SetBkMode(dc, TRANSPARENT as _);
        let face = wide("Segoe UI");
        let font = |size: i32, weight: i32| CreateFontW(size, 0, 0, 0, weight, 0, 0, 0, 0, 0, 0, CLEARTYPE_QUALITY as _, 0, face.as_ptr());
        let (big, small) = (font(20, 700), font(16, 400));
        let text = |s: &str, x: i32, y: i32, color: COLORREF, f: HFONT| {
            SelectObject(dc, f as _);
            SetTextColor(dc, color);
            let w: Vec<u16> = s.encode_utf16().collect();
            TextOutW(dc, x, y, w.as_ptr(), w.len() as i32);
        };
        if let Some(p) = PANEL.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            text(&p.title, 14, 10, CYAN, big);
            let caps = CreateSolidBrush(CYAN);
            for (i, (pad, keys)) in p.rows.iter().enumerate() {
                let y = 44 + i as i32 * ROW;
                text(pad, 14, y, PINK, small);
                // Each key as a cap: a cyan outline around its name.
                let mut x = 140;
                for (j, key) in keys.split(" / ").enumerate() {
                    if j > 0 {
                        text("or", x, y, DIM, small);
                        x += 20;
                    }
                    let w: Vec<u16> = key.encode_utf16().collect();
                    let mut size = std::mem::zeroed();
                    SelectObject(dc, small as _);
                    GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
                    let cap = RECT { left: x, top: y - 1, right: x + size.cx + 10, bottom: y + size.cy + 1 };
                    FrameRect(dc, &cap, caps);
                    text(key, x + 5, y, TEXT, small);
                    x = cap.right + 6;
                }
            }
            DeleteObject(caps as _);
            text("F1 show / hide   F2 hide   F3 fps", 14, 44 + p.rows.len() as i32 * ROW + 6, DIM, small);
        }
        DeleteObject(big as _);
        DeleteObject(small as _);
        EndPaint(hwnd, &ps);
        0
    }
}
