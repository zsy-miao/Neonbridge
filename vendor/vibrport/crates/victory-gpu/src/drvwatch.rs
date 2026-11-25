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
//! The system's own display source (`Display`) counts only for its
//! warnings and errors ("display driver stopped responding and has
//! recovered", 4101, is a warning): Windows also writes Auto HDR's notices
//! there as information entries (4125, 4127), and those say nothing of the
//! driver.
//!
//! On by default; `VICTORY_DRIVER_WATCH=0` subscribes to nothing.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use windows_sys::Win32::System::EventLog::{
    EVT_HANDLE, EVT_SUBSCRIBE_NOTIFY_ACTION, EvtSubscribe, EvtSubscribeActionDeliver, EvtSubscribeToFutureEvents,
};

/// The kernel drivers of the three vendors: every entry of theirs counts.
const DRIVERS: [&str; 5] = ["nvlddmkm", "amdkmdag", "amdwddmg", "igfx", "igfxn"];

/// The subscription's query: every entry of `drivers`, and the entries of
/// `system` from warning level up (critical 1, error 2, warning 3; its
/// information entries, level 4, are left out).
fn query(drivers: &[&str], system: &str) -> String {
    let drivers = drivers.iter().map(|d| format!("@Name='{d}'")).collect::<Vec<_>>().join(" or ");
    format!("*[System[Provider[{drivers}] or (Provider[@Name='{system}'] and (Level=1 or Level=2 or Level=3))]]")
}

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
        // The kernel drivers of the three vendors, and the warnings and
        // errors of the system's own "display driver stopped responding"
        // source.
        let query = wide(&query(&DRIVERS, "Display"));
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
        let (channel, query) = (wide("System"), wide(&query(&DRIVERS, "Display")));
        // SAFETY: as in `start`.
        let subscription = unsafe {
            EvtSubscribe(0, std::ptr::null_mut(), channel.as_ptr(), query.as_ptr(), 0, std::ptr::null(), Some(on_event), EvtSubscribeToFutureEvents)
        };
        assert_ne!(subscription, 0, "{}", std::io::Error::last_os_error());
        // SAFETY: our own subscription.
        unsafe { windows_sys::Win32::System::EventLog::EvtClose(subscription) };
    }

    /// End to end, by hand (`--ignored`): [`query`] read the way [`start`]
    /// reads it, with a name of our own in the Application log standing for
    /// the system's display source, then for a vendor's driver. Of an
    /// information entry and an error entry written under it, the display
    /// source passes the error alone (an Auto HDR notice is an information
    /// entry of `Display`, fxceless/victory-fork#1077); a driver passes both.
    /// It leaves two entries in that log.
    #[test]
    #[ignore = "writes two entries to the Application event log: run it by name with --ignored"]
    fn the_display_source_counts_from_warnings_up_and_a_driver_always() {
        use windows_sys::Win32::System::EventLog::{
            DeregisterEventSource, EVENTLOG_ERROR_TYPE, EVENTLOG_INFORMATION_TYPE, EvtClose, RegisterEventSourceW, ReportEventW,
        };
        type Callback = unsafe extern "system" fn(EVT_SUBSCRIBE_NOTIFY_ACTION, *const core::ffi::c_void, EVT_HANDLE) -> u32;
        static AS_DISPLAY: AtomicU64 = AtomicU64::new(0);
        static AS_DRIVER: AtomicU64 = AtomicU64::new(0);
        unsafe extern "system" fn display(action: EVT_SUBSCRIBE_NOTIFY_ACTION, _: *const core::ffi::c_void, _: EVT_HANDLE) -> u32 {
            if action == EvtSubscribeActionDeliver {
                AS_DISPLAY.fetch_add(1, Relaxed);
            }
            0
        }
        unsafe extern "system" fn driver(action: EVT_SUBSCRIBE_NOTIFY_ACTION, _: *const core::ffi::c_void, _: EVT_HANDLE) -> u32 {
            if action == EvtSubscribeActionDeliver {
                AS_DRIVER.fetch_add(1, Relaxed);
            }
            0
        }
        const NAME: &str = "victory-drvwatch-level-test";
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let channel = wide("Application");
        let subscribe = |query: &str, callback: Callback| {
            let query = wide(query);
            // SAFETY: as in `start`.
            let subscription = unsafe {
                EvtSubscribe(0, std::ptr::null_mut(), channel.as_ptr(), query.as_ptr(), 0, std::ptr::null(), Some(callback), EvtSubscribeToFutureEvents)
            };
            assert_ne!(subscription, 0, "{}", std::io::Error::last_os_error());
            subscription
        };
        let subscriptions = [
            subscribe(&query(&["victory-drvwatch-no-driver"], NAME), display),
            subscribe(&query(&[NAME], "victory-drvwatch-no-display"), driver),
        ];
        let name = wide(NAME);
        // SAFETY: a source of our own name, one string per entry, no data.
        unsafe {
            let source = RegisterEventSourceW(std::ptr::null(), name.as_ptr());
            assert!(!source.is_null(), "{}", std::io::Error::last_os_error());
            for (kind, text) in [
                (EVENTLOG_INFORMATION_TYPE, "an information entry of the VIctory layer's driver watch test"),
                (EVENTLOG_ERROR_TYPE, "an error entry of the VIctory layer's driver watch test"),
            ] {
                let text = wide(text);
                let strings = [text.as_ptr()];
                assert_ne!(
                    ReportEventW(source, kind, 0, 1, std::ptr::null_mut(), 1, 0, strings.as_ptr(), std::ptr::null()),
                    0,
                    "{}",
                    std::io::Error::last_os_error()
                );
            }
            DeregisterEventSource(source);
        }
        let started = std::time::Instant::now();
        while AS_DRIVER.load(Relaxed) < 2 && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // What the display source would wrongly pass arrives with them.
        std::thread::sleep(std::time::Duration::from_millis(500));
        for subscription in subscriptions {
            // SAFETY: our own subscription.
            unsafe { EvtClose(subscription) };
        }
        assert_eq!(AS_DRIVER.load(Relaxed), 2, "a driver's entries: both, whatever their level");
        assert_eq!(AS_DISPLAY.load(Relaxed), 1, "the display source's entries: the error alone");
    }
}
