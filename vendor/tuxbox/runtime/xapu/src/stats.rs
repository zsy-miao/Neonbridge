//! Diagnostics: every 10 s of command traffic, one line saying whether the
//! title feeds its Opus streams in real time. A stream that receives less
//! than a second of decoded audio per second is being starved by its caller
//! (the gap column names the stall); decode cost shows a slow provider.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
struct Window {
    started: Option<Instant>,
    last: Option<Instant>,
    commands: [u32; 6],
    decode_ms: f64,
    slowest_ms: f64,
    longest_gap_ms: f64,
    gaps_over_40: u32,
    queue_full: u32,
    failed: u32,
    frames: HashMap<(usize, usize), u64>,
    threads: Vec<u32>,
}

static WINDOW: Mutex<Option<Window>> = Mutex::new(None);
static DEQUEUED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static PENDING: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// XApuDequeueResult: a result handed back, or PENDING (nothing queued).
pub(super) fn dequeued(pending: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    if pending { PENDING.fetch_add(1, Relaxed); } else { DEQUEUED.fetch_add(1, Relaxed); }
}

fn take_dequeues() -> String {
    use std::sync::atomic::Ordering::Relaxed;
    format!("dequeued {}, PENDING returned {}", DEQUEUED.swap(0, Relaxed), PENDING.swap(0, Relaxed))
}

/// chad (XDsp) counters: one line per 10 s of submits.
pub(super) mod dsp {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
    pub(crate) static ACTIVATES: AtomicU32 = AtomicU32::new(0);
    pub(crate) static DEACTIVATES: AtomicU32 = AtomicU32::new(0);
    pub(crate) static SUBMITTED: AtomicU32 = AtomicU32::new(0);
    pub(crate) static REFUSED: AtomicU32 = AtomicU32::new(0);
    pub(crate) static SHORTCUT: AtomicU32 = AtomicU32::new(0);
    pub(crate) static INVALID: AtomicU32 = AtomicU32::new(0);
    static STARTED: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn note() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let started = STARTED.load(Relaxed);
        if started == 0 {
            STARTED.store(now, Relaxed);
            return;
        }
        if now - started < 10_000 || STARTED.compare_exchange(started, now, Relaxed, Relaxed).is_err() {
            return;
        }
        let line = format!(
            "TuxBox chad: last {:.1} s: {} activates, {} deactivates, {} commands completed ({} with silent output for a transform not computed; TUXBOX_CHAD_STRICT=1 refuses them), {} refused, {} invalid",
            (now - started) as f64 / 1e3, ACTIVATES.swap(0, Relaxed), DEACTIVATES.swap(0, Relaxed),
            SUBMITTED.swap(0, Relaxed), SHORTCUT.swap(0, Relaxed), REFUSED.swap(0, Relaxed), INVALID.swap(0, Relaxed));
        if std::env::var_os("TUXBOX_LOG").is_some_and(|v| v == "stderr") {
            eprintln!("{line}");
        }
    }
}

fn log(line: &str) {
    if std::env::var_os("TUXBOX_LOG").is_some_and(|v| v == "stderr") {
        eprintln!("TuxBox xapu: {line}");
    }
    let text = format!("TuxBox xapu: {line}\n\0");
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(text.as_ptr()) };
}

pub(super) fn note(handle: usize, stream: usize, kind: u32, ms: f64, frames: usize, queue_full: bool, failed: bool) {
    let mut guard = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let window = guard.get_or_insert_with(Window::default);
    let now = Instant::now();
    let started = *window.started.get_or_insert(now);
    if let Some(last) = window.last.replace(now) {
        let gap = now.duration_since(last).as_secs_f64() * 1e3;
        window.longest_gap_ms = window.longest_gap_ms.max(gap);
        window.gaps_over_40 += u32::from(gap > 40.0);
    }
    window.commands[(kind as usize).min(5)] += 1;
    window.decode_ms += ms;
    window.slowest_ms = window.slowest_ms.max(ms);
    window.queue_full += u32::from(queue_full);
    window.failed += u32::from(failed);
    if frames != 0 {
        *window.frames.entry((handle, stream)).or_insert(0) += frames as u64;
    }
    let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    if !window.threads.contains(&thread) {
        window.threads.push(thread);
    }
    let seconds = now.duration_since(started).as_secs_f64();
    if seconds < 10.0 {
        return;
    }
    // Seconds of 48 kHz audio each decoding stream received per second of wall time.
    let mut rates: Vec<f64> = window.frames.values().map(|&f| f as f64 / 48_000.0 / seconds).collect();
    rates.sort_unstable_by(f64::total_cmp);
    let rate = |i: usize| rates.get(i).copied().unwrap_or(0.0);
    log(&format!(
        "last {seconds:.1} s: {} decodes, {} starts, {} stops, {} resets, {} other; {} streams decoding, audio s per wall s: min {:.2}, median {:.2}, max {:.2}; decode {:.1} ms total, slowest {:.2} ms; longest gap between commands {:.0} ms, {} gaps over 40 ms; queue full {}, failed {}; {}; calling threads {:?}",
        window.commands[3], window.commands[1], window.commands[2], window.commands[5],
        window.commands[0] + window.commands[4], rates.len(), rate(0), rate(rates.len() / 2),
        rate(rates.len().saturating_sub(1)), window.decode_ms, window.slowest_ms, window.longest_gap_ms,
        window.gaps_over_40, window.queue_full, window.failed, take_dequeues(), window.threads));
    *window = Window { last: Some(now), started: Some(now), ..Default::default() };
}
