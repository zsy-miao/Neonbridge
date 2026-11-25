//! The graphics driver's errors, seen as the system records them.
//!
//! A card is not lost out of nothing. Every loss of runs 201, 204, 208 and
//! 224 has entries of the card's driver in the system's event log (NVIDIA:
//! provider `nvlddmkm`, event 153, "Error occurred on GPUID") from 2.7 to
//! 6.3 seconds BEFORE the device was removed, several of them; run 210 has
//! three and kept its device; the runs with no such entry lost nothing.
//! Nothing in the layer saw them: the first it knew was the device gone.
//!
//! Here the layer subscribes to that log. Each entry of a display driver
//! is written to our log when the system records it, with our own clock,
//! so it can be laid beside what the layer did in that moment; it is
//! counted; and the first one trips a switch ([`tripped`]) that the paths
//! under suspicion ask before they run (the draw windows of `crate::draw`
//! turn themselves off for the rest of the run). So a driver error costs a
//! slower frame from then on, and not the card.
//!
//! On by default; `VICTORY_DRIVER_WATCH=0` subscribes to nothing.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use windows_sys::Win32::System::EventLog::{
    EVT_HANDLE, EVT_SUBSCRIBE_NOTIFY_ACTION, EvtSubscribe, EvtSubscribeActionDeliver, EvtSubscribeToFutureEvents,
};

/// Entries of a display driver seen since the start.
static ERRORS: AtomicU64 = AtomicU64::new(0);
/// Whether one was seen ([`tripped`]).
static TRIPPED: AtomicBool = AtomicBool::new(false);

/// Whether the card's driver has reported an error in this run: what could
/// have caused it stops for the rest of the run.
pub(crate) fn tripped() -> bool {
    TRIPPED.load(Relaxed)
}

/// Driver errors seen since the start.
pub(crate) fn errors() -> u64 {
    ERRORS.load(Relaxed)
}

/// An entry of a display driver was recorded.
fn seen() {
    let n = ERRORS.fetch_add(1, Relaxed) + 1;
    let first = !TRIPPED.swap(true, Relaxed);
    tracing::error!(
        n,
        "the graphics driver reported an error to the system's event log{}",
        if first {
            ": from now on the paths under suspicion are off for this run (draw windows clamp each record again)"
        } else {
            ""
        }
    );
}

unsafe extern "system" fn on_event(action: EVT_SUBSCRIBE_NOTIFY_ACTION, _context: *const core::ffi::c_void, _event: EVT_HANDLE) -> u32 {
    if action == EvtSubscribeActionDeliver {
        seen();
    }
    0
}

/// Subscribes to the display drivers' entries in the system's event log,
/// once. Entries from before the call are not delivered.
pub(crate) fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("VICTORY_DRIVER_WATCH").is_some_and(|v| v == "0") {
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
            EvtSubscribe(
                0,
                std::ptr::null_mut(),
                channel.as_ptr(),
                query.as_ptr(),
                0,
                std::ptr::null(),
                Some(on_event),
                EvtSubscribeToFutureEvents,
            )
        };
        if subscription == 0 {
            tracing::warn!(
                error = %std::io::Error::last_os_error(),
                "no subscription to the system's event log: the graphics driver's errors are not seen as they happen"
            );
        } else {
            tracing::info!(
                "the graphics driver's errors in the system's event log are watched: each is logged here as it is recorded, and \
                 the first turns the paths under suspicion off (VICTORY_DRIVER_WATCH=0: not watched)"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_error_trips_the_switch_and_every_one_is_counted() {
        // (Other tests do not touch these statics.)
        assert!(!tripped());
        assert_eq!(errors(), 0);
        seen();
        assert!(tripped());
        seen();
        assert_eq!(errors(), 2);
        assert!(tripped());
    }

    /// End to end, by hand (`--ignored`): an entry written to the
    /// Application log under a name of our own reaches a subscription made
    /// the same way as [`start`]'s, within a second. It leaves one
    /// information entry in that log.
    #[test]
    #[ignore = "writes one entry to the Application event log: run it by name with --ignored"]
    fn an_entry_written_reaches_a_subscription() {
        use windows_sys::Win32::System::EventLog::{
            DeregisterEventSource, EVENTLOG_INFORMATION_TYPE, EvtClose, RegisterEventSourceW, ReportEventW,
        };
        static GOT: AtomicU64 = AtomicU64::new(0);
        unsafe extern "system" fn got(action: EVT_SUBSCRIBE_NOTIFY_ACTION, _: *const core::ffi::c_void, _: EVT_HANDLE) -> u32 {
            if action == EvtSubscribeActionDeliver {
                GOT.fetch_add(1, Relaxed);
            }
            0
        }
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let (channel, query) = (wide("Application"), wide("*[System[Provider[@Name='victory-drvwatch-test']]]"));
        // SAFETY: as in `start`.
        let subscription = unsafe {
            EvtSubscribe(0, std::ptr::null_mut(), channel.as_ptr(), query.as_ptr(), 0, std::ptr::null(), Some(got), EvtSubscribeToFutureEvents)
        };
        assert_ne!(subscription, 0, "{}", std::io::Error::last_os_error());
        let (name, text) = (wide("victory-drvwatch-test"), wide("a test entry of the VIctory layer's driver watch"));
        // SAFETY: a source of our own name, one string, no data.
        unsafe {
            let source = RegisterEventSourceW(std::ptr::null(), name.as_ptr());
            assert!(!source.is_null(), "{}", std::io::Error::last_os_error());
            let strings = [text.as_ptr()];
            assert_ne!(
                ReportEventW(source, EVENTLOG_INFORMATION_TYPE, 0, 1, std::ptr::null_mut(), 1, 0, strings.as_ptr(), std::ptr::null()),
                0,
                "{}",
                std::io::Error::last_os_error()
            );
            DeregisterEventSource(source);
        }
        let started = std::time::Instant::now();
        while GOT.load(Relaxed) == 0 && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let after = started.elapsed();
        // SAFETY: our own subscription.
        unsafe { EvtClose(subscription) };
        assert_eq!(GOT.load(Relaxed), 1, "the entry was not delivered in 5 s");
        eprintln!("delivered after {} ms", after.as_millis());
    }

    /// The subscription itself: the system accepts the channel and the
    /// query (a query it cannot read is refused at once).
    #[test]
    fn the_system_takes_the_subscription() {
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let (channel, query) = (wide("System"), wide("*[System[Provider[@Name='nvlddmkm' or @Name='Display']]]"));
        // SAFETY: as in `start`.
        let subscription = unsafe {
            EvtSubscribe(0, std::ptr::null_mut(), channel.as_ptr(), query.as_ptr(), 0, std::ptr::null(), Some(on_event), EvtSubscribeToFutureEvents)
        };
        assert_ne!(subscription, 0, "{}", std::io::Error::last_os_error());
        // SAFETY: our own subscription.
        unsafe { windows_sys::Win32::System::EventLog::EvtClose(subscription) };
    }
}
