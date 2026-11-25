//! The boot splash (inside the title's window) and the sampling of the presented frame (the decisions are in
//! `victory_core::splash`, which is tested anywhere).
//!
//! [`start`] is called first thing when a title is run: a thread of its own reads the user's own
//! store art from the game folder (WIC) and runs the splash's clock. There is no window of its own:
//! the picture is shown inside the title's window. Until the first present it is painted there
//! with GDI (with a thin progress bar that follows the start's phases); from the first present on,
//! the present path draws it over the composited frame at the splash's opacity ([`alpha`]). It
//! fades when the composited frame (game plane plus interface plane) shows content, on a key or
//! button press, or after `VICTORY_SPLASH_MAX_S`; then the thread ends and the present path
//! samples and draws nothing more. `VICTORY_SPLASH=0` turns it off.
//!
//! The title's D3D12 objects are never touched: [`Sampler`] reads a few texels of the frame
//! the host presents anyway.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use victory_core::splash::{self as logic, Splash, Texel};
use windows::Win32::Foundation::{COLORREF, GENERIC_READ, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, HSTRING, w};

static STATE: Mutex<Option<Splash>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PRESENTED: AtomicBool = AtomicBool::new(false);
/// The splash's opacity now, as f32 bits.
static ALPHA: AtomicU32 = AtomicU32::new(0x3f80_0000);
static PICTURE: Mutex<Option<std::sync::Arc<Image>>> = Mutex::new(None);

fn secs() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn state() -> std::sync::MutexGuard<'static, Option<Splash>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The splash shows or fades: the present path samples frames.
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Whether `hwnd` is the splash: never now, the splash is drawn inside the title's window.
pub(crate) fn is_splash(_hwnd: HWND) -> bool {
    false
}

/// The title presented: from now on the present path draws the splash, not GDI.
pub(crate) fn mark_presented() {
    PRESENTED.store(true, Ordering::Release);
}

/// A frame was presented: true when its content is to be sampled.
pub(crate) fn note_present() -> bool {
    state().as_mut().is_some_and(Splash::on_frame)
}

pub(crate) fn note_luma(luma: f32) {
    if let Some(s) = state().as_mut() {
        s.on_luma(luma, secs());
    }
}

/// Shows the splash now (see the module's text). `game_dir` is the title's folder, which the
/// picture is read from. Does nothing with `VICTORY_SPLASH=0`.
pub fn start(game_dir: &Path) {
    if !logic::enabled() || ACTIVE.swap(true, Ordering::AcqRel) {
        return;
    }
    *state() = Some(Splash::new(secs(), logic::max_secs()));
    let dir = game_dir.to_owned();
    let spawned = std::thread::Builder::new()
        .name("victory-splash".into())
        .spawn(move || thread(dir));
    if spawned.is_err() {
        ACTIVE.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------------------
// the window
// ---------------------------------------------------------------------------------------

pub(crate) struct Image {
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) bgra: Vec<u8>,
}

#[derive(Default)]
struct Ui {
    progress: f32,
    /// the title's window, once it exists
    title: Option<HWND>,
    last_search: f64,
    last_pad: f64,
    keys: Vec<bool>,
    pad: [bool; 1],
}

fn thread(dir: PathBuf) {
    // SAFETY: COM for WIC on this thread; GDI on the title's window from this thread.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Some(img) = load_image(&dir) {
            *picture_slot() = Some(std::sync::Arc::new(img));
        }
        let mut ui = Ui::default();
        loop {
            let now = secs();
            let press = pressed(&mut ui, now);
            let view = {
                let mut guard = state();
                let Some(s) = guard.as_mut() else { break };
                if press {
                    s.on_input(now);
                }
                s.tick(now, victory_core::splash::phase())
            };
            let Some(v) = view else { break };
            ALPHA.store(v.alpha.to_bits(), Ordering::Release);
            ui.progress = v.progress;
            // Until the first present the title's window is still GDI's: the picture is painted into it.
            if !PRESENTED.load(Ordering::Acquire) {
                if ui.title.is_none_or(|t| !IsWindow(Some(t)).as_bool()) && now - ui.last_search >= 0.05 {
                    ui.last_search = now;
                    ui.title = find_title(HWND::default());
                }
                if let Some(title) = ui.title {
                    paint_into(title, &ui);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
    ALPHA.store(0f32.to_bits(), Ordering::Release);
    ACTIVE.store(false, Ordering::Release);
    *picture_slot() = None;
    *state() = None;
}

/// The first picture of the game folder's `MicrosoftGame.config` that decodes.
unsafe fn load_image(dir: &Path) -> Option<Image> {
    let text = std::fs::read_to_string(dir.join("MicrosoftGame.config")).ok()?;
    for name in logic::image_candidates(&text) {
        let path = dir.join(name.replace('/', "\\"));
        if !path.is_file() {
            continue;
        }
        // SAFETY: WIC calls on this COM-initialised thread.
        if let Some(img) = unsafe { decode(&path) } {
            tracing::info!(path = %path.display(), width = img.w, height = img.h, "splash picture");
            return Some(img);
        }
        tracing::warn!(path = %path.display(), "splash picture could not be decoded");
    }
    None
}

unsafe fn decode(path: &Path) -> Option<Image> {
    // SAFETY: WIC objects used on one thread.
    unsafe {
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
}

fn picture_slot() -> std::sync::MutexGuard<'static, Option<std::sync::Arc<Image>>> {
    PICTURE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The splash picture (BGRA, `w` x `h`), while the splash shows.
pub(crate) fn picture() -> Option<std::sync::Arc<Image>> {
    picture_slot().clone()
}

/// How much the splash covers the title's frame now (0 nothing .. 1 all of it).
pub(crate) fn alpha() -> f32 {
    if active() { f32::from_bits(ALPHA.load(Ordering::Acquire)) } else { 0.0 }
}

/// Paints the picture (letterboxed, black around it) and the bar into the title's client area.
unsafe fn paint_into(title: HWND, ui: &Ui) {
    // SAFETY: GDI on a live window's DC, released before returning.
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(title, &mut rc).is_err() || rc.right <= 0 || rc.bottom <= 0 {
            return;
        }
        let (w, h) = (rc.right, rc.bottom);
        let hdc = GetDC(Some(title));
        if hdc.is_invalid() {
            return;
        }
        let pic = picture();
        let fill = |r: RECT, color: u32| {
            if r.right > r.left && r.bottom > r.top {
                let brush = CreateSolidBrush(COLORREF(color));
                FillRect(hdc, &r, brush);
                let _ = DeleteObject(brush.into());
            }
        };
        match pic.as_deref() {
            Some(img) => {
                let (x, y, dw, dh) = logic::letterbox(img.w, img.h, w as u32, h as u32);
                fill(RECT { left: 0, top: 0, right: w, bottom: y }, 0);
                fill(RECT { left: 0, top: y + dh, right: w, bottom: h }, 0);
                fill(RECT { left: 0, top: y, right: x, bottom: y + dh }, 0);
                fill(RECT { left: x + dw, top: y, right: w, bottom: y + dh }, 0);
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
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
                StretchDIBits(hdc, x, y, dw, dh, 0, 0, img.w as i32, img.h as i32, Some(img.bgra.as_ptr() as *const c_void), &bmi, DIB_RGB_COLORS, SRCCOPY);
            }
            None => fill(RECT { left: 0, top: 0, right: w, bottom: h }, 0x0014_1010),
        }
        let (track, bar, glint) = logic::bar_rects(w, h, ui.progress, secs());
        for (area, color) in [(track, 0x0030_3030u32), (bar, 0x00C8_C8C8), (glint, 0x00FF_FFFF)] {
            fill(rect(area), color);
        }
        ReleaseDC(Some(title), hdc);
    }
}

fn rect(r: logic::Rect) -> RECT {
    RECT { left: r.0, top: r.1, right: r.2, bottom: r.3 }
}

/// The pid of the window's process (0 if none).
fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0;
    // SAFETY: a plain query.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// A key, mouse button or pad button went down since the last poll, while this process has the focus.
unsafe fn pressed(ui: &mut Ui, now: f64) -> bool {
    // SAFETY: plain queries.
    unsafe {
        let foreground = GetForegroundWindow();
        let mine = !foreground.0.is_null() && pid_of(foreground) == std::process::id();
        let mut down = vec![false; 255];
        if mine {
            for (vk, d) in down.iter_mut().enumerate().skip(1) {
                *d = GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0;
            }
        }
        if ui.keys.len() != down.len() {
            ui.keys = vec![true; down.len()]; // what is held at the start does not count
        }
        let mut any = logic::new_press(&mut ui.keys, &down);
        if mine && now - ui.last_pad >= 0.1 {
            ui.last_pad = now;
            let now_down = [pad_down()];
            any |= logic::new_press(&mut ui.pad, &now_down);
        }
        any
    }
}

/// Whether a pad has a button down (XInput, loaded when first asked: nothing is imported).
unsafe fn pad_down() -> bool {
    #[repr(C)]
    #[derive(Default)]
    struct XState {
        packet: u32,
        buttons: u16,
        triggers: [u8; 2],
        sticks: [i16; 4],
    }
    type Get = unsafe extern "system" fn(u32, *mut XState) -> u32;
    static GET: OnceLock<Option<usize>> = OnceLock::new();
    // SAFETY: the loaded entry point has this signature.
    unsafe {
        let get = GET.get_or_init(|| {
            let lib = LoadLibraryW(w!("xinput1_4.dll")).ok()?;
            GetProcAddress(lib, windows::core::s!("XInputGetState")).map(|f| f as usize)
        });
        let Some(get) = *get else { return false };
        let get: Get = std::mem::transmute::<usize, Get>(get);
        (0..4).any(|user| {
            let mut s = XState::default();
            get(user, &mut s) == 0 && (s.buttons != 0 || s.triggers.iter().any(|&t| t > 200))
        })
    }
}

/// A visible top-level window of this process that is not the splash: the title's.
fn find_title(splash: HWND) -> Option<HWND> {
    struct Search {
        splash: HWND,
        found: Option<HWND>,
    }
    extern "system" fn visit(hwnd: HWND, search: LPARAM) -> windows::core::BOOL {
        // SAFETY: `search` is the `Search` below, alive for the call.
        unsafe {
            let search = &mut *(search.0 as *mut Search);
            // The console window a console program is attached to reports the program's pid too.
            let mut class = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let console = String::from_utf16_lossy(&class[..n]) == "ConsoleWindowClass";
            if hwnd != search.splash && !console && pid_of(hwnd) == std::process::id() && IsWindowVisible(hwnd).as_bool() {
                search.found = Some(hwnd);
                return false.into();
            }
        }
        true.into()
    }
    let mut search = Search { splash, found: None };
    // SAFETY: the callback only writes `search`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&raw mut search as isize)) };
    search.found
}

// ---------------------------------------------------------------------------------------
// sampling the presented frame
// ---------------------------------------------------------------------------------------

const GRID_X: u32 = 16;
const GRID_Y: u32 = 9;
/// placed footprint offsets are 512-byte aligned
const SLOT: u64 = 512;

enum Pending {
    Idle,
    /// Recorded in the present list being built.
    Recorded,
    /// Sent as this submission (0: already waited for).
    Sent(u64),
}

/// A 16x9 grid of texels of the presented frame, copied into a readback buffer.
pub(crate) struct Sampler {
    buffer: ID3D12Resource,
    texel: Texel,
    pending: Pending,
}

impl Sampler {
    pub(crate) fn new(device: &ID3D12Device) -> Option<Self> {
        let mut buffer: Option<ID3D12Resource> = None;
        // SAFETY: plain creation of a readback buffer.
        unsafe {
            device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() },
                    D3D12_HEAP_FLAG_NONE,
                    &D3D12_RESOURCE_DESC {
                        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                        Width: SLOT * u64::from(GRID_X * GRID_Y),
                        Height: 1,
                        DepthOrArraySize: 1,
                        MipLevels: 1,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                        ..Default::default()
                    },
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut buffer,
                )
                .ok()?;
        }
        Some(Sampler { buffer: buffer?, texel: Texel::Rgba8, pending: Pending::Idle })
    }

    /// Whether a new sample can be recorded.
    pub(crate) fn idle(&self) -> bool {
        matches!(self.pending, Pending::Idle)
    }

    /// Records the grid copies of `source` (in the copy-source state, `width` x `height`, format
    /// `format`) into `list`. `layout` is the format's layout as presented.
    pub(crate) fn record(&mut self, list: &ID3D12GraphicsCommandList, source: &ID3D12Resource, format: DXGI_FORMAT, layout: DXGI_FORMAT) {
        let Some(texel) = Texel::from_dxgi(layout.0 as u32) else { return };
        // SAFETY: a live resource.
        let d = unsafe { source.GetDesc() };
        let (width, height) = (d.Width as u32, d.Height);
        if width < GRID_X || height < GRID_Y {
            return;
        }
        // SAFETY: an open list; locations naming live resources.
        unsafe {
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(source),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            for j in 0..GRID_Y {
                for i in 0..GRID_X {
                    let (x, y) = ((i * 2 + 1) * width / (GRID_X * 2), (j * 2 + 1) * height / (GRID_Y * 2));
                    let to = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&self.buffer),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                                Offset: SLOT * u64::from(j * GRID_X + i),
                                Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: format, Width: 1, Height: 1, Depth: 1, RowPitch: 256 },
                            },
                        },
                    };
                    let b = D3D12_BOX { left: x, top: y, front: 0, right: x + 1, bottom: y + 1, back: 1 };
                    list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&b));
                }
            }
        }
        self.texel = texel;
        self.pending = Pending::Recorded;
    }

    /// The recorded list was submitted as `number` (0: it ran and was waited for).
    pub(crate) fn sent(&mut self, number: u64) {
        if matches!(self.pending, Pending::Recorded) {
            self.pending = Pending::Sent(number);
        }
    }

    /// The mean luminance of the grid, once the submission has finished (`finished` is the last
    /// finished submission).
    pub(crate) fn harvest(&mut self, finished: u64) -> Option<f32> {
        let Pending::Sent(number) = self.pending else { return None };
        if number != 0 && finished < number {
            return None;
        }
        self.pending = Pending::Idle;
        let mut data: *mut c_void = std::ptr::null_mut();
        // SAFETY: the copies finished; the buffer is ours and holds the grid.
        unsafe {
            self.buffer.Map(0, None, Some(&mut data)).ok()?;
            if data.is_null() {
                return None;
            }
            let mut sum = 0.0f32;
            for k in 0..GRID_X * GRID_Y {
                let at = (data as *const u8).add(k as usize * SLOT as usize);
                sum += logic::texel_luma(self.texel, std::slice::from_raw_parts(at, 8));
            }
            self.buffer.Unmap(0, None);
            Some(sum / (GRID_X * GRID_Y) as f32)
        }
    }
}

// the buffer is used from the presenting threads one at a time (the presenter is behind a lock)
unsafe impl Send for Sampler {}
