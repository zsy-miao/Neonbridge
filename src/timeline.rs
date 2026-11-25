//! A short, readable timeline beside each run log (`<log>.timeline.txt`), one
//! line per milestone: `[t=12.3 s] first present`. Milestones come from the
//! game's own stderr lines: a line carrying `TIMELINE: <text>` (the runtime's
//! tag for milestones without another line: title window, Tux screen, auto A
//! presses, the first pipeline, the movie closing, the loading screen, the
//! first gameplay frame) becomes `<text>`, and a few existing lines are read
//! directly (first PresentX, BinkOpen, watch-hang state, the fps every 30 s,
//! the exit). The time is the line's own `[t=…]` (seconds since the game
//! process started) where it has one, else the launcher's clock. Each text is
//! written once, except auto A presses, watch-hang changes and fps samples.
//! NEONBRIDGE_TIMELINE=0 writes no timeline.
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

struct State {
    file: fs::File,
    start: Instant,
    seen: std::collections::HashSet<String>,
    hang: Option<String>,
    last_fps: f64,
    runtime: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn enabled() -> bool {
    std::env::var("NEONBRIDGE_TIMELINE").as_deref() != Ok("0")
}

/// Opens `<log>.timeline.txt` (truncating it) and writes the launch line.
pub fn start(log: &Path, start: Instant) {
    if !enabled() {
        return;
    }
    let mut name = log.as_os_str().to_owned();
    name.push(".timeline.txt");
    let Ok(file) = fs::File::create(&name) else { return };
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(State { file, start, seen: Default::default(), hang: None, last_fps: f64::NEG_INFINITY, runtime: false });
    note(None, "launch");
}

fn stamp(line: &str) -> Option<f64> {
    let rest = &line[line.find("[t=")? + 3..];
    rest[..rest.find(' ')?].parse().ok()
}

fn write(state: &mut State, t: Option<f64>, text: &str) {
    let t = t.unwrap_or_else(|| state.start.elapsed().as_secs_f64());
    let _ = writeln!(state.file, "[t={t:.1} s] {text}");
}

/// Writes a launcher-side milestone once.
pub fn note(t: Option<f64>, text: &str) {
    if let Some(state) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
        && state.seen.insert(text.to_owned())
    {
        write(state, t, text);
    }
}

/// Reads one game output line for milestones.
pub fn line(line: &str) {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else { return };
    let t = stamp(line);
    let once = |state: &mut State, text: &str| {
        if state.seen.insert(text.to_owned()) {
            write(state, t, text);
        }
    };
    if let Some(i) = line.find("TIMELINE: ") {
        let text = line[i + 10..].split(" [t=").next().unwrap_or("").trim();
        // Repeatable milestones say so with a trailing count or name
        // ("auto A press 3"): every distinct text is written once.
        once(state, text);
    } else if let Some(i) = line.find("auto start: pressing A once") {
        let n = line[i..].split("(try ").nth(1).and_then(|s| s.split('/').next()).unwrap_or("?");
        write(state, t, &format!("auto A press {n}"));
    } else if line.contains("auto start: no A pressed") || line.contains("auto start: the title never read") {
        once(state, line.split("auto start: ").nth(1).unwrap_or(line).split(" [t=").next().unwrap_or(""));
    } else if !state.runtime && line.contains("TuxBox d3d12_xs: ") {
        state.runtime = true;
        once(state, "runtime loaded");
    } else if line.contains("completed PresentX #1 ") {
        once(state, "first present");
    } else if let Some(i) = line.find("completed PresentX #") {
        let fps = line[i..].split('(').nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse::<f64>().ok());
        let now = t.unwrap_or_else(|| state.start.elapsed().as_secs_f64());
        if let Some(fps) = fps
            && now - state.last_fps >= 30.0
        {
            state.last_fps = now;
            write(state, t, &format!("{fps:.1} fps"));
        }
    } else if line.contains("bink: BinkOpenWithOptions = ") {
        once(state, "intro movie opened (BinkOpen)");
    } else if let Some(i) = line.find("watch-hang: state ") {
        let rest = &line[i + 11..];
        let shown = rest.split(", session byte").next().unwrap_or(rest).to_owned();
        if state.hang.as_deref() != Some(&shown) {
            state.hang = Some(shown.clone());
            write(state, t, &format!("watch-hang {shown}"));
        }
    } else if line.contains("watch-hang: heartbeat moved again") {
        write(state, t, "watch-hang: heartbeat moving again");
    }
}

/// The last line: how the game ended.
pub fn exit(reason: &str) {
    if let Some(state) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        write(state, None, &format!("exit: {reason}"));
    }
}
