//! The graphics driver's errors, seen as the system records them.
//!
//! VIctory-fxceless a253706 victory-gpu drvwatch.rs (GPL-3.0, credit to its
//! authors), copied with our log and switch names. A card is not lost out of
//! nothing: every loss of fxceless's runs 201, 204, 208 and 224 has entries of
//! the card's driver in the system's event log (NVIDIA: provider `nvlddmkm`,
//! event 153, "Error occurred on GPUID") from 2.7 to 6.3 seconds BEFORE the
//! device was removed; the runs with no such entry lost nothing.
//!
//! Here the layer subscribes to that log. Each entry of a display driver is
//! written to our log when the system records it, with our own clock, so it
//! can be laid beside what the layer did in that moment; it is counted; and
//! the first one trips a switch ([`tripped`]) that paths under suspicion may
//! ask before they run.
//!
//! On by default; `TUXBOX_DRIVER_WATCH=0` subscribes to nothing.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

type EvtHandle = isize;
/// EvtSubscribeActionDeliver.
const DELIVER: u32 = 1;
/// EvtSubscribeToFutureEvents.
const FUTURE: u32 = 1;

#[link(name = "wevtapi")]
unsafe extern "system" {
    fn EvtSubscribe(
        session: EvtHandle,
        signal: *mut core::ffi::c_void,
        channel: *const u16,
        query: *const u16,
        bookmark: EvtHandle,
        context: *const core::ffi::c_void,
        callback: Option<unsafe extern "system" fn(u32, *const core::ffi::c_void, EvtHandle) -> u32>,
        flags: u32,
    ) -> EvtHandle;
}

/// Entries of a display driver seen since the start.
static ERRORS: AtomicU64 = AtomicU64::new(0);
/// Whether one was seen ([`tripped`]).
static TRIPPED: AtomicBool = AtomicBool::new(false);

/// Whether the card's driver has reported an error in this run.
#[allow(dead_code)]
pub(crate) fn tripped() -> bool {
    TRIPPED.load(Relaxed)
}

/// Driver errors seen since the start.
#[allow(dead_code)]
pub(crate) fn errors() -> u64 {
    ERRORS.load(Relaxed)
}

/// An entry of a display driver was recorded.
fn seen() {
    let n = ERRORS.fetch_add(1, Relaxed) + 1;
    let first = !TRIPPED.swap(true, Relaxed);
    crate::diagnostics::log_line(format_args!(
        "driver watch: the graphics driver reported an error to the system's event log ({n} so far){}",
        if first { "; the first one of this run" } else { "" }
    ));
}

unsafe extern "system" fn on_event(action: u32, _context: *const core::ffi::c_void, _event: EvtHandle) -> u32 {
    if action == DELIVER {
        seen();
    }
    0
}

/// Subscribes to the display drivers' entries in the system's event log,
/// once. Entries from before the call are not delivered.
pub(crate) fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if crate::env_toggle!("TUXBOX_DRIVER_WATCH") == Some("0") {
            return;
        }
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let channel = wide("System");
        // The kernel drivers of the three vendors, and the system's own
        // "display driver stopped responding" source.
        let query = wide(
            "*[System[Provider[@Name='nvlddmkm' or @Name='amdkmdag' or @Name='amdwddmg' or @Name='igfx' or @Name='igfxn' \
             or @Name='Display']]]",
        );
        // SAFETY: valid strings for the call's length; the callback is a
        // plain function that touches only statics; the subscription is
        // kept for the life of the process.
        let subscription = unsafe {
            EvtSubscribe(0, std::ptr::null_mut(), channel.as_ptr(), query.as_ptr(), 0, std::ptr::null(), Some(on_event), FUTURE)
        };
        if subscription == 0 {
            crate::diagnostics::log_line(format_args!(
                "driver watch: no subscription to the system's event log ({}): the graphics driver's errors are not seen as they happen",
                std::io::Error::last_os_error()));
        } else {
            crate::diagnostics::log_line(format_args!(
                "driver watch: the graphics driver's errors in the system's event log are watched, each logged as it is recorded (TUXBOX_DRIVER_WATCH=0: not watched)"));
        }
    });
}
