//! Flamingo: an on-screen display of the frame rate and what a frame costs,
//! in the manner of RivaTuner's: a click-through window the game's window
//! owns (so it stays over the game's alone, goes when the game's does, and
//! is never taken for the process's main window), at the game's top left,
//! drawn with GDI ten times a second on a thread of its own (nothing of it
//! runs on the threads that present). F9 shows or hides
//! it while the game's window is in front; `FLAMINGO_OSD=1` shows it from
//! the start, `FLAMINGO_OSD=0` leaves it out altogether.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, BitBlt, CLIP_DEFAULT_PRECIS, ClientToScreen, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DeleteDC, DeleteObject,
    EndPaint, FillRect, HDC, HFONT, InvalidateRect, LineTo, MoveToEx, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetForegroundWindow, GetMessageW,
    IsIconic, LWA_ALPHA, MSG, RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, WM_PAINT, WM_TIMER, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

/// What the last second cost, per frame, as the frame-rate line has it.
#[derive(Clone, Copy, Default)]
pub(crate) struct Second {
    pub fps: f64,
    pub ms: f64,
    pub draws: f64,
    /// Preparing submissions, waiting for the GPU, copying back.
    pub cpu: f64,
    pub gpu: f64,
    pub back: f64,
    /// Shaders from the cache, translated in this run, that do not translate.
    pub shaders: [u64; 3],
    /// Pipelines made, being made, without a host pipeline.
    pub pipelines: [u64; 3],
    /// Draws and dispatches dropped per frame (no host pipeline).
    pub dropped: f64,
}

static SECOND: Mutex<Option<Second>> = Mutex::new(None);
static FRAMES: AtomicU64 = AtomicU64::new(0);
/// The game's window.
static GAME: AtomicUsize = AtomicUsize::new(0);
static SHOWN: AtomicBool = AtomicBool::new(false);

/// Frame times for the graph (ms), a ring, and when the last frame came.
const GRAPH: usize = 120;
struct Times {
    ms: [f32; GRAPH],
    next: usize,
    last: Option<std::time::Instant>,
}
static TIMES: Mutex<Times> = Mutex::new(Times { ms: [0.0; GRAPH], next: 0, last: None });

const W: i32 = 330;
const H: i32 = 246;
const KEY_F9: i32 = 0x78;

/// A presented frame of the game's window `hwnd`.
pub(crate) fn frame(hwnd: HWND) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| {
        let v = std::env::var("FLAMINGO_OSD").unwrap_or_default();
        SHOWN.store(v == "1", Ordering::Relaxed);
        v != "0"
    });
    if !on {
        return;
    }
    FRAMES.fetch_add(1, Ordering::Relaxed);
    {
        let mut t = TIMES.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        if let Some(last) = t.last {
            let i = t.next;
            t.ms[i] = (now - last).as_secs_f32() * 1000.0;
            t.next = (i + 1) % GRAPH;
        }
        t.last = Some(now);
    }
    if GAME.swap(hwnd.0 as usize, Ordering::Relaxed) == 0 {
        let _ = std::thread::Builder::new().name("flamingo-osd".into()).spawn(run);
    }
}

/// The last second's numbers.
pub(crate) fn second(s: Second) {
    *SECOND.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
}

fn run() {
    // SAFETY: plain window creation and a message loop on this thread.
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("FlamingoOsd"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("FlamingoOsd"),
            w!("Flamingo OSD"),
            WS_POPUP,
            0,
            0,
            W,
            H,
            Some(HWND(GAME.load(Ordering::Relaxed) as *mut _)),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 220, LWA_ALPHA);
        SetTimer(Some(hwnd), 1, 100, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            tick(hwnd);
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: our window, painted in its own WM_PAINT.
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                paint(hdc);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        // SAFETY: the default handling of our own window's messages.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Ten times a second: F9, and where the window goes or whether it hides.
fn tick(hwnd: HWND) {
    static HELD: AtomicBool = AtomicBool::new(false);
    let game = HWND(GAME.load(Ordering::Relaxed) as *mut _);
    // SAFETY: plain queries of windows and the keyboard.
    unsafe {
        let front = GetForegroundWindow() == game;
        let down = GetAsyncKeyState(KEY_F9) as u16 & 0x8000 != 0;
        if front && down && !HELD.swap(true, Ordering::Relaxed) {
            SHOWN.fetch_xor(true, Ordering::Relaxed);
        } else if !down {
            HELD.store(false, Ordering::Relaxed);
        }
        let mut client = RECT::default();
        let show = SHOWN.load(Ordering::Relaxed)
            && front
            && !IsIconic(game).as_bool()
            && GetClientRect(game, &mut client).is_ok();
        if !show {
            let _ = ShowWindow(hwnd, SW_HIDE);
            return;
        }
        let mut at = POINT { x: 12, y: 12 };
        let _ = ClientToScreen(game, &mut at);
        let _ = SetWindowPos(hwnd, None, at.x, at.y, W, H, SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Process memory not shared with others (bytes).
fn private_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: a zeroed counters structure of the size passed.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut c).cast(), c.cb) == 0 {
            return 0;
        }
        c.PrivateUsage as u64
    }
}

/// The card's memory in use and the budget (bytes), asked once a second.
fn video_memory() -> Option<(u64, u64)> {
    static LAST: Mutex<(Option<std::time::Instant>, Option<(u64, u64)>)> = Mutex::new((None, None));
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.0.is_none_or(|at| at.elapsed().as_secs() >= 1) {
        last.1 = crate::host::host()
            .and_then(|h| h.video_memory())
            .map(|[budget, usage, _, _]| (usage, budget));
        last.0 = Some(std::time::Instant::now());
    }
    last.1
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// The colour of a frame rate: red under 15, amber under 30, green above.
fn rate_colour(fps: f64) -> COLORREF {
    if fps < 15.0 {
        rgb(255, 96, 96)
    } else if fps < 30.0 {
        rgb(255, 200, 80)
    } else {
        rgb(120, 230, 120)
    }
}

fn text(hdc: HDC, x: i32, y: i32, colour: COLORREF, s: &str) {
    let s: Vec<u16> = s.encode_utf16().collect();
    // SAFETY: a device context of ours and a text slice.
    unsafe {
        SetTextColor(hdc, colour);
        let _ = TextOutW(hdc, x, y, &s);
    }
}

fn paint(target: HDC) {
    static FONT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let s = SECOND.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_default();
    let (frames, times, next) = {
        let t = TIMES.lock().unwrap_or_else(|e| e.into_inner());
        (FRAMES.load(Ordering::Relaxed), t.ms, t.next)
    };
    // SAFETY: GDI drawing into a bitmap of ours, then onto our window.
    unsafe {
        let font = HFONT(*FONT.get_or_init(|| {
            CreateFontW(
                -15,
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                w!("Consolas"),
            )
            .0 as usize
        }) as *mut _);
        let hdc = CreateCompatibleDC(Some(target));
        let bitmap = CreateCompatibleBitmap(target, W, H);
        let old_bitmap = SelectObject(hdc, bitmap.into());
        let old_font = SelectObject(hdc, font.into());
        let back = CreateSolidBrush(rgb(14, 14, 20));
        FillRect(hdc, &RECT { left: 0, top: 0, right: W, bottom: H }, back);
        let _ = DeleteObject(back.into());
        SetBkMode(hdc, TRANSPARENT);
        let (dim, white, pink) = (rgb(150, 150, 165), rgb(235, 235, 240), rgb(255, 120, 170));
        let row = 18;
        let mut y = 6;
        text(hdc, 10, y, pink, "FLAMINGO");
        text(hdc, 250, y, dim, "F9 hide");
        y += row + 2;
        text(hdc, 10, y, dim, "FPS");
        text(hdc, 60, y, rate_colour(s.fps), &format!("{:5.1}", s.fps));
        text(hdc, 150, y, white, &format!("{:6.1} ms", s.ms));
        y += row;
        text(hdc, 10, y, dim, "Frames");
        text(hdc, 80, y, white, &frames.to_string());
        text(hdc, 170, y, dim, "Draws");
        text(hdc, 230, y, white, &format!("{:.0}", s.draws));
        y += row;
        text(hdc, 10, y, dim, "CPU");
        text(hdc, 45, y, white, &format!("{:4.1}", s.cpu));
        text(hdc, 110, y, dim, "GPU");
        text(hdc, 145, y, white, &format!("{:4.1}", s.gpu));
        text(hdc, 210, y, dim, "Back");
        text(hdc, 255, y, white, &format!("{:4.1}", s.back));
        y += row;
        let vram = video_memory()
            .map(|(used, budget)| format!("{:.1}/{:.1} GB", used as f64 / 1e9, budget as f64 / 1e9))
            .unwrap_or_else(|| "-".into());
        text(hdc, 10, y, dim, "VRAM");
        text(hdc, 55, y, white, &vram);
        text(hdc, 190, y, dim, "RAM");
        text(hdc, 230, y, white, &format!("{:.1} GB", private_bytes() as f64 / 1e9));
        y += row;
        let red = rgb(255, 90, 90);
        let [cached, translated, failed] = s.shaders;
        text(hdc, 10, y, dim, "Shaders");
        text(hdc, 85, y, white, &format!("{cached} + {translated} new"));
        text(hdc, 255, y, if failed > 0 { red } else { dim }, &format!("{failed} fail"));
        y += row;
        let [made, making, unmade] = s.pipelines;
        text(hdc, 10, y, dim, "Pipelines");
        text(hdc, 100, y, white, &format!("{made} ok {making} wait"));
        text(hdc, 255, y, if unmade > 0 { red } else { dim }, &format!("{unmade} fail"));
        y += row;
        text(hdc, 10, y, dim, "Dropped");
        text(hdc, 85, y, if s.dropped >= 1.0 { red } else { white }, &format!("{:.0} draws/frame", s.dropped));
        y += row + 6;
        // The frame-time graph, newest on the right; lines at 66.7 ms (15
        // fps) and 33.3 ms (30 fps).
        let (gx, gw, gh) = (10, W - 20, H - y - 8);
        let top = times.iter().copied().fold(100.0_f32, f32::max);
        let at = |ms: f32| y + gh - ((ms / top).min(1.0) * gh as f32) as i32;
        let guide = CreatePen(PS_SOLID, 1, rgb(60, 60, 75));
        let old_pen = SelectObject(hdc, guide.into());
        for ms in [66.7, 33.3] {
            let _ = MoveToEx(hdc, gx, at(ms), None);
            let _ = LineTo(hdc, gx + gw, at(ms));
        }
        let line = CreatePen(PS_SOLID, 2, rate_colour(s.fps));
        SelectObject(hdc, line.into());
        for k in 0..GRAPH {
            let ms = times[(next + k) % GRAPH];
            let x = gx + (k as i32 * gw) / (GRAPH as i32 - 1);
            if k == 0 {
                let _ = MoveToEx(hdc, x, at(ms), None);
            } else {
                let _ = LineTo(hdc, x, at(ms));
            }
        }
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(guide.into());
        let _ = DeleteObject(line.into());
        let _ = BitBlt(target, 0, 0, W, H, Some(hdc), 0, 0, SRCCOPY);
        SelectObject(hdc, old_font);
        SelectObject(hdc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(hdc);
    }
}

#[cfg(test)]
mod tests {
    use super::{GRAPH, H, Second, TIMES, W, paint, second};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    };

    /// The display draws its text and graph: many pixels differ from the
    /// background. `FLAMINGO_OSD_SAVE=<file.bmp>` keeps the picture.
    #[test]
    fn draws_text_and_graph() {
        second(Second {
            fps: 11.2,
            ms: 89.3,
            draws: 4554.0,
            cpu: 57.8,
            gpu: 21.5,
            back: 30.5,
            shaders: [25890, 12, 0],
            pipelines: [2512, 3, 1],
            dropped: 269.0,
        });
        {
            let mut t = TIMES.lock().unwrap();
            for k in 0..GRAPH {
                t.ms[k] = 80.0 + (k % 7) as f32 * 4.0;
            }
        }
        // SAFETY: GDI objects of this test, released below.
        let pixels = unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, W, H);
            let old = SelectObject(dc, bitmap.into());
            paint(dc);
            SelectObject(dc, old);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: W,
                    biHeight: -H,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u32; (W * H) as usize];
            let rows = GetDIBits(dc, bitmap, 0, H as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
            assert_eq!(rows, H);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            pixels
        };
        // The background is rgb(14, 14, 20): BGRX 0x0014_0e0e.
        let drawn = pixels.iter().filter(|&&p| p & 0x00ff_ffff != 0x0014_0e0e).count();
        assert!(drawn > 2000, "only {drawn} pixels drawn");
        if let Ok(path) = std::env::var("FLAMINGO_OSD_SAVE") {
            let mut bmp = Vec::new();
            let size = 54 + pixels.len() * 4;
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&(size as u32).to_le_bytes());
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&W.to_le_bytes());
            bmp.extend_from_slice(&(-H).to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0u8; 24]);
            for p in &pixels {
                bmp.extend_from_slice(&p.to_le_bytes());
            }
            std::fs::write(path, bmp).unwrap();
        }
    }
}
