//! Flamingo: an on-screen display of the frame rate and what a frame costs,
//! in the manner of RivaTuner's: a click-through window the game's window
//! owns (so it stays over the game's alone, goes when the game's does, and
//! is never taken for the process's main window), at the game's top left,
//! drawn with GDI ten times a second on a thread of its own (nothing of it
//! runs on the threads that present). F9, while the game's window is in
//! front, goes round three states: the compact display (frame rate and frame
//! time, the card's load and temperature, video memory, the processor's
//! load, memory and the frame-time graph, as MSI Afterburner shows them),
//! the full one (also what a frame costs: preparing submissions, copies,
//! textures, waiting for the card, copying back, shaders, pipelines,
//! dropped draws), and nothing. `FLAMINGO_OSD=1` starts with the compact
//! display, `FLAMINGO_OSD=2` with the full one, `FLAMINGO_OSD=0` leaves it
//! out altogether.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, BitBlt, CLIP_DEFAULT_PRECIS, ClientToScreen, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DeleteDC, DeleteObject,
    EndPaint, FillRect, HDC, HFONT, InvalidateRect, LineTo, MoveToEx, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetForegroundWindow, GetMessageW,
    IsIconic, LWA_ALPHA, MSG, RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, WM_PAINT, WM_TIMER, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{s, w};

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
    /// Of preparing: title buffer copies and texture refreshes, ms and MB.
    pub copies: [f64; 2],
    pub textures: [f64; 2],
    /// MB copied back per frame.
    pub back_mb: f64,
}

static SECOND: Mutex<Option<Second>> = Mutex::new(None);
static FRAMES: AtomicU64 = AtomicU64::new(0);
/// The game's window.
static GAME: AtomicUsize = AtomicUsize::new(0);
/// What is shown: 0 nothing, 1 the compact display, 2 the full one.
static MODE: AtomicU8 = AtomicU8::new(0);

/// Frame times for the graph (ms), a ring, and when the last frame came.
const GRAPH: usize = 120;
struct Times {
    ms: [f32; GRAPH],
    next: usize,
    last: Option<std::time::Instant>,
}
static TIMES: Mutex<Times> = Mutex::new(Times { ms: [0.0; GRAPH], next: 0, last: None });

const W: i32 = 330;
/// The full display's height, and the compact one's.
const H: i32 = 300;
const H_COMPACT: i32 = 124;
const KEY_F9: i32 = 0x78;

fn height(mode: u8) -> i32 {
    if mode == 1 { H_COMPACT } else { H }
}

/// A presented frame of the game's window `hwnd`.
pub(crate) fn frame(hwnd: HWND) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| {
        let v = std::env::var("FLAMINGO_OSD").unwrap_or_default();
        MODE.store(
            match v.as_str() {
                "1" => 1,
                "2" => 2,
                _ => 0,
            },
            Ordering::Relaxed,
        );
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
            // Compact, full, nothing, and round again.
            let _ = MODE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |m| Some((m + 1) % 3));
        } else if !down {
            HELD.store(false, Ordering::Relaxed);
        }
        let mode = MODE.load(Ordering::Relaxed);
        let mut client = RECT::default();
        let show = mode != 0 && front && !IsIconic(game).as_bool() && GetClientRect(game, &mut client).is_ok();
        if !show {
            let _ = ShowWindow(hwnd, SW_HIDE);
            return;
        }
        let mut at = POINT { x: 12, y: 12 };
        let _ = ClientToScreen(game, &mut at);
        let _ = SetWindowPos(hwnd, None, at.x, at.y, W, height(mode), SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW);
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

/// The processor's load over the last second (all cores, percent), from
/// the system's idle, kernel and user times.
fn cpu_load() -> Option<f64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    type Sample = (std::time::Instant, [u64; 3]);
    static LAST: Mutex<(Option<Sample>, Option<f64>)> = Mutex::new((None, None));
    let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.0.is_some_and(|(at, _)| at.elapsed().as_secs() < 1) {
        return last.1;
    }
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: three FILETIMEs of ours for the system to fill.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let now = [ticks(idle), ticks(kernel), ticks(user)];
    if let Some((_, before)) = last.0 {
        // The kernel's time includes the idle time.
        let busy = (now[1] - before[1]) + (now[2] - before[2]);
        let idle = now[0] - before[0];
        if busy > 0 {
            last.1 = Some(100.0 * busy.saturating_sub(idle) as f64 / busy as f64);
        }
    }
    last.0 = Some((std::time::Instant::now(), now));
    last.1
}

/// The card's load (percent) and temperature (degrees C), from NVIDIA's
/// management library (`nvml.dll`, which NVIDIA's driver installs), asked
/// once a second; none on other cards.
fn gpu_stats() -> (Option<u32>, Option<u32>) {
    type Init = unsafe extern "C" fn() -> i32;
    type Handle = unsafe extern "C" fn(u32, *mut usize) -> i32;
    type Load = unsafe extern "C" fn(usize, *mut [u32; 2]) -> i32;
    type Temperature = unsafe extern "C" fn(usize, u32, *mut u32) -> i32;
    struct Nvml {
        device: usize,
        load: Load,
        temperature: Temperature,
    }
    static NVML: std::sync::OnceLock<Option<Nvml>> = std::sync::OnceLock::new();
    static LAST: Mutex<(Option<std::time::Instant>, (Option<u32>, Option<u32>))> = Mutex::new((None, (None, None)));
    let Some(nvml) = NVML.get_or_init(|| {
        // SAFETY: the library's documented entry points, with their C
        // signatures; a failed lookup or call leaves the numbers out.
        unsafe {
            let lib = LoadLibraryW(w!("nvml.dll")).ok()?;
            let init: Init = std::mem::transmute(GetProcAddress(lib, s!("nvmlInit_v2"))?);
            let handle: Handle = std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetHandleByIndex_v2"))?);
            let load: Load = std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetUtilizationRates"))?);
            let temperature: Temperature = std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetTemperature"))?);
            if init() != 0 {
                return None;
            }
            let mut device = 0usize;
            (handle(0, &mut device) == 0).then_some(Nvml { device, load, temperature })
        }
    }) else {
        return (None, None);
    };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.0.is_none_or(|at| at.elapsed().as_secs() >= 1) {
        let (mut rates, mut degrees) = ([0u32; 2], 0u32);
        // SAFETY: the device handle the library gave, and outputs of ours.
        let load = unsafe { (nvml.load)(nvml.device, &mut rates) == 0 }.then_some(rates[0]);
        // SAFETY: as above; sensor 0 is the GPU's own.
        let temperature = unsafe { (nvml.temperature)(nvml.device, 0, &mut degrees) == 0 }.then_some(degrees);
        *last = (Some(std::time::Instant::now()), (load, temperature));
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
    let mode = MODE.load(Ordering::Relaxed).max(1);
    let h = height(mode);
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
        let bitmap = CreateCompatibleBitmap(target, W, h);
        let old_bitmap = SelectObject(hdc, bitmap.into());
        let old_font = SelectObject(hdc, font.into());
        let back = CreateSolidBrush(rgb(14, 14, 20));
        FillRect(hdc, &RECT { left: 0, top: 0, right: W, bottom: h }, back);
        let _ = DeleteObject(back.into());
        SetBkMode(hdc, TRANSPARENT);
        let (dim, white, pink, red) = (rgb(150, 150, 165), rgb(235, 235, 240), rgb(255, 120, 170), rgb(255, 90, 90));
        let row = 18;
        let mut y = 6;
        if mode == 2 {
            text(hdc, 10, y, pink, "FLAMINGO");
            text(hdc, 250, y, dim, "F9 hide");
            y += row + 2;
        }
        text(hdc, 10, y, dim, "FPS");
        text(hdc, 60, y, rate_colour(s.fps), &format!("{:5.1}", s.fps));
        text(hdc, 150, y, white, &format!("{:6.1} ms", s.ms));
        y += row;
        let (load, temperature) = gpu_stats();
        let percent = |v: Option<f64>| v.map_or_else(|| "  -".into(), |v| format!("{v:3.0}%"));
        let vram = video_memory().map_or_else(|| "-".into(), |(used, _)| format!("{:.1} GB", used as f64 / 1e9));
        text(hdc, 10, y, dim, "GPU");
        text(hdc, 50, y, white, &percent(load.map(f64::from)));
        text(hdc, 100, y, white, &temperature.map_or_else(String::new, |t| format!("{t}\u{b0}C")));
        text(hdc, 170, y, dim, "VRAM");
        text(hdc, 220, y, white, &vram);
        y += row;
        text(hdc, 10, y, dim, "CPU");
        text(hdc, 50, y, white, &percent(cpu_load()));
        text(hdc, 170, y, dim, "RAM");
        text(hdc, 220, y, white, &format!("{:.1} GB", private_bytes() as f64 / 1e9));
        y += row;
        if mode == 2 {
            text(hdc, 10, y, dim, "Frames");
            text(hdc, 80, y, white, &frames.to_string());
            text(hdc, 170, y, dim, "Draws");
            text(hdc, 230, y, white, &format!("{:.0}", s.draws));
            y += row;
            // What a frame costs on the submitting thread.
            text(hdc, 10, y, dim, "Prep");
            text(hdc, 60, y, white, &format!("{:5.1} ms", s.cpu));
            text(hdc, 150, y, dim, "copies");
            text(hdc, 210, y, white, &format!("{:.1}/{:.0}MB", s.copies[0], s.copies[1]));
            y += row;
            text(hdc, 150, y, dim, "textures");
            text(hdc, 230, y, white, &format!("{:.1}/{:.0}MB", s.textures[0], s.textures[1]));
            y += row;
            text(hdc, 10, y, dim, "GPU wait");
            text(hdc, 90, y, white, &format!("{:5.1} ms", s.gpu));
            y += row;
            text(hdc, 10, y, dim, "Copy-back");
            text(hdc, 100, y, white, &format!("{:5.1} ms {:.0} MB", s.back, s.back_mb));
            y += row;
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
            y += row;
        }
        y += 6;
        // The frame-time graph, newest on the right; lines at 66.7 ms (15
        // fps) and 33.3 ms (30 fps).
        let (gx, gw, gh) = (10, W - 20, h - y - 8);
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
        let _ = BitBlt(target, 0, 0, W, h, Some(hdc), 0, 0, SRCCOPY);
        SelectObject(hdc, old_font);
        SelectObject(hdc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(hdc);
    }
}

#[cfg(test)]
mod tests {
    use super::{GRAPH, H, H_COMPACT, MODE, Second, TIMES, W, paint, second};
    use std::sync::atomic::Ordering;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
    };

    /// Pixels of the display as painted in `mode`, `h` rows.
    fn painted(mode: u8, h: i32) -> Vec<u32> {
        MODE.store(mode, Ordering::Relaxed);
        // SAFETY: GDI objects of this test, released below.
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, W, h);
            let old = SelectObject(dc, bitmap.into());
            paint(dc);
            SelectObject(dc, old);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: W,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u32; (W * h) as usize];
            let rows = GetDIBits(dc, bitmap, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
            assert_eq!(rows, h);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            pixels
        }
    }

    /// Both displays draw their text and graph: many pixels differ from the
    /// background. `FLAMINGO_OSD_SAVE=<file.bmp>` keeps the full one's picture.
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
            copies: [15.2, 75.4],
            textures: [8.2, 51.0],
            back_mb: 28.3,
        });
        {
            let mut t = TIMES.lock().unwrap();
            for k in 0..GRAPH {
                t.ms[k] = 80.0 + (k % 7) as f32 * 4.0;
            }
        }
        // The background is rgb(14, 14, 20): BGRX 0x0014_0e0e.
        let drawn = |p: &[u32]| p.iter().filter(|&&p| p & 0x00ff_ffff != 0x0014_0e0e).count();
        let compact = painted(1, H_COMPACT);
        assert!(drawn(&compact) > 800, "compact: only {} pixels drawn", drawn(&compact));
        let pixels = painted(2, H);
        assert!(drawn(&pixels) > 2000, "full: only {} pixels drawn", drawn(&pixels));
        if let Ok(path) = std::env::var("FLAMINGO_OSD_SAVE") {
            for (pixels, h, path) in [(&pixels, H, path.clone()), (&compact, H_COMPACT, path.replace(".bmp", "-compact.bmp"))] {
                let mut bmp = Vec::new();
                let size = 54 + pixels.len() * 4;
                bmp.extend_from_slice(b"BM");
                bmp.extend_from_slice(&(size as u32).to_le_bytes());
                bmp.extend_from_slice(&0u32.to_le_bytes());
                bmp.extend_from_slice(&54u32.to_le_bytes());
                bmp.extend_from_slice(&40u32.to_le_bytes());
                bmp.extend_from_slice(&W.to_le_bytes());
                bmp.extend_from_slice(&(-h).to_le_bytes());
                bmp.extend_from_slice(&1u16.to_le_bytes());
                bmp.extend_from_slice(&32u16.to_le_bytes());
                bmp.extend_from_slice(&[0u8; 24]);
                for p in pixels {
                    bmp.extend_from_slice(&p.to_le_bytes());
                }
                std::fs::write(path, bmp).unwrap();
            }
        }
    }
}
