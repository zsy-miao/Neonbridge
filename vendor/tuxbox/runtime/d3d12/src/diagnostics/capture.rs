//! One-frame pass report. TUXBOX_CAPTURE_FRAME=N records the work between
//! presents N and N+1 (TUXBOX_CAPTURE_FRAMES=K: K frames from there); `ctl
//! capture [K]` arms the next frame. Each draw, dispatch, submission and
//! GPU memory wait is one JSON line in submission order, with its list, the
//! bound pipeline's key and its outcome: ran, or dropped and why (pipeline
//! still being made, unresolved root view, unresolved descriptor table,
//! refused recording). The lines go to
//! %TEMP%\tuxbox_capture\<pid>-frame<N>\frame.jsonl (TUXBOX_CAPTURE_DIR
//! names the parent) with a summary in the log; `neonbridge replay-shaders`
//! reads the pipeline keys back. Unarmed, a hook costs one atomic load.
//! Idea from leonida 88797f3e (LEONIDA_CAPTURE) and VIctory
//! (VICTORY_DUMP_TARGETS); written here.
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Presents so far.
static PRESENTS: AtomicU64 = AtomicU64::new(0);
/// The present count after which capture starts (0: none armed) and how many frames.
static ARM_AT: AtomicU64 = AtomicU64::new(0);
static ARM_FRAMES: AtomicU64 = AtomicU64::new(1);
static SEQ: AtomicU64 = AtomicU64::new(0);
const MAX_LINES: usize = 400_000;

struct Frame {
    number: u64,
    lines: Vec<String>,
    outcomes: BTreeMap<(&'static str, String), u64>,
    pipelines: BTreeMap<String, (u64, u64)>,
    truncated: u64,
}

static FRAME: Mutex<Option<Frame>> = Mutex::new(None);

fn armed_from_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(n) = crate::env_toggle!("TUXBOX_CAPTURE_FRAME").and_then(|v| v.trim().parse::<u64>().ok()).filter(|n| *n > 0) {
            let frames = crate::env_toggle!("TUXBOX_CAPTURE_FRAMES").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(1).max(1);
            ARM_FRAMES.store(frames, Ordering::Relaxed);
            ARM_AT.store(n, Ordering::Relaxed);
        }
    });
}

/// Whether this frame is being captured.
#[inline]
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// `ctl capture [K]`: capture the next K frames.
pub(crate) fn arm_next(frames: u64) -> String {
    armed_from_env();
    let frames = frames.max(1);
    let at = PRESENTS.load(Ordering::Relaxed);
    if at == 0 {
        // Nothing presented yet: the first frame is captured from now.
        ARM_FRAMES.store(frames - 1, Ordering::Relaxed);
        ARM_AT.store(1, Ordering::Relaxed);
        start(1);
    } else {
        // The frame being recorded is partly gone: start at the next present.
        ARM_FRAMES.store(frames, Ordering::Relaxed);
        ARM_AT.store(at + 1, Ordering::Relaxed);
    }
    format!("capturing {frames} frame(s) after present {at}; the log names the folder\n")
}

fn start(number: u64) {
    let mut frame = FRAME.lock().unwrap_or_else(|e| e.into_inner());
    *frame = Some(Frame { number, lines: Vec::new(), outcomes: BTreeMap::new(), pipelines: BTreeMap::new(), truncated: 0 });
    SEQ.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
}

/// A present completed: close a captured frame, open an armed one.
pub(crate) fn presented() {
    armed_from_env();
    let count = PRESENTS.fetch_add(1, Ordering::Relaxed) + 1;
    if ACTIVE.load(Ordering::Relaxed) {
        finish();
    }
    let at = ARM_AT.load(Ordering::Relaxed);
    if at == 0 || count < at {
        return;
    }
    let frames = ARM_FRAMES.load(Ordering::Relaxed);
    if count < at + frames {
        start(count + 1);
    } else {
        ARM_AT.store(0, Ordering::Relaxed);
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => { let _ = write!(out, "\\u{:04x}", c as u32); }
            c => out.push(c),
        }
    }
    out
}

fn push(kind: &'static str, outcome: &str, line: String, pipeline: Option<&str>) {
    let mut guard = FRAME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(frame) = guard.as_mut() else { return };
    *frame.outcomes.entry((kind, outcome.to_owned())).or_default() += 1;
    if let Some(key) = pipeline {
        let entry = frame.pipelines.entry(key.to_owned()).or_default();
        if outcome == "ran" { entry.0 += 1 } else { entry.1 += 1 }
    }
    if frame.lines.len() < MAX_LINES {
        frame.lines.push(line);
    } else {
        frame.truncated += 1;
    }
}

fn head(kind: &str, list: usize) -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    format!("{{\"seq\":{seq},\"kind\":\"{kind}\",\"thread\":{thread},\"list\":\"{list:#x}\"")
}

/// A draw or dispatch and what became of it.
pub(crate) fn call(kind: &'static str, list: usize, outcome: &str, pipeline: Option<&str>, null_pipeline: bool) {
    let mut line = head(kind, list);
    let _ = write!(line, ",\"outcome\":\"{}\"", escape(outcome));
    match pipeline {
        Some(key) => { let _ = write!(line, ",\"pipeline\":\"{}\"", escape(key)); }
        None => line.push_str(",\"pipeline\":null"),
    }
    if null_pipeline {
        line.push_str(",\"null_pipeline\":true");
    }
    let _ = write!(line, ",\"rva\":\"{:#x}\"}}", crate::survey::title_rva().unwrap_or(0));
    push(kind, outcome, line, pipeline);
}

/// Another event (a submission, a memory wait) with a free-form detail.
pub(crate) fn note(kind: &'static str, list: usize, outcome: &str, detail: std::fmt::Arguments) {
    let mut line = head(kind, list);
    let _ = write!(line, ",\"outcome\":\"{}\",\"detail\":\"{}\"}}", escape(outcome), escape(&detail.to_string()));
    push(kind, outcome, line, None);
}

fn folder(number: u64) -> std::path::PathBuf {
    let parent = std::env::var_os("TUXBOX_CAPTURE_DIR")
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("tuxbox_capture"));
    parent.join(format!("{}-frame{number}", std::process::id()))
}

fn finish() {
    ACTIVE.store(false, Ordering::Relaxed);
    let Some(frame) = FRAME.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    // Written on another thread so the present does not stall on the disk.
    let _ = std::thread::Builder::new().name("TuxBox capture".into()).spawn(move || write(frame));
}

fn write(frame: Frame) {
    let dir = folder(frame.number);
    let mut summary = String::new();
    let _ = writeln!(summary, "frame {}: {} events{}", frame.number, frame.lines.len(),
        if frame.truncated != 0 { format!(" ({} more not written)", frame.truncated) } else { String::new() });
    for ((kind, outcome), n) in &frame.outcomes {
        let _ = writeln!(summary, "  {kind} {outcome}: {n}");
    }
    let mut pipelines = String::new();
    for (key, (ran, dropped)) in &frame.pipelines {
        let _ = writeln!(pipelines, "{{\"pipeline\":\"{}\",\"ran\":{ran},\"dropped\":{dropped}}}", escape(key));
    }
    let result = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("frame.jsonl"), frame.lines.join("\n") + "\n"))
        .and_then(|()| std::fs::write(dir.join("pipelines.jsonl"), pipelines))
        .and_then(|()| std::fs::write(dir.join("summary.txt"), &summary));
    match result {
        Ok(()) => super::log_line(format_args!("captured {} (TUXBOX_CAPTURE_FRAME): {summary}", dir.display())),
        Err(error) => super::log_line(format_args!("frame capture could not write {}: {error}", dir.display())),
    }
}
