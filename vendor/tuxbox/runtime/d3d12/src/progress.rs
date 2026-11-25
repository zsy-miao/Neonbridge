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
mod tests {
    use super::*;

    fn idle_for(idle_ms: u64) -> Snapshot {
        Snapshot { idle_ms: Some(idle_ms), ..Snapshot::default() }
    }

    #[test]
    fn mode_parses_the_documented_values() {
        assert_eq!(Mode::parse(None), Mode::Auto);
        assert_eq!(Mode::parse(Some("auto")), Mode::Auto);
        assert_eq!(Mode::parse(Some("always")), Mode::Always);
        assert_eq!(Mode::parse(Some("0")), Mode::Off);
        assert_eq!(Mode::parse(Some("off")), Mode::Off);
        assert_eq!(Mode::parse(Some("nonsense")), Mode::Auto);
    }

    #[test]
    fn auto_shows_while_working_and_lingers_then_hides() {
        assert!(!visible(Mode::Auto, &Snapshot::default()));
        let translating = Snapshot { in_flight: 1, ..idle_for(LINGER_MS * 2) };
        let building = Snapshot { building: 1, ..idle_for(LINGER_MS * 2) };
        assert!(visible(Mode::Auto, &translating));
        assert!(visible(Mode::Auto, &building));
        assert!(visible(Mode::Auto, &idle_for(LINGER_MS - 1)));
        assert!(!visible(Mode::Auto, &idle_for(LINGER_MS)));
    }

    #[test]
    fn always_and_off_ignore_activity() {
        assert!(visible(Mode::Always, &Snapshot::default()));
        assert!(!visible(Mode::Off, &Snapshot { building: 9, ..idle_for(0) }));
    }

    #[test]
    fn a_shader_is_counted_once() {
        let mut shaders = Shaders::default();
        assert!(shaders.record("a", Shader::Translated));
        assert!(!shaders.record("a", Shader::Cached));
        assert!(shaders.record("b", Shader::Cached));
        assert!(!shaders.record("b", Shader::Cached));
        assert!(shaders.record("c", Shader::Unsupported));
        assert_eq!(shaders.counts.compiled(), 2);
        assert_eq!(shaders.counts.translated, 1);
        assert_eq!(shaders.counts.cached, 1);
        assert_eq!(shaders.counts.unsupported, 1);
    }

    #[test]
    fn a_late_result_replaces_a_timeout() {
        let mut shaders = Shaders::default();
        assert!(shaders.record("a", Shader::TimedOut));
        assert!(!shaders.record("a", Shader::TimedOut));
        assert!(shaders.record("a", Shader::Translated));
        assert_eq!(shaders.counts, ShaderCounts { translated: 1, ..ShaderCounts::default() });
    }

    #[test]
    fn numbers_are_grouped_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1234567), "1,234,567");
    }

    #[test]
    fn lines_describe_the_counts() {
        let text = lines(&Snapshot {
            in_flight: 3,
            building: 2,
            built: 5120,
            failed: 32,
            stand_ins: 1024,
            shaders: ShaderCounts { translated: 898, cached: 336, unsupported: 12, timed_out: 0 },
            idle_ms: Some(0),
        });
        assert_eq!(text[0], "TuxBox  |  translating 3, building 2");
        assert_eq!(text[1], "1,234 shaders compiled  (898 new)");
        assert_eq!(text[2], "12 unsupported");
        assert_eq!(text[3], "pipelines 5,120 | failed 32 | empty 1,024");
        assert_eq!(lines(&Snapshot::default())[0], "TuxBox  |  idle");
    }

    #[test]
    fn lines_fit_the_panel_with_large_counts() {
        let busy = Snapshot {
            in_flight: 32,
            building: 32,
            built: 99_999,
            failed: 9_999,
            stand_ins: 99_999,
            shaders: ShaderCounts { translated: 99_999, cached: 0, unsupported: 99_999, timed_out: 999 },
            idle_ms: Some(0),
        };
        // 14 px fixed-pitch text across a 380 px panel holds about 49 columns.
        for line in lines(&busy) {
            assert!(line.len() <= 49, "{} columns: {line}", line.len());
        }
    }
}
