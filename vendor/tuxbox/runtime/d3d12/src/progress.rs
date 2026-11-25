//! Counts of shader translation and pipeline building, shown by the on-screen
//! progress panel.
//!
//! A shader is counted once, by its request key, however often the title asks
//! for it. A pipeline is counted once per creation call from the title; that
//! call includes any translation wait and the driver's own compile.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

static START: LazyLock<Instant> = LazyLock::new(Instant::now);
static SHADERS: LazyLock<Mutex<Shaders>> = LazyLock::new(Default::default);
static IN_FLIGHT: AtomicU32 = AtomicU32::new(0);
static BUILDING: AtomicU32 = AtomicU32::new(0);
static BUILT: AtomicU32 = AtomicU32::new(0);
static FAILED: AtomicU32 = AtomicU32::new(0);
static STAND_INS: AtomicU32 = AtomicU32::new(0);
/// Milliseconds after `START` of the last change, plus one (zero means none yet).
static LAST_CHANGE: AtomicU64 = AtomicU64::new(0);

/// What happened to one shader request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shader {
    /// Translated while the title waited.
    Translated,
    /// Served from an earlier translation.
    Cached,
    Unsupported,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShaderCounts {
    pub translated: u32,
    pub cached: u32,
    pub unsupported: u32,
    pub timed_out: u32,
}

impl ShaderCounts {
    /// Distinct shaders the title received in compiled form.
    pub(crate) fn compiled(&self) -> u32 {
        self.translated + self.cached
    }

    fn slot(&mut self, shader: Shader) -> &mut u32 {
        match shader {
            Shader::Translated => &mut self.translated,
            Shader::Cached => &mut self.cached,
            Shader::Unsupported => &mut self.unsupported,
            Shader::TimedOut => &mut self.timed_out,
        }
    }
}

/// The distinct shaders seen so far. The first result for a key stands, except
/// that a timed-out request is replaced by whatever it produces later.
#[derive(Default)]
struct Shaders {
    seen: HashMap<String, Shader>,
    counts: ShaderCounts,
}

impl Shaders {
    /// Returns whether anything changed.
    fn record(&mut self, key: &str, shader: Shader) -> bool {
        match self.seen.get(key).copied() {
            None => {
                self.seen.insert(key.to_owned(), shader);
            }
            Some(Shader::TimedOut) if shader != Shader::TimedOut => {
                *self.counts.slot(Shader::TimedOut) -= 1;
                self.seen.insert(key.to_owned(), shader);
            }
            Some(_) => return false,
        }
        *self.counts.slot(shader) += 1;
        true
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// Translations the title is waiting on.
    pub in_flight: u32,
    pub shaders: ShaderCounts,
    /// Pipeline creation calls running now.
    pub building: u32,
    pub built: u32,
    pub failed: u32,
    /// Empty pipelines handed out for shaders that could not be translated.
    pub stand_ins: u32,
    /// Milliseconds since anything last changed, or `None` before any activity.
    pub idle_ms: Option<u64>,
}

impl Snapshot {
    pub(crate) fn working(&self) -> bool {
        self.in_flight > 0 || self.building > 0
    }
}

fn now_ms() -> u64 {
    START.elapsed().as_millis() as u64
}

fn touch() {
    LAST_CHANGE.store(now_ms() + 1, Relaxed);
}

pub(crate) fn record(key: &str, shader: Shader) {
    let changed = SHADERS.lock().unwrap_or_else(|e| e.into_inner()).record(key, shader);
    if changed {
        touch();
    }
}

/// One translation the title is waiting on. Dropping it ends the wait.
pub(crate) struct Waiting(());

pub(crate) fn waiting() -> Waiting {
    IN_FLIGHT.fetch_add(1, Relaxed);
    touch();
    Waiting(())
}

impl Drop for Waiting {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Relaxed);
        touch();
    }
}

/// One pipeline creation call. Dropping it without `finish` counts nothing.
pub(crate) struct Building(Option<bool>);

pub(crate) fn building() -> Building {
    BUILDING.fetch_add(1, Relaxed);
    touch();
    Building(None)
}

impl Building {
    pub(crate) fn finish(mut self, succeeded: bool) {
        self.0 = Some(succeeded);
    }
}

impl Drop for Building {
    fn drop(&mut self) {
        BUILDING.fetch_sub(1, Relaxed);
        match self.0 {
            Some(true) => BUILT.fetch_add(1, Relaxed),
            Some(false) => FAILED.fetch_add(1, Relaxed),
            None => 0,
        };
        touch();
    }
}

pub(crate) fn stand_in() {
    STAND_INS.fetch_add(1, Relaxed);
    touch();
}

pub(crate) fn snapshot() -> Snapshot {
    let last = LAST_CHANGE.load(Relaxed);
    Snapshot {
        in_flight: IN_FLIGHT.load(Relaxed),
        shaders: SHADERS.lock().unwrap_or_else(|e| e.into_inner()).counts,
        building: BUILDING.load(Relaxed),
        built: BUILT.load(Relaxed),
        failed: FAILED.load(Relaxed),
        stand_ins: STAND_INS.load(Relaxed),
        idle_ms: (last != 0).then(|| now_ms().saturating_sub(last - 1)),
    }
}

/// `TUXBOX_PROGRESS_OVERLAY`: unset or `auto` shows the panel while shaders are
/// being translated or pipelines built and briefly afterwards, `always` keeps
/// it on, `0` or `off` hides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Auto,
    Always,
    Off,
}

impl Mode {
    pub(crate) fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("0" | "off" | "false" | "no") => Self::Off,
            Some("always" | "on" | "1") => Self::Always,
            _ => Self::Auto,
        }
    }

    pub(crate) fn from_env() -> Self {
        Self::parse(std::env::var("TUXBOX_PROGRESS_OVERLAY").ok().as_deref())
    }
}

/// How long the panel stays up after the last change.
pub(crate) const LINGER_MS: u64 = 4000;

pub(crate) fn visible(mode: Mode, snapshot: &Snapshot) -> bool {
    match mode {
        Mode::Off => false,
        Mode::Always => true,
        Mode::Auto => snapshot.working() || snapshot.idle_ms.is_some_and(|idle| idle < LINGER_MS),
    }
}

/// Digits in groups of three: 12345 becomes "12,345".
pub(crate) fn grouped(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The panel's lines of text.
pub(crate) fn lines(snapshot: &Snapshot) -> [String; 4] {
    let mut state = Vec::new();
    if snapshot.in_flight > 0 {
        state.push(format!("translating {}", grouped(snapshot.in_flight)));
    }
    if snapshot.building > 0 {
        state.push(format!("building {}", grouped(snapshot.building)));
    }
    if state.is_empty() {
        state.push("idle".to_string());
    }
    let shaders = snapshot.shaders;
    let mut problems = format!("{} unsupported", grouped(shaders.unsupported));
    if shaders.timed_out > 0 {
        problems.push_str(&format!("  |  {} timed out", grouped(shaders.timed_out)));
    }
    [
        format!("TuxBox  |  {}", state.join(", ")),
        format!(
            "{} shaders compiled  ({} new)",
            grouped(shaders.compiled()),
            grouped(shaders.translated)
        ),
        problems,
        format!(
            "pipelines {} | failed {} | empty {}",
            grouped(snapshot.built),
            grouped(snapshot.failed),
            grouped(snapshot.stand_ins)
        ),
    ]
}

#[cfg(test)]
#[path = "tests/progress.rs"]
mod tests;
