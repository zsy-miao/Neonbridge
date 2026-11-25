//! The displays stay on while the game runs.
//!
//! Cause (test121-130): with every monitor powered off, DWM paces flip-model
//! presents from a slow fallback clock (about 10 Hz). DXGI Present then
//! blocks ~92 ms, the GPU sits idle behind it, and the Bink loading movie
//! breaks up. test130, with the monitors woken, presented in ~4 ms. The user
//! watches over the feed while away, so the launcher must keep them on:
//!
//! - While the game runs, the launcher's main thread holds
//!   ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED, as a game or video
//!   player does, and releases it when the game exits (NEONBRIDGE_KEEP_AWAKE=0
//!   off). That keeps an awake display on but does not wake one already off
//!   (test128).
//! - So the console display state is watched too (GUID_CONSOLE_DISPLAY_STATE):
//!   when it is off at launch or turns off during the run, one zero-delta
//!   mouse input wakes it (NEONBRIDGE_WAKE_DISPLAY=0 off). Both are logged.
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::System::Power::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::core::GUID;

/// GUID_CONSOLE_DISPLAY_STATE: Data is 0 off, 1 on, 2 dimmed.
const CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);
const DEVICE_NOTIFY_CALLBACK: u32 = 2;
const PBT_POWERSETTINGCHANGE: u32 = 0x8013;
static WAKES: AtomicU32 = AtomicU32::new(0);

fn off(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v.trim() == "0")
}

fn state_name(state: u32) -> &'static str {
    match state {
        0 => "off",
        1 => "on",
        2 => "dimmed",
        _ => "unknown",
    }
}

/// One zero-delta mouse move: counts as user input, so the displays wake.
fn wake(why: &str) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } },
    };
    // SAFETY: one well-formed INPUT of its own size.
    let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
    let n = WAKES.fetch_add(1, Ordering::Relaxed) + 1;
    println!("[launch] display: {why}; woke it with a zero-delta mouse input (sent {sent}, {n} so far; NEONBRIDGE_WAKE_DISPLAY=0 off)");
}

unsafe extern "system" fn display_changed(_: *const core::ffi::c_void, kind: u32, setting: *const core::ffi::c_void) -> u32 {
    if kind != PBT_POWERSETTINGCHANGE || setting.is_null() {
        return 0;
    }
    // SAFETY: the system passes a POWERBROADCAST_SETTING with a 4-byte Data for this GUID.
    let setting = unsafe { &*setting.cast::<POWERBROADCAST_SETTING>() };
    if setting.DataLength < 4 {
        return 0;
    }
    let state = unsafe { setting.Data.as_ptr().cast::<u32>().read_unaligned() };
    static FIRST: std::sync::Once = std::sync::Once::new();
    let mut first = false;
    FIRST.call_once(|| first = true);
    println!("[launch] display: console display {}{}", state_name(state), if first { " at launch" } else { "" });
    if state == 0 && !off("NEONBRIDGE_WAKE_DISPLAY") {
        let why = if first { "the displays were off at launch" } else { "the displays turned off during the run" };
        // Off the notification thread.
        std::thread::spawn(move || wake(why));
    }
    0
}

/// Held for the game's run on the thread that waits for it.
pub(crate) struct Awake {
    held: bool,
    notify: *mut core::ffi::c_void,
}

pub(crate) fn hold() -> Awake {
    let held = !off("NEONBRIDGE_KEEP_AWAKE") && unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED) != 0
    };
    if held {
        println!("[launch] display: kept on while the game runs (ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED; NEONBRIDGE_KEEP_AWAKE=0 off)");
    }
    let mut notify: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut params = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(display_changed), Context: core::ptr::null_mut() };
    // SAFETY: params outlives the call (the system copies it); notify receives the handle.
    let status = unsafe {
        PowerSettingRegisterNotification(&CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_CALLBACK, (&raw mut params).cast(), &mut notify)
    };
    if status != 0 {
        println!("[launch] display: state not watched (PowerSettingRegisterNotification {status}); sleeping displays are not woken");
        notify = core::ptr::null_mut();
    }
    Awake { held, notify }
}

impl Drop for Awake {
    fn drop(&mut self) {
        if !self.notify.is_null() {
            unsafe { PowerSettingUnregisterNotification(self.notify as HPOWERNOTIFY) };
        }
        if self.held {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            println!("[launch] display: keep-on released (game exited; {} wake(s))", WAKES.load(Ordering::Relaxed));
        }
    }
}
