//! Progress from real counts: completed/total, the rate as a moving average
//! and the time left from it. A long stage (the shader prewarm) feeds the
//! loading screen when one is shown, or one console line rewritten in place
//! (`\r`) when it is not; callers keep their own plain log lines.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The rate's time constant: about the last 5 s of work weigh most.
const SMOOTHING: f64 = 5.0;
/// Console line rewrites at most this often.
const CONSOLE_EVERY: Duration = Duration::from_millis(250);

/// Completed/total with a smoothed rate.
pub struct Meter {
    total: usize,
    started: Instant,
    last: Option<(Instant, usize)>,
    rate: Option<f64>,
}

impl Meter {
    pub fn new(total: usize) -> Self {
        Self { total, started: Instant::now(), last: None, rate: None }
    }

    /// Takes the completed count now; never moves backwards.
    pub fn sample(&mut self, done: usize) {
        let now = Instant::now();
        let done = done.min(self.total);
        match self.last {
            None => self.last = Some((now, done)),
            Some((at, before)) => {
                let dt = now.duration_since(at).as_secs_f64();
                if dt < 0.05 {
                    return;
                }
                let done = done.max(before);
                let instant = (done - before) as f64 / dt;
                // Time-weighted exponential average, so the sampling rate does
                // not change how fast it follows.
                let a = 1.0 - (-dt / SMOOTHING).exp();
                self.rate = Some(match self.rate {
                    // The first estimate is the whole run's average.
                    None => done as f64 / now.duration_since(self.started).as_secs_f64().max(0.001),
                    Some(r) => r + a * (instant - r),
                });
                self.last = Some((now, done));
            }
        }
    }

    pub fn done(&self) -> usize {
        self.last.map_or(0, |(_, d)| d)
    }

    /// 0..1 of the total.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 1.0 } else { self.done() as f32 / self.total as f32 }
    }

    /// Smoothed items per second, once there is a sample to take it from.
    pub fn rate(&self) -> Option<f64> {
        self.rate.filter(|r| *r > 0.0)
    }

    /// Time left at the smoothed rate.
    pub fn eta(&self) -> Option<Duration> {
        let left = self.total.saturating_sub(self.done());
        if left == 0 {
            return Some(Duration::ZERO);
        }
        self.rate().map(|r| Duration::from_secs_f64((left as f64 / r).min(360_000.0)))
    }
}

/// "1m 05s", "42s".
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// The meter as one short line: "seen prewarm 412/1300 (31.7%), 12.4/s, 1m 12s left".
pub fn line(label: &str, m: &Meter) -> String {
    let eta = m.eta().map_or_else(|| "estimating".to_owned(), |e| format!("{} left", clock(e)));
    let rate = m.rate().map_or_else(String::new, |r| format!(", {r:.1}/s"));
    format!("{label} {}/{} ({:.1}%){rate}, {eta}", m.done(), m.total, 100.0 * m.fraction())
}

static INLINE: AtomicBool = AtomicBool::new(false);
static CONSOLE: Mutex<Option<(Instant, usize)>> = Mutex::new(None);

/// --prewarm with no loading screen: long stages show one console line
/// rewritten in place instead of a line per tick.
pub fn set_inline(on: bool) {
    INLINE.store(on, Ordering::Release);
}

pub fn inline() -> bool {
    INLINE.load(Ordering::Acquire)
}

/// Shows `label`'s meter: on the loading screen between `from` and `to` of
/// its bar, or on the in-place console line. Call as often as wanted; both
/// redraw at their own steady rate.
pub fn report(label: &str, m: &Meter, from: f32, to: f32) {
    let text = line(label, m);
    crate::splash::counted(from + (to - from) * m.fraction(), &text);
    if !inline() {
        return;
    }
    let mut console = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if console.is_some_and(|(at, _)| now.duration_since(at) < CONSOLE_EVERY) && m.done() < m.total {
        return;
    }
    let width = console.map_or(0, |(_, w)| w);
    let mut out = std::io::stdout().lock();
    // Pad over the previous line's tail.
    let _ = write!(out, "\r{text:<width$}");
    let _ = out.flush();
    *console = Some((now, text.chars().count()));
}

/// Ends the in-place line so the next output starts on its own line.
pub fn finish() {
    let mut console = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
    if console.take().is_some() {
        println!();
    }
}
