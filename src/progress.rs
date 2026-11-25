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

/// 26503 -> "26,503".
pub fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The progress count in a runtime's output line: (done, Some(total)), or
/// (done, None) where it prints no total. Read from the leaders' sources:
/// - a `done/total` token: cargo's `Building [=>  ] 45/120: a, b` (with
///   CARGO_TERM_PROGRESS_WHEN=always), xcl's `  1000/26503 (72 s)`
///   (gpu/recomp/dxc.rs), TuxBox's `<label> · 12/400 ready · ...`
///   (project_tuxbox/prep_progress.py), Five Star's `1200/5000 (24.0%)
///   elapsed ...` (Shader/ConsoleProgress.cs) and setup's `[2/7] <title>`
///   (Setup/SetupRunner.cs);
/// - victory's `warm-up      120 shaders so far, 3 s` (apps/victory/src/translate.rs).
pub fn count(line: &str) -> Option<(u64, Option<u64>)> {
    let num = |s: &str| -> Option<u64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b',') || s.starts_with(',') {
            return None;
        }
        s.replace(',', "").parse().ok()
    };
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("warm-up") && let Some((n, _)) = rest.trim_start().split_once(" shaders so far") {
        return num(n.trim()).map(|n| (n, None));
    }
    for tok in t.split(|c: char| c.is_whitespace() || matches!(c, '[' | ']' | ':' | '(' | ')')) {
        if let Some((a, b)) = tok.split_once('/') && let (Some(d), Some(m)) = (num(a), num(b)) && m > 0 && d <= m {
            return Some((d, Some(m)));
        }
    }
    None
}

/// Five Star's setup inside a run: `[2/7] <title>` (Setup/SetupRunner.cs) as
/// (done, total, title), or its meter `1200/5000 (24.0%) ...`
/// (Shader/ConsoleProgress.cs) with no title. Only these two shapes, so a
/// game's own log lines are never read as progress.
pub fn setup_line(line: &str) -> Option<(u64, u64, Option<&str>)> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix('[') && let Some((n, title)) = rest.split_once("] ") && let Some((Some(d), Some(m))) = n.split_once('/').map(|(a, b)| (a.parse().ok(), b.parse().ok())) {
        return Some((u64::saturating_sub(d, 1), m, Some(title.trim())));
    }
    let (first, rest) = t.split_once(' ')?;
    if !rest.trim_start().starts_with('(') || !rest.contains("%)") {
        return None;
    }
    match count(first)? {
        (d, Some(m)) => Some((d, m, None)),
        _ => None,
    }
}

/// A build's stderr, read to its end: each line goes to `each` (cargo's
/// in-place progress lines excepted), and the loader bar between `from` and
/// `to` follows cargo's `Building [..] N/M` (asked for with
/// CARGO_TERM_PROGRESS_WHEN=always) or another N/M count; with none yet, the
/// crate being compiled and a pulse.
pub fn build_lines(err: impl std::io::Read, label: &str, from: f32, to: f32, mut each: impl FnMut(&str)) {
    use std::io::BufRead;
    let mut r = std::io::BufReader::new(err);
    let (mut compiled, mut counted) = (0u64, false);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // A line ends at \n, or at the \r cargo rewrites its progress with.
        let mut end = false;
        loop {
            let chunk = match r.fill_buf() {
                Ok(c) => c,
                Err(_) => {
                    end = true;
                    break;
                }
            };
            if chunk.is_empty() {
                end = true;
                break;
            }
            match chunk.iter().position(|&b| b == b'\n' || b == b'\r') {
                Some(i) => {
                    buf.extend_from_slice(&chunk[..i]);
                    r.consume(i + 1);
                    break;
                }
                None => {
                    let n = chunk.len();
                    buf.extend_from_slice(chunk);
                    r.consume(n);
                }
            }
        }
        let l = String::from_utf8_lossy(&buf);
        let t = l.trim();
        if !t.is_empty() {
            if t.starts_with("Building [") {
                if let Some((d, Some(m))) = count(t) {
                    counted = true;
                    crate::splash::step(from, to, label, d, Some(m));
                }
            } else {
                each(&l);
                if let Some(k) = t.strip_prefix("Compiling ") {
                    compiled += 1;
                    if !counted {
                        crate::splash::pulse(&format!("{label}: {} ({compiled} compiled)", k.split(' ').next().unwrap_or(k)));
                    }
                } else if let Some((d, Some(m))) = count(t) {
                    counted = true;
                    crate::splash::step(from, to, label, d, Some(m));
                }
            }
        }
        if end {
            break;
        }
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

#[cfg(test)]
mod counts {
    use super::{count, grouped, setup_line};

    #[test]
    fn cargo_build() {
        // Every Rust runtime's build (CARGO_TERM_PROGRESS_WHEN=always).
        assert_eq!(count("    Building [=======>                 ] 45/120: xcl, windows-sys"), Some((45, Some(120))));
        assert_eq!(count(r"   Compiling xcl v0.1.0 (C:\src\xcl\crates\xcl)"), None);
        assert_eq!(count("    Finished `release` profile [optimized + debuginfo] target(s) in 1m 08s"), None);
    }

    #[test]
    fn xcl_warm() {
        assert_eq!(count("  1000/26503 (72 s)"), Some((1000, Some(26503))));
        assert_eq!(count(r"precompiling 26503 shaders into C:\x\dxil (15 compile threads; 4 memory windows)"), None);
    }

    #[test]
    fn victory_warm() {
        // victory, vitoriabr and ScarlettPort's warm (translate.rs).
        assert_eq!(count("warm-up      120 shaders so far, 3 s"), Some((120, None)));
    }

    #[test]
    fn scarlettport_graphics() {
        // Record lines are read by warm_line; no N/M in them.
        assert_eq!(count("    41 s  12000 records: 11996 made, 0 failed, 4 duplicates, 0 unmapped, 0 undecoded"), None);
    }

    #[test]
    fn tuxbox_prepare() {
        // TuxBox and neonbridge-own's --prepare-only (prep_progress.py PrepPhase).
        assert_eq!(count("ProjectScarlett shaders \u{b7} 400/26503 ready \u{b7} 12 compiled \u{b7} 388 reused"), Some((400, Some(26503))));
        assert_eq!(count("Frosty compute pipelines ready: 12 cached."), None);
    }

    #[test]
    fn fivestar_setup() {
        assert_eq!(setup_line("[2/7] Translating shaders"), Some((1, 7, Some("Translating shaders"))));
        assert_eq!(setup_line("1200/5000 (24.0%)  elapsed 1m 05s  left ~4m 30s  finish ~12:30  pso"), Some((1200, 5000, None)));
        assert_eq!(setup_line("  1200/5000  41/s  peak 3.2 GB"), None);
        assert_eq!(count("  1200/5000  41/s  peak 3.2 GB"), Some((1200, Some(5000))));
        // A game's own lines are not setup progress.
        assert_eq!(setup_line(r"TuxBox xmem: G:\common.rpf -> C:\g\common.rpf (3 so far)"), None);
    }

    #[test]
    fn not_counts() {
        assert_eq!(count("2026/11/25 dates are not counts"), None);
        assert_eq!(count("12.3 fps, 81.3 ms/frame"), None);
        assert_eq!(count("5/3 is more done than total"), None);
    }

    #[test]
    fn groups() {
        assert_eq!(grouped(26503), "26,503");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(999), "999");
    }
}
