//! Boot splash logic that needs no operating system: the phases of a start, the state machine that
//! decides when the splash fades, the bar, the letterbox and the image choice.
//!
//! Starting a title takes a long while (reading and patching the executable, mapping it, the
//! caches, then menus that may present black frames). The window that shows the splash lives in
//! `victory-gpu` (Windows only); everything it decides is here so that it is unit-tested anywhere.
//!
//! The picture is never part of this repository: it is read at run time from the user's own game
//! folder (`MicrosoftGame.config`, see [`image_candidates`]). Without one the splash is a dark
//! background and the progress bar.
//!
//! Environment: `VICTORY_SPLASH=0` turns the splash off, `VICTORY_SPLASH_MAX_S` is how long it may
//! stay before it fades by itself (default 180).

use std::sync::atomic::{AtomicU8, Ordering};

/// Seconds the fade-out takes.
pub const FADE_SECS: f64 = 0.6;
/// Default seconds before the splash fades without any sign of content.
pub const DEFAULT_MAX_SECS: f64 = 180.0;
/// The presented frame is sampled on every n-th present.
pub const SAMPLE_EVERY: u64 = 6;
/// Consecutive bright samples that count as content.
pub const NEEDED: u32 = 5;
/// Mean luminance (0..1 as presented) above which a sample is bright.
pub const LUMA_MIN: f32 = 0.03;

// ---------------------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------------------

/// `VICTORY_SPLASH`'s value: everything but `0` (and `off`, `false`) leaves the splash on.
pub fn parse_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// `VICTORY_SPLASH_MAX_S`'s value: a positive number of seconds, else the default.
pub fn parse_max_secs(value: Option<&str>) -> f64 {
    value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(DEFAULT_MAX_SECS)
}

/// Whether the splash is on, from the environment.
pub fn enabled() -> bool {
    parse_enabled(std::env::var("VICTORY_SPLASH").ok().as_deref())
}

/// Seconds the splash may stay, from the environment.
pub fn max_secs() -> f64 {
    parse_max_secs(std::env::var("VICTORY_SPLASH_MAX_S").ok().as_deref())
}

// ---------------------------------------------------------------------------------------
// boot phases
// ---------------------------------------------------------------------------------------

/// Where the start is, in order. The runtime reports it with [`set_phase`]; the bar advances by it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum BootPhase {
    /// The process has begun.
    Start = 0,
    /// Reading and patching the title's executable.
    Reading = 1,
    /// Opening the shader and pipeline caches.
    Cache = 2,
    /// Mapping the program.
    Mapping = 3,
    /// Mapping the title's DLLs and binding imports.
    Dlls = 4,
    /// The title runs (its own startup, menus).
    Starting = 5,
}

impl BootPhase {
    fn from_u8(v: u8) -> BootPhase {
        match v {
            1 => BootPhase::Reading,
            2 => BootPhase::Cache,
            3 => BootPhase::Mapping,
            4 => BootPhase::Dlls,
            5 => BootPhase::Starting,
            _ => BootPhase::Start,
        }
    }

    /// The bar's range while in this phase, and the seconds it takes to cover most of it (the
    /// time of a phase is not known, so the bar creeps towards the end of its range).
    fn range(self) -> (f32, f32, f64) {
        match self {
            BootPhase::Start => (0.0, 0.03, 2.0),
            BootPhase::Reading => (0.03, 0.30, 8.0),
            BootPhase::Cache => (0.30, 0.35, 3.0),
            BootPhase::Mapping => (0.35, 0.60, 8.0),
            BootPhase::Dlls => (0.60, 0.70, 4.0),
            BootPhase::Starting => (0.70, 0.97, 25.0),
        }
    }
}

static PHASE: AtomicU8 = AtomicU8::new(0);

/// The runtime reports a phase of the start (never goes back).
pub fn set_phase(phase: BootPhase) {
    PHASE.fetch_max(phase as u8, Ordering::Relaxed);
}

/// The phase reported last.
pub fn phase() -> BootPhase {
    BootPhase::from_u8(PHASE.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------------------
// state machine
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Showing,
    /// Fading since this time.
    Fading(f64),
    Done,
}

/// What the window shows now.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct View {
    /// Opacity of the whole splash, 1 shown .. 0 gone.
    pub alpha: f32,
    /// The bar's fill, 0..1.
    pub progress: f32,
}

/// Why the splash began to fade.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cause {
    Content,
    Input,
    Timeout,
}

#[derive(Debug)]
pub struct Splash {
    state: State,
    start: f64,
    max_secs: f64,
    streak: u32,
    frames: u64,
    phase: BootPhase,
    phase_since: f64,
    last_tick: f64,
    shown: f32,
    cause: Option<Cause>,
}

impl Splash {
    pub fn new(now: f64, max_secs: f64) -> Self {
        Splash {
            state: State::Showing,
            start: now,
            max_secs,
            streak: 0,
            frames: 0,
            phase: BootPhase::Start,
            phase_since: now,
            last_tick: now,
            shown: 0.0,
            cause: None,
        }
    }

    /// A frame was presented: is its content to be sampled?
    pub fn on_frame(&mut self) -> bool {
        if self.state != State::Showing {
            return false;
        }
        let due = self.frames % SAMPLE_EVERY == 0;
        self.frames += 1;
        due
    }

    /// The mean luminance of a sampled frame: [`NEEDED`] bright ones in a row fade the splash.
    pub fn on_luma(&mut self, luma: f32, now: f64) {
        if self.state != State::Showing {
            return;
        }
        self.streak = if luma > LUMA_MIN { self.streak + 1 } else { 0 };
        if self.streak >= NEEDED {
            self.fade(now, Cause::Content);
        }
    }

    /// A key or button was pressed.
    pub fn on_input(&mut self, now: f64) {
        if self.state == State::Showing {
            self.fade(now, Cause::Input);
        }
    }

    fn fade(&mut self, now: f64, cause: Cause) {
        self.state = State::Fading(now);
        self.cause = Some(cause);
    }

    /// Why it faded, once it has begun.
    pub fn cause(&self) -> Option<Cause> {
        self.cause
    }

    /// Advance to `now` with the runtime at `phase`; `None` once finished.
    pub fn tick(&mut self, now: f64, phase: BootPhase) -> Option<View> {
        if self.state == State::Showing && now - self.start >= self.max_secs {
            self.fade(now, Cause::Timeout);
        }
        if phase > self.phase {
            self.phase = phase;
            self.phase_since = now;
        }
        let dt = (now - self.last_tick).max(0.0);
        self.last_tick = now;
        // the bar eases towards where the phase says it should be, and never goes back
        let target = match self.state {
            State::Showing => {
                let (lo, hi, tau) = self.phase.range();
                lo + (hi - lo) * 0.9 * (1.0 - (-(now - self.phase_since) / tau).exp() as f32)
            }
            _ => 1.0,
        };
        let follow = (dt * 6.0).min(1.0) as f32;
        self.shown = self.shown.max(self.shown + (target - self.shown) * follow);
        match self.state {
            State::Showing => Some(View { alpha: 1.0, progress: self.shown }),
            State::Fading(since) => {
                let t = ((now - since) / FADE_SECS).clamp(0.0, 1.0) as f32;
                if t >= 1.0 {
                    self.state = State::Done;
                    return None;
                }
                Some(View { alpha: 1.0 - t * t * (3.0 - 2.0 * t), progress: self.shown })
            }
            State::Done => None,
        }
    }

    pub fn finished(&self) -> bool {
        self.state == State::Done
    }
}

/// Whether a key or button went down since the last poll: `now` and `before` are the state of each
/// (the keyboard, the mouse buttons, a pad's buttons). Keys held at the start do not count.
pub fn new_press(before: &mut [bool], now: &[bool]) -> bool {
    let mut pressed = false;
    for (b, &n) in before.iter_mut().zip(now) {
        pressed |= n && !*b;
        *b = n;
    }
    pressed
}

// ---------------------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------------------

/// The largest rectangle of the image's aspect that fits the client area, centered:
/// (x, y, width, height) in pixels.
pub fn letterbox(img_w: u32, img_h: u32, dst_w: u32, dst_h: u32) -> (i32, i32, i32, i32) {
    if img_w == 0 || img_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w as i32, dst_h as i32);
    }
    let scale = (dst_w as f64 / img_w as f64).min(dst_h as f64 / img_h as f64);
    let w = ((img_w as f64 * scale).round() as i32).clamp(1, dst_w as i32);
    let h = ((img_h as f64 * scale).round() as i32).clamp(1, dst_h as i32);
    ((dst_w as i32 - w) / 2, (dst_h as i32 - h) / 2, w, h)
}

/// A rectangle as (left, top, right, bottom).
pub type Rect = (i32, i32, i32, i32);

/// The progress bar in a `w` x `h` window: the track, the filled part and a moving glint on the
/// fill (so the bar shows life while a phase takes long). A thin bar low in the window.
pub fn bar_rects(w: i32, h: i32, progress: f32, t: f64) -> (Rect, Rect, Rect) {
    let thick = (h / 180).clamp(3, 8);
    let left = w / 5;
    let right = w - w / 5;
    let top = h - h / 12 - thick;
    let track = (left, top, right, top + thick);
    let fill_to = left + ((right - left) as f32 * progress.clamp(0.0, 1.0)).round() as i32;
    let fill = (left, top, fill_to, top + thick);
    // the glint sweeps across the fill once every 1.6 s
    let width = ((right - left) / 8).max(8);
    let phase = (t / 1.6).fract() as f32;
    let at = left - width + ((fill_to - left + width) as f32 * phase) as i32;
    let glint = (at.max(left), top, (at + width).min(fill_to), top + thick);
    let glint = if glint.2 > glint.0 { glint } else { (left, top, left, top + thick) };
    (track, fill, glint)
}

// ---------------------------------------------------------------------------------------
// the picture
// ---------------------------------------------------------------------------------------

/// `name="value"` in the config's text (the attribute after a space).
fn attribute<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = text.find(&key)? + key.len();
    let v = &text[i..i + text[i..].find('"')?];
    (!v.is_empty()).then_some(v)
}

/// Image files of `MicrosoftGame.config`'s `ShellVisuals`, best first: the splash screen, then the
/// logos from largest to smallest.
pub fn image_candidates(config: &str) -> Vec<String> {
    ["SplashScreenImage", "Square480x480Logo", "StoreLogo", "Square150x150Logo", "Square44x44Logo"]
        .iter()
        .filter_map(|n| attribute(config, n))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------------------
// sampling the presented frame
// ---------------------------------------------------------------------------------------

/// How a sampled texel is laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Texel {
    Rgba8,
    Bgra8,
    Rgb10A2,
    Rgba16F,
}

impl Texel {
    /// The layout of a DXGI format number (typeless and sRGB variants included), if sampled.
    pub fn from_dxgi(format: u32) -> Option<Texel> {
        match format {
            24 | 25 => Some(Texel::Rgb10A2),
            9 | 10 => Some(Texel::Rgba16F),
            27..=29 => Some(Texel::Rgba8),
            87 | 88 | 90 | 91 => Some(Texel::Bgra8),
            _ => None,
        }
    }
}

fn half(h: u16) -> f32 {
    let (e, m) = ((h >> 10 & 0x1F) as i32, (h & 0x3FF) as f32);
    let v = match e {
        0 => m * 2f32.powi(-24),
        31 => 1.0,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    };
    if h & 0x8000 != 0 { 0.0 } else { v }
}

/// Luminance 0..1 of the texel at the start of `b` (4 bytes, 8 for [`Texel::Rgba16F`]).
pub fn texel_luma(kind: Texel, b: &[u8]) -> f32 {
    let rgb = if kind == Texel::Rgba16F {
        if b.len() < 8 {
            return 0.0;
        }
        let r = |i: usize| half(u16::from_le_bytes([b[i], b[i + 1]])).min(1.0);
        [r(0), r(2), r(4)]
    } else {
        if b.len() < 4 {
            return 0.0;
        }
        let t = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        match kind {
            Texel::Rgb10A2 => [
                (t & 0x3FF) as f32 / 1023.0,
                (t >> 10 & 0x3FF) as f32 / 1023.0,
                (t >> 20 & 0x3FF) as f32 / 1023.0,
            ],
            Texel::Bgra8 => [
                (t >> 16 & 0xFF) as f32 / 255.0,
                (t >> 8 & 0xFF) as f32 / 255.0,
                (t & 0xFF) as f32 / 255.0,
            ],
            _ => [
                (t & 0xFF) as f32 / 255.0,
                (t >> 8 & 0xFF) as f32 / 255.0,
                (t >> 16 & 0xFF) as f32 / 255.0,
            ],
        }
    };
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: f64 = DEFAULT_MAX_SECS;

    fn bright(s: &mut Splash, n: u32, t: f64) {
        for _ in 0..n {
            s.on_luma(0.5, t);
        }
    }

    fn alpha(s: &mut Splash, t: f64) -> Option<f32> {
        s.tick(t, BootPhase::Starting).map(|v| v.alpha)
    }

    #[test]
    fn shows_until_enough_consecutive_bright_samples() {
        let mut s = Splash::new(0.0, MAX);
        bright(&mut s, NEEDED - 1, 1.0);
        assert_eq!(alpha(&mut s, 1.0), Some(1.0));
        s.on_luma(0.0, 1.0); // a dark frame resets the streak
        bright(&mut s, NEEDED - 1, 2.0);
        assert_eq!(alpha(&mut s, 2.0), Some(1.0));
        s.on_luma(0.5, 2.0);
        assert_eq!(s.cause(), Some(Cause::Content));
        assert_eq!(alpha(&mut s, 2.0), Some(1.0)); // the fade has just begun
        assert!(alpha(&mut s, 2.3).unwrap() < 1.0);
    }

    #[test]
    fn threshold_is_exclusive_and_dark_frames_never_fade() {
        let mut s = Splash::new(0.0, MAX);
        for _ in 0..100 {
            s.on_luma(LUMA_MIN, 1.0);
        }
        assert_eq!(alpha(&mut s, 1.0), Some(1.0));
        assert_eq!(s.cause(), None);
    }

    #[test]
    fn fade_is_smooth_monotonic_and_ends() {
        let mut s = Splash::new(0.0, MAX);
        bright(&mut s, NEEDED, 10.0);
        let mut last = 1.0;
        let mut t = 10.0;
        while t < 10.0 + FADE_SECS - 0.02 {
            let a = alpha(&mut s, t).unwrap();
            assert!(a <= last && (0.0..=1.0).contains(&a));
            last = a;
            t += 0.01;
        }
        assert!(last < 0.05);
        assert_eq!(alpha(&mut s, 10.0 + FADE_SECS + 0.01), None);
        assert!(s.finished());
        assert_eq!(alpha(&mut s, 11.0), None);
        // a finished splash samples nothing and ignores input
        assert!(!s.on_frame());
        s.on_input(12.0);
        assert_eq!(s.cause(), Some(Cause::Content));
    }

    #[test]
    fn fades_after_the_timeout_without_content() {
        let mut s = Splash::new(5.0, 30.0);
        assert_eq!(alpha(&mut s, 5.0 + 29.9), Some(1.0));
        assert_eq!(alpha(&mut s, 5.0 + 30.0), Some(1.0));
        assert_eq!(s.cause(), Some(Cause::Timeout));
        assert!(alpha(&mut s, 5.0 + 30.3).unwrap() < 1.0);
        assert_eq!(alpha(&mut s, 5.0 + 30.0 + FADE_SECS + 0.1), None);
    }

    #[test]
    fn input_fades_it_once() {
        let mut s = Splash::new(0.0, MAX);
        assert_eq!(alpha(&mut s, 1.0), Some(1.0));
        s.on_input(2.0);
        s.on_input(2.4); // a second press does not restart the fade
        assert_eq!(s.cause(), Some(Cause::Input));
        assert!(alpha(&mut s, 2.3).unwrap() < 1.0);
        assert_eq!(alpha(&mut s, 2.0 + FADE_SECS + 0.01), None);
    }

    #[test]
    fn samples_every_nth_frame_and_stops_when_fading() {
        let mut s = Splash::new(0.0, MAX);
        let due: Vec<bool> = (0..2 * SAMPLE_EVERY).map(|_| s.on_frame()).collect();
        assert_eq!(due.iter().filter(|d| **d).count(), 2);
        assert!(due[0] && due[SAMPLE_EVERY as usize]);
        bright(&mut s, NEEDED, 1.0);
        assert!((0..20).all(|_| !s.on_frame()));
    }

    #[test]
    fn the_bar_advances_by_phase_and_never_goes_back() {
        let mut s = Splash::new(0.0, MAX);
        let mut t = 0.0;
        let mut last = 0.0;
        for phase in [
            BootPhase::Start,
            BootPhase::Reading,
            BootPhase::Cache,
            BootPhase::Mapping,
            BootPhase::Dlls,
            BootPhase::Starting,
        ] {
            let mut at_end = 0.0;
            for _ in 0..600 {
                t += 0.02;
                let v = s.tick(t, phase).unwrap();
                assert!(v.progress >= last, "{phase:?}: {} < {last}", v.progress);
                assert!(v.progress < 1.0);
                last = v.progress;
                at_end = v.progress;
            }
            let (lo, hi, _) = phase.range();
            assert!(at_end > lo && at_end <= hi, "{phase:?} ends at {at_end}");
        }
        // a late report of an earlier phase changes nothing
        let v = s.tick(t + 0.02, BootPhase::Reading).unwrap();
        assert!(v.progress >= last);
        // fading completes the bar
        s.on_input(t + 0.1);
        let mut v = s.tick(t + 0.1, BootPhase::Starting).unwrap();
        for k in 1..10 {
            v = s.tick(t + 0.1 + k as f64 * 0.05, BootPhase::Starting).unwrap();
        }
        assert!(v.progress > 0.99);
    }

    #[test]
    fn the_reported_phase_only_moves_forward() {
        set_phase(BootPhase::Mapping);
        set_phase(BootPhase::Reading);
        assert!(phase() >= BootPhase::Mapping);
    }

    #[test]
    fn presses_are_edges() {
        let mut before = [false; 4];
        assert!(!new_press(&mut before, &[false, false, false, false]));
        assert!(new_press(&mut before, &[false, true, false, false]));
        // held: not a new press
        assert!(!new_press(&mut before, &[false, true, false, false]));
        // released, then pressed again
        assert!(!new_press(&mut before, &[false; 4]));
        assert!(new_press(&mut before, &[true, false, false, false]));
        // a key held from before the splash existed does not count
        let mut held = [true, false];
        assert!(!new_press(&mut held, &[true, false]));
    }

    #[test]
    fn settings_from_the_environment() {
        assert!(parse_enabled(None));
        assert!(parse_enabled(Some("1")));
        assert!(parse_enabled(Some("")));
        assert!(!parse_enabled(Some("0")));
        assert!(!parse_enabled(Some(" 0 ")));
        assert!(!parse_enabled(Some("off")));
        assert_eq!(parse_max_secs(None), 180.0);
        assert_eq!(parse_max_secs(Some("45")), 45.0);
        assert_eq!(parse_max_secs(Some(" 2.5 ")), 2.5);
        for bad in ["0", "-3", "abc", "", "NaN", "inf"] {
            assert_eq!(parse_max_secs(Some(bad)), 180.0, "{bad}");
        }
    }

    #[test]
    fn letterbox_math() {
        assert_eq!(letterbox(1920, 1080, 1280, 720), (0, 0, 1280, 720));
        // wide window: pillarbox
        assert_eq!(letterbox(1920, 1080, 1920, 800), (249, 0, 1422, 800));
        // tall window: bars above and below
        assert_eq!(letterbox(1920, 1080, 800, 800), (0, 175, 800, 450));
        // square image in a wide window
        assert_eq!(letterbox(480, 480, 1600, 900), (350, 0, 900, 900));
        // upscales small images; degenerate sizes fill the area
        assert_eq!(letterbox(100, 50, 400, 400), (0, 100, 400, 200));
        assert_eq!(letterbox(0, 0, 640, 360), (0, 0, 640, 360));
    }

    #[test]
    fn the_bar_is_thin_low_and_inside_the_window() {
        let (track, fill, _) = bar_rects(1920, 1080, 0.5, 0.0);
        assert_eq!(track.0, 384);
        assert_eq!(track.2, 1536);
        assert!(track.3 - track.1 <= 8 && track.3 - track.1 >= 3);
        assert!(track.3 < 1080 && track.1 > 1080 * 3 / 4);
        assert_eq!(fill.2, 384 + 576);
        for t in [0.0, 0.4, 0.8, 1.2, 1.59, 5.0] {
            let (_, fill, glint) = bar_rects(1920, 1080, 0.5, t);
            assert!(glint.0 >= fill.0 && glint.2 <= fill.2, "glint {glint:?} outside {fill:?}");
        }
        // an empty bar has an empty glint; a full one is filled to the track's end
        let (_, fill, glint) = bar_rects(800, 600, 0.0, 0.7);
        assert_eq!(fill.0, fill.2);
        assert_eq!(glint.0, glint.2);
        let (track, fill, _) = bar_rects(800, 600, 2.0, 0.0);
        assert_eq!(fill.2, track.2);
    }

    #[test]
    fn picks_the_splash_then_logos() {
        let cfg = r##"<ShellVisuals DefaultDisplayName="G" Square150x150Logo="a150.png" Square480x480Logo="a480.png" SplashScreenImage="splash.png" StoreLogo="store.png" BackgroundColor="#000000" />"##;
        assert_eq!(image_candidates(cfg), ["splash.png", "a480.png", "store.png", "a150.png"]);
        assert_eq!(image_candidates("<ShellVisuals StoreLogo=\"\" Square44x44Logo=\"s.png\" />"), ["s.png"]);
        assert!(image_candidates("<Game />").is_empty());
    }

    #[test]
    fn luma_of_texels() {
        assert_eq!(texel_luma(Texel::Rgba8, &[0, 0, 0, 255]), 0.0);
        assert!((texel_luma(Texel::Rgba8, &[255, 255, 255, 255]) - 1.0).abs() < 1e-4);
        // BGRA: the red channel is the third byte
        assert!((texel_luma(Texel::Bgra8, &[0, 0, 255, 255]) - 0.2126).abs() < 1e-4);
        // half 1.0 = 0x3C00
        let one = 0x3C00u16.to_le_bytes();
        let b = [one[0], one[1], one[0], one[1], one[0], one[1], 0, 0];
        assert!((texel_luma(Texel::Rgba16F, &b) - 1.0).abs() < 1e-3);
        // short input is dark, not a panic
        assert_eq!(texel_luma(Texel::Rgba16F, &[1, 2]), 0.0);
        assert_eq!(texel_luma(Texel::Rgb10A2, &[]), 0.0);
        assert_eq!(Texel::from_dxgi(28), Some(Texel::Rgba8));
        assert_eq!(Texel::from_dxgi(87), Some(Texel::Bgra8));
        assert_eq!(Texel::from_dxgi(10), Some(Texel::Rgba16F));
        assert_eq!(Texel::from_dxgi(2), None);
    }
}
