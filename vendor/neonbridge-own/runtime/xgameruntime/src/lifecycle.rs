//! TUXBOX_PLM (default on, =0 the OS's own appnotify calls and no window
//! hook, as before): the title's life cycle. A console title is never told it
//! is closing: quitting it first suspends it (its suspend routines run, and
//! it has a second to save) and then ends the process. Closing the title's
//! window does the same here.
//!
//! Copied from ztrx victory-hle lifecycle.rs:21-60,165-230,288-294 (GPL-3.0,
//! credit to its authors). Differences: we do not hook CreateWindowEx (xmem
//! owns the title's imports), so a watcher thread finds the process's
//! top-level windows and subclasses them; the leader's screen fit and
//! launcher relaunch are not ported; `end` releases our PLS drive letter
//! instead of saving the leader's pipeline cache. With the switch off the
//! exports forward to kernel.appcore.dll (the OS's appnotify), which is what
//! the title reached before.
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;
use windows_sys::core::{PCWSTR, w};

type Routine = extern "system" fn(quiesced: u8, context: *mut c_void);

fn on() -> bool {
    std::env::var("TUXBOX_PLM").map_or(true, |v| v.trim() != "0")
}

/// Registration id, routine and context of each suspend routine.
static ROUTINES: Mutex<Vec<(usize, Routine, usize)>> = Mutex::new(Vec::new());

fn routines() -> MutexGuard<'static, Vec<(usize, Routine, usize)>> {
    ROUTINES.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long the console lets suspend routines run before it ends the title.
const SUSPEND_DEADLINE: Duration = Duration::from_secs(1);

const ERROR_INVALID_PARAMETER: u32 = 87;

/// The OS's own export, for the switch off.
fn appcore(name: &core::ffi::CStr) -> Option<usize> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let module = unsafe { LoadLibraryW(w!("kernel.appcore.dll")) };
    if module.is_null() { return None; }
    unsafe { GetProcAddress(module, name.as_ptr().cast()) }.map(|f| f as usize)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterAppStateChangeNotification(
    routine: Option<Routine>,
    context: *mut c_void,
    registration: *mut usize,
) -> u32 {
    if !on() {
        type Os = unsafe extern "system" fn(Option<Routine>, *mut c_void, *mut usize) -> u32;
        return match appcore(c"RegisterAppStateChangeNotification") {
            Some(f) => unsafe { core::mem::transmute::<usize, Os>(f)(routine, context, registration) },
            None => 120, // ERROR_CALL_NOT_IMPLEMENTED
        };
    }
    let Some(routine) = routine else {
        return ERROR_INVALID_PARAMETER;
    };
    if registration.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    routines().push((id, routine, context as usize));
    // SAFETY: checked non-null; the title passes room for its registration.
    unsafe { registration.write(id) };
    watch_windows();
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterAppStateChangeNotification(registration: usize) {
    if !on() {
        type Os = unsafe extern "system" fn(usize);
        if let Some(f) = appcore(c"UnregisterAppStateChangeNotification") {
            unsafe { core::mem::transmute::<usize, Os>(f)(registration) };
        }
        return;
    }
    routines().retain(|&(id, ..)| id != registration);
}

/// The window procedure a title window had before ours.
const TITLE_PROC: PCWSTR = w!("tuxbox.wndproc");

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
}

/// Our stand-in for the leader's CreateWindowEx hook: a thread that puts
/// our procedure in front of each top-level window of this process.
pub(super) fn watch_windows() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !on() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> i32 {
            let mut pid = 0;
            unsafe { wm::GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid == unsafe { GetCurrentProcessId() }
                && unsafe { wm::GetWindow(hwnd, wm::GW_OWNER) }.is_null()
                && unsafe { wm::GetPropW(hwnd, TITLE_PROC) }.is_null()
            {
                hook(hwnd);
            }
            1
        }
        unsafe { wm::EnumWindows(Some(each), 0) };
        std::thread::sleep(Duration::from_millis(250));
    });
}

/// Puts our procedure in front of the title's.
fn hook(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    // SAFETY: a window of this process. The title's procedure is stored
    // before ours can run.
    unsafe {
        let title = wm::GetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC);
        if title == 0 { return; }
        wm::SetPropW(hwnd, TITLE_PROC, title as *mut c_void);
        wm::SetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC, window_proc as *const () as isize);
    }
}

extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // A user close (close button, Alt+F4, window or taskbar menu); a
    // console title never sees one, nor a WM_CLOSE from it.
    if message == wm::WM_SYSCOMMAND && wparam & 0xFFF0 == wm::SC_CLOSE as usize {
        end_title("window closed");
        return 0;
    }
    // Alt or F10 puts a window into menu mode: the system runs a modal menu
    // loop on the thread that owns the window, which is the title's. The
    // title then stops (no frames, no input) until a key ends the loop. A
    // console has no window menu. The mouse still opens it from the title bar.
    if message == wm::WM_SYSCOMMAND && wparam & 0xFFF0 == wm::SC_KEYMENU as usize {
        return 0;
    }
    // SAFETY: the procedure `hook` stored, called the way the system would.
    unsafe {
        let title = wm::GetPropW(hwnd, TITLE_PROC);
        if message == wm::WM_NCDESTROY {
            wm::RemovePropW(hwnd, TITLE_PROC);
        }
        let title = std::mem::transmute::<*mut c_void, wm::WNDPROC>(title);
        wm::CallWindowProcW(title, hwnd, message, wparam, lparam)
    }
}

/// Suspends the title as the console does when it ends it, then ends the
/// process. Returns at once; later calls do nothing.
fn end_title(why: &'static str) {
    static ENDING: AtomicBool = AtomicBool::new(false);
    if ENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let suspend = routines().clone();
    super::report(format_args!("{why}: suspending the title ({} routines)", suspend.len()));
    // Whoever comes first (the routines or the deadline) ends the process;
    // the other waits for it, so the process never ends halfway through.
    let once = std::sync::Arc::new(Mutex::new(()));
    let finish = move |how: &str| {
        let _guard = once.lock().unwrap_or_else(|e| e.into_inner());
        super::report(format_args!("{how}; ending the title"));
        end();
    };
    let late = finish.clone();
    // Not on the calling thread: titles hand the suspend to their window
    // thread and wait for it inside the routine.
    std::thread::spawn(move || {
        for (_, routine, context) in suspend {
            routine(1, context as *mut c_void);
        }
        finish("title suspended");
    });
    std::thread::spawn(move || {
        std::thread::sleep(SUSPEND_DEADLINE);
        late("title did not finish suspending in time");
    });
}

fn end() {
    // TerminateProcess skips atexit, so the PLS drive letter goes here.
    super::services::release_storage_drive();
    // SAFETY: ends this process, as the console ends a suspended title.
    unsafe { TerminateProcess(GetCurrentProcess(), 0) };
}
