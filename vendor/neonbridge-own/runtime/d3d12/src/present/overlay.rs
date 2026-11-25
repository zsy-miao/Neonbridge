//! FPS counter in the top-right corner of the output window.
//! A separate click-through layered popup on its own thread: it never touches
//! the swap chain or the title's window, so it costs the render path one
//! atomic add per present. TUXBOX_FPS=0 turns it off.
use std::sync::Once;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

static PRESENTS: AtomicU64 = AtomicU64::new(0);
static TARGET: AtomicUsize = AtomicUsize::new(0);
static STARTED: Once = Once::new();
const WIDTH: i32 = 110;
const HEIGHT: i32 = 26;
const MARGIN: i32 = 8;

/// One completed present.
pub(super) fn frame() {
    PRESENTS.fetch_add(1, Ordering::Relaxed);
}

/// Follow `target` (the swap chain's window); the first call starts the overlay.
pub(super) fn attach(target: HWND) {
    TARGET.store(target.0 as usize, Ordering::Relaxed);
    if std::env::var("TUXBOX_FPS").is_ok_and(|v| v == "0") {
        return;
    }
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("TuxBox fps".into())
            .spawn(run);
    });
}

struct Rate {
    text: Vec<u16>,
    last: Instant,
    count: u64,
}

static mut RATE: Option<Rate> = None;

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: dispatched only on the overlay thread, which alone touches RATE.
    unsafe {
        match msg {
            WM_TIMER => {
                tick(hwnd);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut rect = RECT { left: 0, top: 0, right: WIDTH, bottom: HEIGHT };
                let back = CreateSolidBrush(COLORREF(0x0010_1010));
                FillRect(dc, &rect, back);
                let _ = DeleteObject(back.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x0040_FF40));
                let font = GetStockObject(ANSI_FIXED_FONT);
                let old = SelectObject(dc, font);
                #[allow(static_mut_refs)]
                // Never hand DrawTextW an empty slice: its pointer is dangling, and a
                // fault inside a window procedure ends the process (0xc000041d, test7).
                if let Some(rate) = RATE.as_mut().filter(|r| !r.text.is_empty()) {
                    DrawTextW(dc, &mut rate.text, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                }
                SelectObject(dc, old);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

unsafe fn tick(hwnd: HWND) {
    // SAFETY: overlay thread only; target is an opaque HWND checked by IsWindow.
    unsafe {
        #[allow(static_mut_refs)]
        let Some(rate) = RATE.as_mut() else { return };
        let now = Instant::now();
        let count = PRESENTS.load(Ordering::Relaxed);
        let secs = now.duration_since(rate.last).as_secs_f64();
        if secs >= 0.5 {
            let fps = (count - rate.count) as f64 / secs;
            rate.text = format!("{fps:.1} fps").encode_utf16().collect();
            rate.last = now;
            rate.count = count;
            let _ = InvalidateRect(Some(hwnd), None, true);
        }
        let target = HWND(TARGET.load(Ordering::Relaxed) as _);
        let mut client = RECT::default();
        let mut corner = POINT::default();
        // Show only over our own foreground, unminimized output window.
        let visible = IsWindow(Some(target)).as_bool()
            && !IsIconic(target).as_bool()
            && GetForegroundWindow() == target
            && GetClientRect(target, &mut client).is_ok()
            && { corner.x = client.right; ClientToScreen(target, &mut corner).as_bool() };
        if visible {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                corner.x - WIDTH - MARGIN,
                corner.y + MARGIN,
                WIDTH,
                HEIGHT,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn run() {
    // SAFETY: the class, window and timer live and die on this thread.
    unsafe {
        #[allow(static_mut_refs)]
        {
            RATE = Some(Rate { text: "-- fps".encode_utf16().collect(), last: Instant::now(), count: PRESENTS.load(Ordering::Relaxed) });
        }
        let Ok(module) = GetModuleHandleW(None) else { return };
        let instance = HINSTANCE(module.0);
        let class = w!("TuxBox.Fps");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: HCURSOR::default(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return;
        }
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class,
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            WIDTH,
            HEIGHT,
            None,
            None,
            Some(instance),
            None,
        ) else {
            return;
        };
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 200, LWA_ALPHA);
        SetTimer(Some(hwnd), 1, 250, None);
        crate::diagnostics::log_line(format_args!("fps overlay on (TUXBOX_FPS=0 turns it off)"));
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            DispatchMessageW(&message);
        }
    }
}
