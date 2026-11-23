//! A console title can present without creating a desktop window.
//! The host owns and pumps its window on a dedicated UI thread.
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::{self, JoinHandle};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error, PCWSTR, Result};

static OUTPUT: Mutex<Weak<OwnedWindow>> = Mutex::new(Weak::new());

pub(super) struct OutputWindow {
    handle: usize,
    // Existing desktop titles keep ownership of their own window.
    _owner: Option<Arc<OwnedWindow>>,
}

impl OutputWindow {
    pub(super) fn acquire(width: u32, height: u32) -> Result<Self> {
        let mut shared = OUTPUT.lock().map_err(|_| E_FAIL)?;
        if let Some(window) = shared.upgrade() {
            return Ok(Self {
                handle: window.handle,
                _owner: Some(window),
            });
        }
        // The render thread often has no active HWND. Frosty's main thread
        // already owns a top-level window and receives the game's input there.
        let active = unsafe { GetActiveWindow() };
        let title = if !active.is_invalid() && unsafe { IsWindowVisible(active) }.as_bool() {
            active
        } else {
            title_window()
        };
        if !title.is_invalid() {
            fit_title_window(title, width, height)?;
            return Ok(Self {
                handle: title.0 as usize,
                _owner: None,
            });
        }
        let owned = Arc::new(OwnedWindow::new(width, height)?);
        *shared = Arc::downgrade(&owned);
        Ok(Self {
            handle: owned.handle,
            _owner: Some(owned),
        })
    }

    pub(super) fn hwnd(&self) -> HWND {
        HWND(self.handle as _)
    }

    pub(super) fn is_open(&self) -> bool {
        unsafe { IsWindow(Some(self.hwnd())).as_bool() }
    }
}

fn title_window() -> HWND {
    unsafe extern "system" fn visit(hwnd: HWND, parameter: LPARAM) -> BOOL {
        let mut pid = 0;
        let mut bounds = RECT::default();
        // SAFETY: EnumWindows supplies a top-level HWND and invokes the
        // callback synchronously with our live candidate below.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == GetCurrentProcessId()
                && IsWindowVisible(hwnd).as_bool()
                && GetWindow(hwnd, GW_OWNER).is_err()
                && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW.0 as isize == 0
                && GetClientRect(hwnd, &mut bounds).is_ok()
            {
                let area = i64::from(bounds.right) * i64::from(bounds.bottom);
                let best = &mut *(parameter.0 as *mut (HWND, i64));
                if area > best.1 {
                    *best = (hwnd, area);
                }
            }
        }
        true.into()
    }
    let mut best = (HWND::default(), 0i64);
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(core::ptr::from_mut(&mut best) as isize));
    }
    best.0
}

fn monitor_info(monitor: HMONITOR) -> Result<MONITORINFO> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: info is a sized writable MONITORINFO for this Win32 monitor.
    unsafe { GetMonitorInfoW(monitor, &mut info).ok()? };
    Ok(info)
}

fn fitted_window(
    width: u32,
    height: u32,
    area: RECT,
    style: WINDOW_STYLE,
    extended: WINDOW_EX_STYLE,
    menu: bool,
) -> Result<RECT> {
    // The monitor work area includes room for the window frame. DXGI scales
    // the original back buffer into the resulting client area.
    let mut frame = RECT::default();
    unsafe { AdjustWindowRectEx(&mut frame, style, menu, extended)? };
    let border_w = frame.right - frame.left;
    let border_h = frame.bottom - frame.top;
    let available_w = i64::from(area.right - area.left - border_w);
    let available_h = i64::from(area.bottom - area.top - border_h);
    let (w, h) = (i64::from(width), i64::from(height));
    if w == 0 || h == 0 || available_w <= 0 || available_h <= 0 {
        return Err(E_INVALIDARG.into());
    }
    let client_w = w.min(available_w).min(available_h * w / h).max(1);
    let client_h = (client_w * h / w).max(1);
    let outer_w = client_w as i32 + border_w;
    let outer_h = client_h as i32 + border_h;
    let left = area.left + (area.right - area.left - outer_w) / 2;
    let top = area.top + (area.bottom - area.top - outer_h) / 2;
    Ok(RECT {
        left,
        top,
        right: left + outer_w,
        bottom: top + outer_h,
    })
}

fn fit_title_window(hwnd: HWND, width: u32, height: u32) -> Result<()> {
    // SAFETY: HWND is the selected visible title window. Asynchronous sizing
    // leaves its UI thread free to consume the event outside this render call.
    unsafe {
        let info = monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST))?;
        let work = info.rcWork;
        let mut current = RECT::default();
        GetWindowRect(hwnd, &mut current)?;
        if current == info.rcMonitor
            || (current.left >= work.left
                && current.top >= work.top
                && current.right <= work.right
                && current.bottom <= work.bottom)
        {
            return Ok(());
        }
        let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
        let extended = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        let bounds = fitted_window(
            width,
            height,
            work,
            style,
            extended,
            !GetMenu(hwnd).is_invalid(),
        )?;
        SetWindowPos(
            hwnd,
            None,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )
    }
}

struct OwnedWindow {
    handle: usize,
    thread: Option<JoinHandle<()>>,
}

impl OwnedWindow {
    fn new(width: u32, height: u32) -> Result<Self> {
        let (ready, receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("TuxBox display".into())
            .spawn(move || {
                if let Err(e) = run_window(width, height, &ready) {
                    let _ = ready.send(Err(e));
                }
            })
            .map_err(|_| E_OUTOFMEMORY)?;
        let handle = match receiver.recv().unwrap_or_else(|_| Err(E_FAIL.into())) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = thread.join();
                return Err(error);
            }
        };
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

impl Drop for OwnedWindow {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            if !thread.is_finished() {
                // SAFETY: Only our window class handles this private shutdown message.
                let _ = unsafe {
                    PostMessageW(Some(HWND(self.handle as _)), WM_APP, WPARAM(0), LPARAM(0))
                };
            }
            let _ = thread.join();
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: The system dispatches this procedure only on the window's owner thread.
    unsafe {
        match msg {
            WM_APP => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_SETCURSOR if l.0 as u16 == HTCLIENT as u16 => {
                SetCursor(None);
                LRESULT(1)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

fn run_window(width: u32, height: u32, ready: &mpsc::SyncSender<Result<usize>>) -> Result<()> {
    let exe = std::env::current_exe().ok();
    let title = exe
        .as_deref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("TuxBox");
    let title: Vec<_> = title.encode_utf16().chain([0]).collect();
    // SAFETY: The class name and callback remain valid until this thread
    // destroys its window and unregisters its class.
    unsafe {
        let class: Vec<_> = format!("TuxBox.PresentX.{}", GetCurrentThreadId())
            .encode_utf16()
            .chain([0])
            .collect();
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let name = PCWSTR(class.as_ptr());
        let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTONEAREST);
        let area = monitor_info(monitor)?.rcWork;
        let bounds = fitted_window(
            width,
            height,
            area,
            WS_OVERLAPPEDWINDOW,
            WINDOW_EX_STYLE(0),
            false,
        )?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: name,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(Error::from_win32());
        }
        let result = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            name,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            None,
            None,
            Some(instance),
            None,
        );
        if let Ok(hwnd) = &result {
            let _ = ShowWindow(*hwnd, SW_SHOWNORMAL);
            if ready.send(Ok(hwnd.0 as usize)).is_ok() {
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            if IsWindow(Some(*hwnd)).as_bool() {
                let _ = DestroyWindow(*hwnd);
            }
        }
        let _ = UnregisterClassW(name, Some(instance));
        result.map(|_| ())
    }
}
