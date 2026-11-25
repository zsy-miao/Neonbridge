//! Frame pacing: the title sets a frame interval and waits for each frame
//! to begin. Ours is a clock started by the first interval set, ticking
//! once per interval; the token is the frame number. Each event kind
//! (origin, vblank, flip) happens once per tick, so two waits for the same
//! kind never end on the same tick: the title polls the origin
//! with "token on time-out" (starting its next frame at once) and is paced
//! by a blocking wait for the flip before each present.
//!
//! shortcut: event offsets are ignored (every event is at the tick); tie
//! the clock to presentation if a title paces by them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::object::{HRESULT, S_FALSE, S_OK, This};

/// What a wait returns when its time is up first.
const TIMEOUT: HRESULT = S_FALSE;
/// The frame-origin event: the one that starts frames and makes tokens.
const ORIGIN: u32 = 0;
/// The flip event: the title waits for it before each present.
const FLIP: u32 = 2;
/// Whether a flip wait has ended since the last present (see
/// [`before_present`]).
static FLIPPED: AtomicBool = AtomicBool::new(true);
/// Make the frame's token even if the wait times out (the title starts the
/// frame early).
const TOKEN_ON_TIMEOUT: u32 = 2;
const SIXTIETH: Duration = Duration::from_micros(16_667);

struct Clock {
    start: Instant,
    interval: Duration,
    /// The tick number at `start` (ticks go on across interval changes).
    first: u64,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);
static FRAME: AtomicU64 = AtomicU64::new(0);

fn clock() -> std::sync::MutexGuard<'static, Option<Clock>> {
    CLOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Device method: a frame every `length` microseconds. The period is how
/// many intervals a frame may take to reach the display (pipeline depth),
/// not part of the rate.
pub(crate) extern "system" fn set_interval(
    _: This,
    _output: This,
    length: u32,
    _period: u32,
    _flags: u32,
) -> HRESULT {
    let interval = match length {
        0 => SIXTIETH,
        us => Duration::from_micros(u64::from(us)),
    };
    tracing::info!(?interval, "frame interval");
    // A new interval starts from now, after the last tick handed out.
    let first = LAST
        .iter()
        .map(|t| t.load(Ordering::Relaxed))
        .max()
        .unwrap_or(0);
    *clock() = Some(Clock {
        start: Instant::now(),
        interval,
        first,
    });
    S_OK
}

/// Device method: events are scheduled at every tick.
pub(crate) extern "system" fn schedule(
    _: This,
    _kind: u32,
    _offset: u32,
    _list: *const u8,
    _flags: u32,
) -> HRESULT {
    S_OK
}

/// Frames begun so far (frame waits that ended).
pub(crate) fn current() -> u64 {
    FRAME.load(Ordering::Relaxed)
}

/// Per event kind (origin, vblank, flip and later kinds): the last tick a
/// wait for it ended on.
static LAST: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

fn last(kind: u32) -> &'static AtomicU64 {
    &LAST[(kind as usize).min(LAST.len() - 1)]
}

/// The next tick after now and after the last one a wait for `kind` ended
/// on (an event happens once per tick: a wait woken a little early must not
/// let the next one end on the same tick), and how long until it.
fn next_tick(kind: u32) -> (u64, Duration) {
    let mut clock = clock();
    let c = clock.get_or_insert_with(|| Clock {
        start: Instant::now(),
        interval: SIXTIETH,
        first: 0,
    });
    let elapsed = c.start.elapsed().as_nanos();
    let interval = c.interval.as_nanos().max(1);
    let tick =
        (c.first + (elapsed / interval + 1) as u64).max(last(kind).load(Ordering::Relaxed) + 1);
    let at = u128::from(tick - c.first) * interval;
    (
        tick,
        Duration::from_nanos(at.saturating_sub(elapsed) as u64),
    )
}

/// Device method: blocks until the next tick (or the time-out); an origin
/// wait then hands out the new frame's token. Other events keep the token
/// the title passes. Ticks are absolute (start plus whole intervals), so
/// lateness never accumulates.
pub(crate) extern "system" fn wait(
    _: This,
    kind: u32,
    timeout_ms: u32,
    _list: *const u8,
    flags: u32,
    token: *mut u64,
) -> HRESULT {
    let origin = kind == ORIGIN;
    let started = Instant::now();
    let limit = Duration::from_millis(u64::from(timeout_ms));
    let deadline = started.checked_add(limit);
    // A blocking wait for an event that has happened since the last wait
    // for it ended (a frame took longer than a tick) ends at once, on the
    // tick that passed: sleeping on to the next one cost 17 ms of every
    // 730 ms frame (run 58: "2.0 frame waits sleeping 16.8 ms"). Polls (a
    // time-out shorter than a tick) answer as before.
    // `VICTORY_LATE_WAITS=1` sleeps to the next tick, as before.
    static SLEEP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let late_waits =
        *SLEEP.get_or_init(|| std::env::var_os("VICTORY_LATE_WAITS").is_some_and(|v| v == "1"));
    let (hr, _tick) = loop {
        let (mut tick, mut wait) = next_tick(kind);
        let remaining = deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::MAX);
        if !late_waits
            && remaining >= wait
            && tick > last(kind).load(Ordering::Relaxed) + 1
        {
            tick -= 1;
            wait = Duration::ZERO;
        }
        if wait > remaining {
            if !remaining.is_zero() {
                std::thread::sleep(remaining);
            }
            break (TIMEOUT, tick);
        }
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        // Only one waiter may consume a given tick. If another waiter already
        // claimed `tick` (or a later one), retry against the original deadline.
        if last(kind).fetch_max(tick, Ordering::AcqRel) < tick {
            if kind == FLIP {
                FLIPPED.store(true, Ordering::Relaxed);
            }
            break (S_OK, tick);
        }
    };
    #[cfg(feature = "diagnostics")]
    let waited = started.elapsed();
    #[cfg(all(windows, feature = "diagnostics"))]
    {
        use crate::copystat::{C, add};
        add(C::PaceWaits, 1);
        add(C::PaceUs, waited.as_micros() as u64);
    }
    // A frame begun on a time-out takes no tick: its flip still does.
    if origin && (hr == S_OK || flags & TOKEN_ON_TIMEOUT != 0) {
        // Tokens count frames (consecutive), not ticks.
        let frame = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
        if !token.is_null() {
            // SAFETY: checked non-null; the title passes its token.
            unsafe { token.write(frame) };
        }
    }
    #[cfg(feature = "diagnostics")]
    tracing::debug!(
        kind,
        timeout_ms,
        flags,
        hr,
        tick = _tick,
        wait_us = waited.as_micros() as u64,
        frame = current(),
        "frame wait"
    );
    hr
}

/// Called before each present to the host. A present with no flip wait
/// since the last one (a frame begun early, on an origin time-out: the
/// movie player catching up) waits for the next flip tick and takes it, as
/// the console flips such a frame on its tick. Shown at once, it stayed on
/// screen ~5 ms and the frames around it twice as long: the intro movie
/// judders on its first seconds.
pub(crate) fn before_present() {
    if FLIPPED.swap(false, Ordering::Relaxed) {
        return;
    }
    let (tick, wait) = next_tick(FLIP);
    std::thread::sleep(wait);
    last(FLIP).fetch_max(tick, Ordering::Relaxed);
}

/// Device method: no statistics are kept, so none are returned.
pub(crate) extern "system" fn statistics(
    _: This,
    _token: u64,
    _kinds: u32,
    count: *mut u32,
    _out: *mut u8,
) -> HRESULT {
    if !count.is_null() {
        // SAFETY: checked non-null.
        unsafe { count.write(0) };
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn waits_end_on_ticks_and_count_frames() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let null = std::ptr::null();
        assert_eq!(set_interval(null, null, 2_000, 1, 0), S_OK);
        let mut first = 0;
        assert_eq!(wait(null, 0, 1_000, std::ptr::null(), 0, &mut first), S_OK);
        let before = Instant::now();
        let mut second = 0;
        assert_eq!(wait(null, 0, 1_000, std::ptr::null(), 0, &mut second), S_OK);
        assert!(second > first);
        assert!(before.elapsed() <= Duration::from_millis(50));
        assert_eq!(wait(null, 0, 0, std::ptr::null(), 0, &mut second), TIMEOUT);
        // Tokens count frames, a frame started early on a time-out too.
        let mut early = 0;
        let flags = TOKEN_ON_TIMEOUT;
        assert_eq!(
            wait(null, 0, 0, std::ptr::null(), flags, &mut early),
            TIMEOUT
        );
        let mut next = 0;
        assert_eq!(wait(null, 0, 1_000, std::ptr::null(), 0, &mut next), S_OK);
        assert!(early == second + 1 && next == early + 1);
        assert!(LAST[0].load(Ordering::Relaxed) >= 2);

        // The title's loop (the clock is global, so in the same test): an
        // origin poll that times out (and starts the frame), then a
        // blocking flip wait. Each flip takes its own tick.
        const FLIP: u32 = 2;
        assert_eq!(set_interval(null, null, 5_000, 3, 0), S_OK);
        let mut token = 0;
        let flips: Vec<u64> = (0..4)
            .map(|_| {
                let _ = wait(null, 0, 0, std::ptr::null(), TOKEN_ON_TIMEOUT, &mut token);
                assert_eq!(
                    wait(null, FLIP, u32::MAX, std::ptr::null(), 0, &mut token),
                    S_OK
                );
                LAST[2].load(Ordering::Relaxed)
            })
            .collect();
        assert!(flips.windows(2).all(|w| w[1] == w[0] + 1), "{flips:?}");
        // Even when a flip wait begins on the tick another just ended on.
        let (tick, _) = next_tick(FLIP);
        assert!(tick > flips[3]);

        // A present right after a flip wait goes at once and takes no tick.
        assert_eq!(
            wait(null, FLIP, u32::MAX, std::ptr::null(), 0, &mut token),
            S_OK
        );
        let flipped = LAST[2].load(Ordering::Relaxed);
        before_present();
        assert_eq!(LAST[2].load(Ordering::Relaxed), flipped);
        // A present with no flip wait since the last one (a frame begun
        // early, on a time-out) is held to the next flip tick, as the
        // console flips it there: the movie player's catch-up frames.
        before_present();
        let held = LAST[2].load(Ordering::Relaxed);
        assert!(held > flipped, "{held} after {flipped}");
        // And the next frame's flip wait takes the tick after it.
        assert_eq!(
            wait(null, FLIP, u32::MAX, std::ptr::null(), 0, &mut token),
            S_OK
        );
        assert!(LAST[2].load(Ordering::Relaxed) > held);
    }

    #[test]
    fn concurrent_flip_waits_consume_distinct_ticks() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        const FLIP: u32 = 2;
        let null = std::ptr::null();
        assert_eq!(set_interval(null, null, 20_000, 1, 0), S_OK);
        let before = LAST[2].load(Ordering::Relaxed);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    let mut token = 0;
                    wait(std::ptr::null(), FLIP, 500, std::ptr::null(), 0, &mut token)
                })
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), S_OK);
        }
        let after = LAST[2].load(Ordering::Relaxed);
        assert!(
            after >= before + 2,
            "two concurrent FLIP waits must consume two distinct ticks (before={before}, after={after})"
        );
    }

    #[test]
    fn concurrent_flip_retry_does_not_extend_timeout() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        const FLIP: u32 = 2;
        // 200 ms interval, 285 ms timeout: one tick fits with wide scheduling
        // margin, while a second tick (400 ms) does not. Start the interval
        // after both threads are spawned and aligned at the first barrier so
        // thread-spawn delay cannot consume the first tick window.
        let before = LAST[2].load(Ordering::Relaxed);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let b = barrier.clone();
                std::thread::spawn(move || {
                    if b.wait().is_leader() {
                        assert_eq!(
                            set_interval(std::ptr::null(), std::ptr::null(), 200_000, 1, 0),
                            S_OK
                        );
                    }
                    b.wait();
                    let mut token = 0;
                    wait(std::ptr::null(), FLIP, 285, std::ptr::null(), 0, &mut token)
                })
            })
            .collect();
        let results: Vec<HRESULT> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|&&r| r == S_OK).count(),
            1,
            "exactly one waiter should claim the single tick within 285 ms: {results:?}"
        );
        assert_eq!(
            results.iter().filter(|&&r| r == TIMEOUT).count(),
            1,
            "the losing waiter must time out against the original deadline: {results:?}"
        );
        assert_eq!(LAST[2].load(Ordering::Relaxed), before + 1);
    }
}
