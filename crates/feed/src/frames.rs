//! The reader side of the game's frame double buffer (layout in the d3d12
//! provider's present/feed.rs). Either side may create the mapping first.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
const HEADER: usize = 64;
const SLOT_HEADER: usize = 64;
const SLOT: usize = SLOT_HEADER + MAX_W * MAX_H * 4;
const SIZE: usize = HEADER + 2 * SLOT;

struct View(*mut u8);
// SAFETY: mapped for the process's lifetime; fields are atomics or seqlocked.
unsafe impl Send for View {}
unsafe impl Sync for View {}

impl View {
    fn u32(&self, at: usize) -> &AtomicU32 {
        unsafe { &*(self.0.add(at) as *const AtomicU32) }
    }
    fn u64(&self, at: usize) -> &AtomicU64 {
        unsafe { &*(self.0.add(at) as *const AtomicU64) }
    }
}

fn view() -> Option<&'static View> {
    static V: OnceLock<Option<View>> = OnceLock::new();
    V.get_or_init(|| unsafe {
        use windows_sys::Win32::System::Memory::*;
        let wide: Vec<u16> = crate::FRAMES_NAME.encode_utf16().chain(Some(0)).collect();
        let mapping = CreateFileMappingW(
            windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE,
            (SIZE as u64 >> 32) as u32, SIZE as u32, wide.as_ptr());
        if mapping.is_null() {
            crate::say("shared frame memory unavailable");
            return None;
        }
        let at = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, SIZE);
        (!at.Value.is_null()).then(|| View(at.Value.cast()))
    }).as_ref()
}

/// The feed's rate and width, read by the game at each capture.
pub(crate) fn configure(fps: u32, width: u32) {
    if let Some(v) = view() {
        v.u32(16).store(fps, Ordering::Relaxed);
        v.u32(20).store(width, Ordering::Relaxed);
    }
}

/// Someone is watching: the game captures for 5 s after the last call.
pub(crate) fn heartbeat() {
    if let Some(v) = view() {
        v.u64(8).store(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }, Ordering::Release);
    }
}

pub(crate) struct Frame {
    pub width: usize,
    pub height: usize,
    /// width * height * 4 bytes, B G R 0.
    pub pixels: Arc<Vec<u8>>,
    pub number: u64,
}

/// The newest complete frame, if one was published since `after`.
pub(crate) fn latest(after: u64) -> Option<Frame> {
    let v = view()?;
    let number = v.u64(32).load(Ordering::Acquire);
    if number == 0 || number == after {
        return None;
    }
    for _ in 0..3 {
        let slot = v.u32(24).load(Ordering::Acquire) as usize & 1;
        let base = HEADER + slot * SLOT;
        let before = v.u64(base).load(Ordering::Acquire);
        if before & 1 == 1 {
            std::thread::yield_now();
            continue;
        }
        let (w, h) = (v.u32(base + 8).load(Ordering::Relaxed) as usize, v.u32(base + 12).load(Ordering::Relaxed) as usize);
        if w == 0 || h == 0 || w > MAX_W || h > MAX_H {
            return None;
        }
        // SAFETY: inside the view; a torn copy is caught by the seq check.
        let pixels = unsafe { std::slice::from_raw_parts(v.0.add(base + SLOT_HEADER), w * h * 4) }.to_vec();
        std::sync::atomic::fence(Ordering::Acquire);
        if v.u64(base).load(Ordering::Acquire) == before {
            return Some(Frame { width: w, height: h, pixels: Arc::new(pixels), number });
        }
    }
    None
}

/// What the waiting screen says: "live" while frames arrive, "paused" when
/// they stopped within the last minute, else "none" (no game presenting).
pub(crate) fn game() -> &'static str {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST: Mutex<(u64, Option<Instant>)> = Mutex::new((0, None));
    let now = published();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if now != last.0 {
        *last = (now, Some(Instant::now()));
    }
    match last.1.map(|t| t.elapsed()) {
        Some(d) if d < Duration::from_secs(3) => "live",
        _ if window_live() => "live",
        Some(d) if d < Duration::from_secs(60) => "paused",
        _ => "none",
    }
}

pub(crate) fn published() -> u64 {
    view().map_or(0, |v| v.u64(32).load(Ordering::Acquire))
}

// ---- Window capture: the launcher's loader and any runtime's game window ----
// Runtimes other than neonbridge-own have no frame tap, and the loader is the
// launcher's own window, so the feed also captures the largest visible
// top-level window of the launcher or a watched game process (PrintWindow with
// PW_RENDERFULLCONTENT). NEONBRIDGE_FEED_WINDOWS=0 turns this off.

static PIDS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
static WIN_NUMBER: AtomicU64 = AtomicU64::new(1 << 62);
static GAME_SEEN: std::sync::Mutex<(u64, Option<std::time::Instant>)> = std::sync::Mutex::new((0, None));

/// A process whose windows the feed may show (the game the launcher started).
pub fn watch_pid(pid: u32) {
    let mut p = PIDS.lock().unwrap_or_else(|e| e.into_inner());
    if !p.contains(&pid) {
        p.push(pid);
    }
}

fn windows_on() -> bool {
    std::env::var("NEONBRIDGE_FEED_WINDOWS").map_or(true, |v| v.trim() != "0")
}

/// Game frames published within the last second win over window capture.
fn game_fresh() -> bool {
    let now = published();
    let mut g = GAME_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if now != g.0 {
        *g = (now, Some(std::time::Instant::now()));
    }
    g.1.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1))
}

fn target_window() -> Option<windows_sys::Win32::Foundation::HWND> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let mut pids = PIDS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    pids.push(std::process::id());
    struct Ctx { pids: Vec<u32>, best: HWND, area: i64 }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> windows_sys::core::BOOL {
        let c = unsafe { &mut *(l as *mut Ctx) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, &mut pid) };
        if c.pids.contains(&pid) && unsafe { IsWindowVisible(h) } != 0 && unsafe { IsIconic(h) } == 0 {
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(h, &mut r) };
            let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
            if area > c.area && r.right - r.left >= 64 && r.bottom - r.top >= 64 {
                c.area = area;
                c.best = h;
            }
        }
        1
    }
    let mut c = Ctx { pids, best: std::ptr::null_mut(), area: 0 };
    unsafe { EnumWindows(Some(each), &mut c as *mut Ctx as LPARAM) };
    (!c.best.is_null()).then_some(c.best)
}

fn capture(max_w: usize) -> Option<(usize, usize, Vec<u8>)> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
    let h = target_window()?;
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(h, &mut r);
        let (sw, sh) = ((r.right - r.left) as usize, (r.bottom - r.top) as usize);
        let scale = (max_w.min(MAX_W) as f64 / sw as f64).min(MAX_H as f64 / sh as f64).min(1.0);
        let w = ((sw as f64 * scale) as usize) & !1;
        let hgt = ((sh as f64 * scale) as usize) & !1;
        if w < 2 || hgt < 2 {
            return None;
        }
        let screen = GetDC(std::ptr::null_mut());
        let full_dc = CreateCompatibleDC(screen);
        let full = CreateCompatibleBitmap(screen, sw as i32, sh as i32);
        let old_full = SelectObject(full_dc, full);
        // 1 = PW_CLIENTONLY, 2 = PW_RENDERFULLCONTENT (DirectX swap chains)
        let ok = PrintWindow(h, full_dc, 3) != 0;
        let out_dc = CreateCompatibleDC(screen);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w as i32;
        bi.bmiHeader.biHeight = -(hgt as i32);
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut px = None;
        if ok && !dib.is_null() && !bits.is_null() {
            let old_out = SelectObject(out_dc, dib);
            SetStretchBltMode(out_dc, HALFTONE);
            StretchBlt(out_dc, 0, 0, w as i32, hgt as i32, full_dc, 0, 0, sw as i32, sh as i32, SRCCOPY);
            GdiFlush();
            px = Some(std::slice::from_raw_parts(bits as *const u8, w * hgt * 4).to_vec());
            SelectObject(out_dc, old_out);
        }
        if !dib.is_null() {
            DeleteObject(dib);
        }
        SelectObject(full_dc, old_full);
        DeleteObject(full);
        DeleteDC(full_dc);
        DeleteDC(out_dc);
        ReleaseDC(std::ptr::null_mut(), screen);
        px.map(|p| (w, hgt, p))
    }
}

/// The next frame: the game's tap while it publishes, else a window capture.
pub(crate) fn next_frame(after: u64) -> Option<Frame> {
    if game_fresh() || !windows_on() {
        return latest(after);
    }
    let width = view().map_or(1280, |v| v.u32(20).load(Ordering::Relaxed) as usize).max(160);
    let (w, h, pixels) = capture(width)?;
    Some(Frame { width: w, height: h, pixels: Arc::new(pixels), number: WIN_NUMBER.fetch_add(1, Ordering::Relaxed) })
}

/// A window the feed can show exists (the loader or a game).
pub(crate) fn window_live() -> bool {
    windows_on() && target_window().is_some()
}
