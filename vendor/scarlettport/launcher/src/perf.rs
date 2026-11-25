//! The performance readout after a run, parsed from its log: average frame rate, the worst
//! frame interval and the time from start to the world (the first interval that draws a world's
//! worth of geometry). Also the run log files the launcher keeps (`<data>\logs`).
use std::path::{Path, PathBuf};

/// Draws per frame from which a frame counts as "in the world" (menus draw a few dozen).
pub const WORLD_DRAWS: u64 = 1000;
/// Run logs kept.
pub const KEEP_LOGS: usize = 10;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Perf {
    pub avg_fps: Option<f32>,
    pub worst_ms: Option<f32>,
    pub world_secs: Option<f32>,
    pub samples: usize,
}

impl Perf {
    pub fn line(&self) -> Option<String> {
        if self.samples == 0 && self.world_secs.is_none() {
            return None;
        }
        let mut parts = Vec::new();
        if let Some(f) = self.avg_fps {
            parts.push(format!("{f:.1} fps"));
        }
        if let Some(w) = self.worst_ms {
            parts.push(format!("worst frame {w:.0} ms"));
        }
        parts.push(match self.world_secs {
            Some(s) => format!("world in {}", crate::flow::duration(s as u64)),
            None => "world not reached".into(),
        });
        Some(parts.join("  ·  "))
    }
}

/// Removes ANSI colour sequences (the runtime colours its log when it thinks it has a terminal).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Seconds of the day from a leading `2026-11-25T03:19:48.820868Z`.
pub fn timestamp(line: &str) -> Option<f64> {
    let t = line.trim_start();
    let time = t.get(11..)?.split(|c: char| c == 'Z' || c.is_whitespace()).next()?;
    if t.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    let mut p = time.split(':');
    let (h, m, s): (f64, f64, f64) = (p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?);
    Some(h * 3600. + m * 60. + s)
}

fn number_before(s: &str) -> Option<f32> {
    let n: String = s.trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
    n.parse().ok()
}

fn number_after(s: &str) -> Option<u64> {
    let n: String = s.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok()
}

/// One interval line: `1.3 fps, 781.5 ms/frame (...), 0 draws/frame` -> (fps, ms, draws).
pub fn interval(line: &str) -> Option<(f32, f32, Option<u64>)> {
    let i = line.find(" fps, ")?;
    let fps = number_before(&line[..i])?;
    let rest = &line[i + 6..];
    let j = rest.find(" ms/frame")?;
    let ms: f32 = rest[..j].trim().parse().ok()?;
    let draws = line.find(" draws/frame").and_then(|k| number_before(&line[..k])).map(|d| d as u64);
    Some((fps, ms, draws))
}

/// Parses a run's log lines. `started` is the run's start as seconds of the day, when known
/// (else the first timestamp in the log).
pub fn parse<'a>(lines: impl IntoIterator<Item = &'a str>, started: Option<f64>) -> Perf {
    let mut first = started;
    let mut fps = Vec::new();
    let mut worst: Option<f32> = None;
    let mut world = None;
    let mut in_world = false;
    for raw in lines {
        let l = strip_ansi(raw);
        let ts = timestamp(&l);
        if first.is_none() {
            first = ts;
        }
        if let Some((f, ms, draws)) = interval(&l) {
            if world.is_none() && draws.is_some_and(|d| d >= WORLD_DRAWS) {
                world = ts.zip(first).map(|(t, s)| ((t - s).rem_euclid(86400.)) as f32);
                // boot and loading stalls are not the game's frames: count from the world on
                fps.clear();
                worst = None;
                in_world = true;
            }
            fps.push(f);
            worst = Some(worst.map_or(ms, |w: f32| w.max(ms)));
        } else if let Some(f) = crate::engine::parse_fps(&l).filter(|_| l.contains("fps over")) {
            fps.push(f);
        }
        if let Some(k) = l.find("world after") {
            // a runtime that says so itself: "... world after 102.4 s"
            if world.is_none() {
                world = number_after(&l[k + 11..]).map(|s| s as f32);
            }
        }
    }
    // the first intervals are boot and menus: average the second half of the run
    let tail = if in_world { &fps[..] } else { &fps[fps.len() / 2..] };
    Perf { avg_fps: (!tail.is_empty()).then(|| tail.iter().sum::<f32>() / tail.len() as f32), worst_ms: worst, world_secs: world, samples: fps.len() }
}

pub fn logs_dir() -> PathBuf {
    crate::runtime::data_dir().join("logs")
}

/// A new run log path (`run-<stamp>.log`), pruning old ones.
pub fn new_log() -> PathBuf {
    let dir = logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut logs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect();
        logs.sort();
        let excess = (logs.len() + 1).saturating_sub(KEEP_LOGS);
        for p in &logs[..excess.min(logs.len())] {
            let _ = std::fs::remove_file(p);
        }
    }
    dir.join(format!("run-{}.log", crate::saves::now_stamp()))
}

pub fn last_log() -> Option<PathBuf> {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(logs_dir()).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect();
    logs.sort();
    logs.pop()
}

/// The last `n` lines of a file.
pub fn tail(path: &Path, n: usize) -> Vec<String> {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(strip_ansi).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_intervals() {
        let l = "2026-11-25T03:19:48.820868Z  INFO victory_gpu::host: 1.3 fps, 781.5 ms/frame (0.2 preparing: copies 0.0 ms), 0 draws/frame, dro";
        assert_eq!(interval(l), Some((1.3, 781.5, Some(0))));
        assert_eq!(timestamp(l).map(|t| t.round()), Some((3 * 3600 + 19 * 60 + 49) as f64));
        assert_eq!(strip_ansi("\u{1b}[2mA\u{1b}[0m B"), "A B");
    }

    #[test]
    fn readout() {
        let log = [
            "2026-11-25T03:00:00.000Z  INFO x: starting",
            "2026-11-25T03:00:30.000Z  INFO victory_gpu::host: 30.0 fps, 33.3 ms/frame (x), 40 draws/frame",
            "2026-11-25T03:01:42.000Z  INFO victory_gpu::host: 3.0 fps, 263.0 ms/frame (x), 4200 draws/frame",
            "2026-11-25T03:01:52.000Z  INFO victory_gpu::host: 5.0 fps, 200.0 ms/frame (x), 4300 draws/frame",
        ];
        let p = parse(log, None);
        assert_eq!(p.samples, 2);
        assert_eq!(p.worst_ms, Some(263.));
        assert_eq!(p.world_secs, Some(102.));
        assert_eq!(p.avg_fps, Some(4.));
        assert!(p.line().unwrap().contains("worst frame 263 ms"));
        assert_eq!(parse(["nothing"], None).line(), None);
    }
}
