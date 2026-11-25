//! Boot splash: an image over the XCL window while the title loads, faded out once it presents
//! real content.
//!
//! Loading a title takes 20–60 s of black frames. The splash is the title's own store art, read
//! from the user's game folder at run time (`MicrosoftGame.Config`: `SplashScreenImage`, else the
//! largest logo); nothing is embedded in XCL. Without such a file a dark background with a pulsing
//! bar is shown.
//!
//! It is a layered, click-through child window of the XCL window painted with GDI, so none of it
//! touches the title's D3D12 objects: the window thread fades it with a timer. The present path
//! only samples the title's frame (a 16x9 grid of texels copied into a small readback buffer
//! every few frames) and reports its average luminance. `LUMA_MIN` for `NEEDED` samples in a row
//! means real content: the splash fades over `FADE_SECS`. It also fades after `TIMEOUT_SECS`.
//! After the fade the window is destroyed and the sampling stops. `XCL_SPLASH=0` disables it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, GENERIC_READ, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const FADE_SECS: f64 = 0.6;
pub const TIMEOUT_SECS: f64 = 90.0;
/// the title's frame is sampled on every n-th presented frame
pub const SAMPLE_EVERY: u64 = 6;
/// consecutive bright samples that count as real content
pub const NEEDED: u32 = 5;
/// mean luminance (0..1, as presented) above which a sample is bright
pub const LUMA_MIN: f32 = 0.03;

// ---------------------------------------------------------------------------------------
// state machine
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Showing,
    Fading(f64),
    Done,
}

#[derive(Debug)]
pub struct Splash {
    phase: Phase,
    start: f64,
    streak: u32,
    frames: u64,
}

impl Splash {
    pub fn new(now: f64) -> Self {
        Splash { phase: Phase::Showing, start: now, streak: 0, frames: 0 }
    }

    /// A frame was presented: is its content to be sampled?
    pub fn on_frame(&mut self) -> bool {
        if self.phase != Phase::Showing {
            return false;
        }
        let due = self.frames % SAMPLE_EVERY == 0;
        self.frames += 1;
        due
    }

    /// The mean luminance of a sampled frame.
    pub fn on_luma(&mut self, luma: f32, now: f64) {
        if self.phase != Phase::Showing {
            return;
        }
        self.streak = if luma > LUMA_MIN { self.streak + 1 } else { 0 };
        if self.streak >= NEEDED {
            self.phase = Phase::Fading(now);
        }
    }

    /// Opacity of the splash now (None: finished).
    pub fn alpha(&mut self, now: f64) -> Option<f32> {
        if self.phase == Phase::Showing && now - self.start >= TIMEOUT_SECS {
            self.phase = Phase::Fading(now);
        }
        match self.phase {
            Phase::Showing => Some(1.0),
            Phase::Fading(s) => {
                let t = ((now - s) / FADE_SECS).clamp(0.0, 1.0) as f32;
                if t >= 1.0 {
                    self.phase = Phase::Done;
                    return None;
                }
                Some(1.0 - t * t * (3.0 - 2.0 * t))
            }
            Phase::Done => None,
        }
    }
}

/// The largest rectangle of the image's aspect that fits the client area, centered:
/// (x, y, width, height) in pixels.
pub fn letterbox(img_w: u32, img_h: u32, dst_w: u32, dst_h: u32) -> (i32, i32, i32, i32) {
    if img_w == 0 || img_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w as i32, dst_h as i32);
    }
    let scale = (dst_w as f64 / img_w as f64).min(dst_h as f64 / img_h as f64);
    let w = ((img_w as f64 * scale).round() as i32).clamp(1, dst_w as i32);
    let h = ((img_h as f64 * scale).round() as i32).clamp(1, dst_h as i32);
    ((dst_w as i32 - w) / 2, (dst_h as i32 - h) / 2, w, h)
}

/// `name="value"` in the config's text (the attribute after a space).
fn attribute<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = text.find(&key)? + key.len();
    let v = &text[i..i + text[i..].find('"')?];
    (!v.is_empty()).then_some(v)
}

/// Image files of `MicrosoftGame.Config`'s `ShellVisuals`, best first: the splash screen, then
/// the logos from largest to smallest.
pub fn image_candidates(config: &str) -> Vec<String> {
    ["SplashScreenImage", "Square480x480Logo", "StoreLogo", "Square150x150Logo", "Square44x44Logo"]
        .iter()
        .filter_map(|n| attribute(config, n))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------------------
// global state
// ---------------------------------------------------------------------------------------

static STATE: Mutex<Option<Splash>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CHILD: AtomicIsize = AtomicIsize::new(0);

fn secs() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

pub fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| std::env::var("XCL_SPLASH").map_or(true, |v| v.trim() != "0"))
}

/// The splash is showing or fading: the present path samples frames.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// A frame was presented: true when its content is to be sampled (`Sampler::record`).
pub fn note_present() -> bool {
    STATE.lock().unwrap().as_mut().is_some_and(Splash::on_frame)
}

pub fn note_luma(luma: f32) {
    if let Some(s) = STATE.lock().unwrap().as_mut() {
        s.on_luma(luma, secs());
    }
}

fn alpha_now() -> Option<f32> {
    let a = STATE.lock().unwrap().as_mut().and_then(|s| s.alpha(secs()));
    if a.is_none() {
        ACTIVE.store(false, Ordering::Relaxed);
    }
    a
}

// ---------------------------------------------------------------------------------------
// the window (created and driven on the XCL window thread)
// ---------------------------------------------------------------------------------------

struct Image {
    w: u32,
    h: u32,
    bgra: Vec<u8>,
}

static IMAGE: Mutex<Option<Image>> = Mutex::new(None);
static LAST_ALPHA: AtomicIsize = AtomicIsize::new(-1);
const TIMER: usize = 0x5917;

/// The first of `XCL_SPLASH`'s candidates that decodes, from the game folder.
unsafe fn load_image() -> Option<Image> {
    let boot = crate::state::boot()?;
    let text = std::fs::read_to_string(boot.game_root.join("MicrosoftGame.Config")).ok()?;
    for name in image_candidates(&text) {
        let path = boot.game_root.join(name.replace('/', "\\"));
        if !path.is_file() {
            continue;
        }
        match decode(&path) {
            Some(img) => {
                crate::xlog_info!("splash: {} ({}x{})", path.display(), img.w, img.h);
                return Some(img);
            }
            None => crate::xlog_warn!("splash: could not decode {}", path.display()),
        }
    }
    None
}

unsafe fn decode(path: &std::path::Path) -> Option<Image> {
    let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
    let dec = factory
        .CreateDecoderFromFilename(&HSTRING::from(path.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
        .ok()?;
    let frame: IWICBitmapSource = dec.GetFrame(0).ok()?.cast().ok()?;
    let conv = factory.CreateFormatConverter().ok()?;
    conv.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
    let (mut w, mut h) = (0u32, 0u32);
    conv.GetSize(&mut w, &mut h).ok()?;
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }
    let mut bgra = vec![0u8; w as usize * h as usize * 4];
    conv.CopyPixels(std::ptr::null(), w * 4, &mut bgra).ok()?;
    Some(Image { w, h, bgra })
}

/// Called by the window thread once the XCL window exists.
pub unsafe fn attach(parent: HWND) {
    if !enabled() {
        return;
    }
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    *IMAGE.lock().unwrap() = load_image();
    let Ok(inst) = GetModuleHandleW(PCWSTR::null()) else { return };
    let class = windows::core::w!("XCLSplash");
    let wc = WNDCLASSW { lpfnWndProc: Some(splash_proc), hInstance: inst.into(), lpszClassName: class, ..Default::default() };
    RegisterClassW(&wc);
    let mut rc = RECT::default();
    let _ = GetClientRect(parent, &mut rc);
    let child = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT,
        class,
        windows::core::w!(""),
        WS_CHILD | WS_VISIBLE | WS_DISABLED,
        0,
        0,
        rc.right.max(1),
        rc.bottom.max(1),
        Some(parent),
        None,
        Some(inst.into()),
        None,
    );
    let Ok(child) = child else { return };
    let _ = SetLayeredWindowAttributes(child, COLORREF(0), 255, LWA_ALPHA);
    CHILD.store(child.0 as isize, Ordering::Release);
    *STATE.lock().unwrap() = Some(Splash::new(secs()));
    ACTIVE.store(true, Ordering::Relaxed);
    SetTimer(Some(child), TIMER, 16, None);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (cw, ch) = (rc.right.max(0) as u32, rc.bottom.max(0) as u32);
    let img = IMAGE.lock().unwrap();
    let bg = if img.is_some() { COLORREF(0) } else { COLORREF(0x0014_1010) };
    let brush = CreateSolidBrush(bg);
    FillRect(hdc, &rc, brush);
    let _ = DeleteObject(brush.into());
    if let Some(img) = img.as_ref() {
        let (x, y, w, h) = letterbox(img.w, img.h, cw, ch);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: img.w as i32,
                biHeight: -(img.h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        SetStretchBltMode(hdc, HALFTONE);
        let _ = SetBrushOrgEx(hdc, 0, 0, None);
        StretchDIBits(hdc, x, y, w, h, 0, 0, img.w as i32, img.h as i32, Some(img.bgra.as_ptr() as *const c_void), &bmi, DIB_RGB_COLORS, SRCCOPY);
    } else {
        // no art: a pulsing bar
        let pulse = 0.5 + 0.5 * (secs() * 3.0).sin();
        let v = (0x30 as f64 + pulse * 0x90 as f64) as u32;
        let bar = RECT { left: rc.right * 2 / 5, right: rc.right * 3 / 5, top: rc.bottom / 2 - 3, bottom: rc.bottom / 2 + 3 };
        let b = CreateSolidBrush(COLORREF(v | v << 8 | v << 16));
        FillRect(hdc, &bar, b);
        let _ = DeleteObject(b.into());
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn splash_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(-1), // HTTRANSPARENT: the mouse goes to the game window
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER => {
            let Some(a) = alpha_now() else {
                let _ = KillTimer(Some(hwnd), TIMER);
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            };
            let v = (a * 255.0).round() as isize;
            if LAST_ALPHA.swap(v, Ordering::Relaxed) != v {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), v as u8, LWA_ALPHA);
            }
            // follow the game window's size
            if let Ok(parent) = GetParent(hwnd) {
                let (mut p, mut c) = (RECT::default(), RECT::default());
                let _ = GetClientRect(parent, &mut p);
                let _ = GetClientRect(hwnd, &mut c);
                if (p.right, p.bottom) != (c.right, c.bottom) && p.right > 0 && p.bottom > 0 {
                    let _ = MoveWindow(hwnd, 0, 0, p.right, p.bottom, true);
                }
            }
            if IMAGE.lock().unwrap().is_none() {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            *IMAGE.lock().unwrap() = None;
            CHILD.store(0, Ordering::Release);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------------------------------------------------------------------------------------
// sampling the title's frame
// ---------------------------------------------------------------------------------------

const GRID_X: u32 = 16;
const GRID_Y: u32 = 9;
const SLOT: u64 = 512; // placed footprint offsets are 512-byte aligned

/// A 16x9 grid of texels of the title's frame, copied into a readback buffer.
pub struct Sampler {
    buf: ID3D12Resource,
    fmt: DXGI_FORMAT,
    pending: bool,
}

unsafe impl Send for Sampler {}

impl Sampler {
    pub unsafe fn new(dev: &ID3D12Device) -> Option<Self> {
        let mut rb: Option<ID3D12Resource> = None;
        dev.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: SLOT * (GRID_X * GRID_Y) as u64,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut rb,
        )
        .ok()?;
        Some(Sampler { buf: rb?, fmt: DXGI_FORMAT_UNKNOWN, pending: false })
    }

    /// Record the grid copies of `src` (a w x h texture of `fmt`, 4 or 8 bytes per texel).
    pub unsafe fn record(&mut self, list: &ID3D12GraphicsCommandList, src: &ID3D12Resource, fmt: DXGI_FORMAT, w: u32, h: u32) {
        if texel_bytes(fmt).is_none() || w < GRID_X || h < GRID_Y {
            return;
        }
        let s = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(src), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
        for j in 0..GRID_Y {
            for i in 0..GRID_X {
                let (x, y) = ((i * 2 + 1) * w / (GRID_X * 2), (j * 2 + 1) * h / (GRID_Y * 2));
                let fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: SLOT * (j * GRID_X + i) as u64,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: fmt, Width: 1, Height: 1, Depth: 1, RowPitch: 256 },
                };
                let d = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&self.buf), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp } };
                let b = D3D12_BOX { left: x, top: y, front: 0, right: x + 1, bottom: y + 1, back: 1 };
                list.CopyTextureRegion(&d, 0, 0, 0, &s, Some(&b));
            }
        }
        self.fmt = fmt;
        self.pending = true;
    }

    /// The mean luminance of the grid recorded last, once its copy has finished.
    pub unsafe fn harvest(&mut self) -> Option<f32> {
        if !std::mem::take(&mut self.pending) {
            return None;
        }
        let mut p: *mut c_void = std::ptr::null_mut();
        self.buf.Map(0, None, Some(&mut p)).ok()?;
        if p.is_null() {
            return None;
        }
        let mut sum = 0.0f32;
        for k in 0..GRID_X * GRID_Y {
            let at = (p as *const u8).add(k as usize * SLOT as usize);
            sum += texel_luma(self.fmt, std::slice::from_raw_parts(at, 8));
        }
        self.buf.Unmap(0, None);
        Some(sum / (GRID_X * GRID_Y) as f32)
    }
}

fn texel_bytes(fmt: DXGI_FORMAT) -> Option<u32> {
    match fmt {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R10G10B10A2_UNORM => Some(4),
        DXGI_FORMAT_R16G16B16A16_FLOAT => Some(8),
        _ => None,
    }
}

fn half(h: u16) -> f32 {
    let (e, m) = ((h >> 10 & 0x1F) as i32, (h & 0x3FF) as f32);
    let v = match e {
        0 => m * 2f32.powi(-24),
        31 => 1.0,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    };
    if h & 0x8000 != 0 { 0.0 } else { v }
}

/// Luminance 0..1 of the texel at the start of `b`.
fn texel_luma(fmt: DXGI_FORMAT, b: &[u8]) -> f32 {
    let rgb = if fmt == DXGI_FORMAT_R16G16B16A16_FLOAT {
        let r = |i: usize| half(u16::from_le_bytes([b[i], b[i + 1]])).min(1.0);
        [r(0), r(2), r(4)]
    } else {
        let t = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let c = match fmt {
            DXGI_FORMAT_R10G10B10A2_UNORM => [(t & 0x3FF) as f32 / 1023.0, (t >> 10 & 0x3FF) as f32 / 1023.0, (t >> 20 & 0x3FF) as f32 / 1023.0],
            DXGI_FORMAT_B8G8R8A8_UNORM => [(t >> 16 & 0xFF) as f32 / 255.0, (t >> 8 & 0xFF) as f32 / 255.0, (t & 0xFF) as f32 / 255.0],
            _ => [(t & 0xFF) as f32 / 255.0, (t >> 8 & 0xFF) as f32 / 255.0, (t >> 16 & 0xFF) as f32 / 255.0],
        };
        c
    };
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bright(s: &mut Splash, n: u32, t: f64) {
        for _ in 0..n {
            s.on_luma(0.5, t);
        }
    }

    #[test]
    fn shows_until_enough_consecutive_bright_samples() {
        let mut s = Splash::new(0.0);
        bright(&mut s, NEEDED - 1, 1.0);
        assert_eq!(s.alpha(1.0), Some(1.0));
        s.on_luma(0.0, 1.0); // a dark frame resets the streak
        bright(&mut s, NEEDED - 1, 2.0);
        assert_eq!(s.alpha(2.0), Some(1.0));
        s.on_luma(0.5, 2.0);
        assert_eq!(s.alpha(2.0), Some(1.0)); // fade has just begun: still fully shown
        assert!(s.alpha(2.3).unwrap() < 1.0);
    }

    #[test]
    fn threshold_is_exclusive_and_dark_frames_never_fade() {
        let mut s = Splash::new(0.0);
        for _ in 0..100 {
            s.on_luma(LUMA_MIN, 1.0);
        }
        assert_eq!(s.alpha(1.0), Some(1.0));
    }

    #[test]
    fn fade_is_smooth_monotonic_and_ends() {
        let mut s = Splash::new(0.0);
        bright(&mut s, NEEDED, 10.0);
        let mut last = 1.0;
        let mut t = 10.0;
        while t < 10.0 + FADE_SECS - 0.02 {
            let a = s.alpha(t).unwrap();
            assert!(a <= last && (0.0..=1.0).contains(&a));
            last = a;
            t += 0.01;
        }
        assert!(last < 0.05);
        assert_eq!(s.alpha(10.0 + FADE_SECS + 0.01), None);
        assert_eq!(s.alpha(11.0), None);
        // a finished splash samples nothing
        assert!(!s.on_frame());
    }

    #[test]
    fn fades_after_the_timeout_without_content() {
        let mut s = Splash::new(5.0);
        assert_eq!(s.alpha(5.0 + TIMEOUT_SECS - 0.1), Some(1.0));
        assert_eq!(s.alpha(5.0 + TIMEOUT_SECS), Some(1.0));
        assert!(s.alpha(5.0 + TIMEOUT_SECS + 0.3).unwrap() < 1.0);
        assert_eq!(s.alpha(5.0 + TIMEOUT_SECS + FADE_SECS + 0.1), None);
    }

    #[test]
    fn samples_every_nth_frame_and_stops_when_fading() {
        let mut s = Splash::new(0.0);
        let due: Vec<bool> = (0..2 * SAMPLE_EVERY).map(|_| s.on_frame()).collect();
        assert_eq!(due.iter().filter(|d| **d).count(), 2);
        assert!(due[0] && due[SAMPLE_EVERY as usize]);
        bright(&mut s, NEEDED, 1.0);
        assert!((0..20).all(|_| !s.on_frame()));
    }

    #[test]
    fn letterbox_math() {
        assert_eq!(letterbox(1920, 1080, 1280, 720), (0, 0, 1280, 720));
        // wide window: pillarbox
        assert_eq!(letterbox(1920, 1080, 1920, 800), (249, 0, 1422, 800));
        // tall window: bars above and below
        assert_eq!(letterbox(1920, 1080, 800, 800), (0, 175, 800, 450));
        // square image in a wide window
        assert_eq!(letterbox(480, 480, 1600, 900), (350, 0, 900, 900));
        // upscales small images; degenerate sizes fill the area
        assert_eq!(letterbox(100, 50, 400, 400), (0, 100, 400, 200));
        assert_eq!(letterbox(0, 0, 640, 360), (0, 0, 640, 360));
    }

    #[test]
    fn picks_the_splash_then_logos() {
        let cfg = r##"<ShellVisuals DefaultDisplayName="G" Square150x150Logo="a150.png" Square480x480Logo="a480.png" SplashScreenImage="splash.png" StoreLogo="store.png" BackgroundColor="#000000" />"##;
        assert_eq!(image_candidates(cfg), ["splash.png", "a480.png", "store.png", "a150.png"]);
        assert_eq!(image_candidates("<ShellVisuals StoreLogo=\"\" Square44x44Logo=\"s.png\" />"), ["s.png"]);
        assert!(image_candidates("<Game />").is_empty());
    }

    #[test]
    fn luma_of_texels() {
        assert_eq!(texel_luma(DXGI_FORMAT_R8G8B8A8_UNORM, &[0, 0, 0, 255, 0, 0, 0, 0]), 0.0);
        assert!((texel_luma(DXGI_FORMAT_R8G8B8A8_UNORM, &[255, 255, 255, 255, 0, 0, 0, 0]) - 1.0).abs() < 1e-4);
        // BGRA: the red channel is the third byte
        assert!((texel_luma(DXGI_FORMAT_B8G8R8A8_UNORM, &[0, 0, 255, 255, 0, 0, 0, 0]) - 0.2126).abs() < 1e-4);
        // half 1.0 = 0x3C00
        let one = 0x3C00u16.to_le_bytes();
        let b = [one[0], one[1], one[0], one[1], one[0], one[1], 0, 0];
        assert!((texel_luma(DXGI_FORMAT_R16G16B16A16_FLOAT, &b) - 1.0).abs() < 1e-3);
    }
}
