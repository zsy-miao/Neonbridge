//! Fine timer resolution for the title process.
//!
//! The title's audio mixer sleeps about 4 ms per 5.3 ms block. On Windows'
//! default 15.6 ms tick every such sleep rounds up, so audio runs at roughly a
//! third of its speed (gaps, delay, crackle). The console's tick is fine, so the
//! process asks for the finest resolution the system offers (about 0.5 ms via
//! NtSetTimerResolution) plus timeBeginPeriod(1), and opts out of Windows 11's
//! power throttling that ignores timer requests from hidden windows.
//! TUXBOX_FINE_TIMER=0 leaves the default tick.

use std::ffi::c_void;

unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    fn GetCurrentProcess() -> *mut c_void;
    fn SetProcessInformation(process: *mut c_void, class: i32, info: *const c_void, size: u32) -> i32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
}

/// PROCESS_POWER_THROTTLING_STATE (ProcessPowerThrottling = 4).
#[repr(C)]
struct Throttling {
    version: u32,
    control_mask: u32,
    state_mask: u32,
}
const PROCESS_POWER_THROTTLING: i32 = 4;
const IGNORE_TIMER_RESOLUTION: u32 = 0x4;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Called from xmem's DllMain (process attach). Only ntdll and kernel32 calls
/// run under the loader lock; timeBeginPeriod loads winmm, so it runs on a
/// thread that starts once the lock is released.
pub(crate) unsafe fn install() {
    if std::env::var("TUXBOX_FINE_TIMER").is_ok_and(|v| v.trim() == "0") {
        crate::loader::log("timer: TUXBOX_FINE_TIMER=0, keeping the default tick");
        return;
    }
    unsafe {
        // Keep the request honoured while the window is hidden or minimised.
        let state = Throttling { version: 1, control_mask: IGNORE_TIMER_RESOLUTION, state_mask: 0 };
        let opted_out = SetProcessInformation(GetCurrentProcess(), PROCESS_POWER_THROTTLING, (&state as *const Throttling).cast(), size_of::<Throttling>() as u32) != 0;

        // NtQueryTimerResolution / NtSetTimerResolution work in 100 ns units.
        let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
        let query = GetProcAddress(ntdll, b"NtQueryTimerResolution\0".as_ptr());
        let set = GetProcAddress(ntdll, b"NtSetTimerResolution\0".as_ptr());
        let mut granted = None;
        if !query.is_null() && !set.is_null() {
            let query: unsafe extern "system" fn(*mut u32, *mut u32, *mut u32) -> i32 = std::mem::transmute(query);
            let set: unsafe extern "system" fn(u32, u8, *mut u32) -> i32 = std::mem::transmute(set);
            let (mut coarsest, mut finest, mut current) = (0u32, 0u32, 0u32);
            if query(&mut coarsest, &mut finest, &mut current) >= 0 {
                let mut actual = 0u32;
                if set(finest.max(5000).min(10000), 1, &mut actual) >= 0 {
                    granted = Some((current, actual));
                }
            }
        }
        match granted {
            Some((before, after)) => crate::loader::log(&format!(
                "timer: resolution {:.3} ms -> {:.3} ms (power throttling opt-out {}); TUXBOX_FINE_TIMER=0 keeps the default",
                f64::from(before) / 10_000.0, f64::from(after) / 10_000.0, if opted_out { "set" } else { "unavailable" })),
            None => crate::loader::log("timer: NtSetTimerResolution unavailable; relying on timeBeginPeriod(1)"),
        }
    }
    let _ = std::thread::Builder::new().name("tuxbox timer period".into()).spawn(|| unsafe {
        let winmm = LoadLibraryW(wide("winmm.dll").as_ptr());
        if winmm.is_null() {
            crate::loader::log("timer: winmm.dll not loaded; timeBeginPeriod skipped");
            return;
        }
        let begin = GetProcAddress(winmm, b"timeBeginPeriod\0".as_ptr());
        if begin.is_null() {
            return;
        }
        let begin: unsafe extern "system" fn(u32) -> u32 = std::mem::transmute(begin);
        let status = begin(1);
        crate::loader::log(&format!("timer: timeBeginPeriod(1) = {status}"));
    });
}
