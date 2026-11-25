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

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::object::{HRESULT, S_FALSE, S_OK, This};

/// What `GetFrameStatisticsX` answers for a frame not flipped yet.
const E_FAIL: HRESULT = 0x8000_4005_u32 as HRESULT;
const E_INVALIDARG: HRESULT = 0x8007_0057_u32 as HRESULT;

/// What a wait returns when its time is up first.
const TIMEOUT: HRESULT = S_FALSE;
/// The frame-origin event: the one that starts frames and makes tokens.
const ORIGIN: u32 = 0;
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

/// Set while fast boot ran the clock without sleeping.
static UNTHROTTLED: AtomicBool = AtomicBool::new(false);

/// Fast boot (testing only): while the title is not in the world our own
/// pacing sleep is skipped. Ticks still advance one per wait, so tokens stay
/// consecutive; the title's own clocks are untouched. Returns whether this
/// wait runs unthrottled. When fast boot ends the clock restarts from now,
/// so the ticks run ahead meanwhile are not slept off.
fn unthrottled() -> bool {
    if victory_core::fastboot::active() {
        UNTHROTTLED.store(true, Ordering::Relaxed);
        return true;
    }
    if UNTHROTTLED.swap(false, Ordering::Relaxed) {
        let first = LAST.iter().map(|t| t.load(Ordering::Relaxed)).max().unwrap_or(0);
        if let Some(c) = clock().as_mut() {
            c.start = Instant::now();
            c.first = first;
        }
    }
    false
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
    let (mut tick, mut wait) = next_tick(kind);
    let limit = Duration::from_millis(u64::from(timeout_ms));
    // A blocking wait for an event that has happened since the last wait
    // for it ended (a frame took longer than a tick) ends at once, on the
    // tick that passed: sleeping on to the next one cost 17 ms of every
    // 730 ms frame (run 58: "2.0 frame waits sleeping 16.8 ms"). Polls (a
    // time-out shorter than a tick) answer as before.
    // `VICTORY_LATE_WAITS=1` sleeps to the next tick, as before.
    static SLEEP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*SLEEP.get_or_init(|| std::env::var_os("VICTORY_LATE_WAITS").is_some_and(|v| v == "1"))
        && limit >= wait
        && tick > last(kind).load(Ordering::Relaxed) + 1
    {
        tick -= 1;
        wait = Duration::ZERO;
    }
    let fast = unthrottled() && !limit.is_zero();
    let hr = if fast {
        last(kind).fetch_max(tick, Ordering::Relaxed);
        S_OK
    } else if wait > limit {
        std::thread::sleep(limit);
        TIMEOUT
    } else {
        std::thread::sleep(wait);
        last(kind).fetch_max(tick, Ordering::Relaxed);
        S_OK
    };
    #[cfg(all(windows, feature = "diagnostics"))]
    {
        use crate::copystat::{C, add};
        add(C::PaceWaits, 1);
        add(C::PaceUs, wait.min(limit).as_micros() as u64);
    }
    // A frame begun on a time-out takes no tick: its flip still does.
    if origin && (hr == S_OK || flags & TOKEN_ON_TIMEOUT != 0) {
        // Tokens count frames (consecutive), not ticks.
        let frame = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
        issued(frame);
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
        tick,
        wait_us = wait.as_micros() as u64,
        frame = current(),
        "frame wait"
    );
    hr
}

/// The frames the title may ask about (`GetFrameStatisticsX`): a token's
/// origin, taken when its origin wait handed it out, and its flip, taken
/// at the `PresentX` that names it; both as the performance counter reads,
/// like the title's own clock. The last few only: the title asks about the
/// frame it just presented, or the one before.
const KEPT: usize = 16;
/// (token, origin)
static ISSUED: Mutex<VecDeque<(u64, i64)>> = Mutex::new(VecDeque::new());
/// (token, origin, flip)
static FLIPPED: Mutex<VecDeque<(u64, i64, i64)>> = Mutex::new(VecDeque::new());

fn counter() -> i64 {
    #[cfg(windows)]
    {
        let mut count = 0;
        // SAFETY: a valid out pointer.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut count) };
        count
    }
    #[cfg(not(windows))]
    {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as i64
    }
}

fn keep<T>(queue: &Mutex<VecDeque<T>>, item: T) {
    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
    q.push_back(item);
    while q.len() > KEPT {
        q.pop_front();
    }
}

/// An origin wait handed `token` out: the frame begins now.
fn issued(token: u64) {
    keep(&ISSUED, (token, counter()));
}

/// The token of the last frame `PresentX` flipped.
static LAST_TOKEN: AtomicU64 = AtomicU64::new(0);

/// The token of the frame flipped last (0: none yet).
pub(crate) fn last_token() -> u64 {
    LAST_TOKEN.load(Ordering::Relaxed)
}

/// The title's frame interval now.
pub(crate) fn interval() -> Duration {
    clock().as_ref().map_or(SIXTIETH, |c| c.interval)
}

/// Performance counter ticks per second.
fn frequency() -> i64 {
    #[cfg(windows)]
    {
        static F: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
        *F.get_or_init(|| {
            let mut f = 0;
            // SAFETY: a valid out pointer.
            unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut f) };
            f.max(1)
        })
    }
    #[cfg(not(windows))]
    {
        1_000_000_000
    }
}

/// `VICTORY_PACE_FEEDBACK`: the frame of `token` reached the display at
/// performance counter `qpc` (the swap chain's statistics). What the title
/// is told about the frame becomes that time, and the frame clock moves a
/// step toward it, so the title's ticks fall where frames are shown instead
/// of drifting against the display.
pub(crate) fn displayed(token: u64, qpc: i64) {
    if token == 0 || qpc <= 0 {
        return;
    }
    {
        let mut q = FLIPPED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = q.iter_mut().rev().find(|e| e.0 == token) {
            e.2 = e.2.max(qpc);
        }
    }
    let now = counter();
    if qpc > now {
        return;
    }
    let ago = Duration::from_nanos(((now - qpc) as u128 * 1_000_000_000 / frequency() as u128) as u64);
    let Some(at) = Instant::now().checked_sub(ago) else { return };
    if let Some(c) = clock().as_mut()
        && let Some(elapsed) = at.checked_duration_since(c.start)
    {
        let nudge = victory_core::pace::phase_nudge(elapsed.as_nanos() as u64, c.interval.as_nanos() as u64);
        // Ticks are start + n * interval: a later start moves them later.
        if nudge >= 0 {
            c.start += Duration::from_nanos(nudge as u64);
        } else if let Some(s) = c.start.checked_sub(Duration::from_nanos(nudge.unsigned_abs())) {
            c.start = s;
        }
    }
}

/// `PresentX` flipped the frame of `token` (0: a present without a frame).
pub(crate) fn flipped(token: u64) {
    if token == 0 {
        return;
    }
    LAST_TOKEN.store(token, Ordering::Relaxed);
    let origin = ISSUED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .find(|&&(t, _)| t == token)
        .map(|&(_, at)| at);
    let now = counter();
    // A token not from an origin wait of ours (none is expected): its frame
    // began no later than it was flipped.
    keep(&FLIPPED, (token, origin.unwrap_or(now).min(now), now));
}

/// Device method: one record for a frame that was flipped, as the console
/// gives it (`{ u32 kind; ...; i64 origin @0x10; i64 flip @0x18 }`),
/// `count` the records there is room for in, written out. E_FAIL with no
/// record while the frame has not been flipped; the title asks again, or
/// goes on without. Without it a title that waits for its frames to be
/// shown before it reuses their buffers (the opening video: three frames
/// in turn, each kept eight frames) never learns that they were.
pub(crate) extern "system" fn statistics(
    _: This,
    token: u64,
    kind: u32,
    count: *mut u32,
    out: *mut u8,
) -> HRESULT {
    if count.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let flipped = FLIPPED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .find(|&&(t, _, _)| t == token)
        .map(|&(_, origin, flip)| (origin, flip));
    // SAFETY: checked non-null; the title passes the room it has.
    let room = unsafe { count.read() };
    #[cfg(feature = "diagnostics")]
    tracing::debug!(token, kind, room, answered = flipped.is_some(), "frame statistics");
    let Some((origin, flip)) = flipped else {
        // SAFETY: as above.
        unsafe { count.write(0) };
        return E_FAIL;
    };
    if room < 1 {
        return E_INVALIDARG;
    }
    // SAFETY: the title's record, at least 0x20 bytes for one.
    unsafe {
        count.write(1);
        out.cast::<u32>().write_unaligned(kind);
        out.add(0x10).cast::<i64>().write_unaligned(origin);
        out.add(0x18).cast::<i64>().write_unaligned(flip);
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_tell_when_a_frame_was_flipped() {
        let null = std::ptr::null();
        assert_eq!(set_interval(null, null, 2_000, 1, 0), S_OK);
        let mut token = 0;
        assert_eq!(wait(null, 0, 1_000, std::ptr::null(), 0, &mut token), S_OK);
        let mut count = 1u32;
        let mut record = [0u8; 0x20];
        // Not flipped yet: no record.
        assert_eq!(statistics(null, token, 2, &mut count, record.as_mut_ptr()), E_FAIL);
        assert_eq!(count, 0);
        let before = counter();
        flipped(token);
        let mut count = 1u32;
        assert_eq!(statistics(null, token, 2, &mut count, record.as_mut_ptr()), S_OK);
        assert_eq!(count, 1);
        assert_eq!(u32::from_ne_bytes(record[..4].try_into().unwrap()), 2);
        let origin = i64::from_ne_bytes(record[0x10..0x18].try_into().unwrap());
        let flip = i64::from_ne_bytes(record[0x18..0x20].try_into().unwrap());
        assert!(origin <= flip && flip >= before, "{origin} {flip} {before}");
        // No room for a record.
        let mut none = 0u32;
        assert_eq!(statistics(null, token, 2, &mut none, record.as_mut_ptr()), E_INVALIDARG);
        // A frame never presented stays unknown.
        let mut count = 1u32;
        assert_eq!(statistics(null, token + 1_000_000, 2, &mut count, record.as_mut_ptr()), E_FAIL);
    }

    #[test]
    fn waits_end_on_ticks_and_count_frames() {
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
    }
}
