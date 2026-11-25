//! Keyboard and mouse played the way PC games are: keys to pad buttons and
//! sticks, the mouse to the right stick, mouse buttons to the triggers.
//!
//! the title reads gamepads and keyboards only (it registers no
//! mouse), and turns its right stick into a turn rate. Mouse counts are a
//! distance, so they become a rate when the title reads the pad: the counts
//! since its last read over the time since then. The camera then turns the
//! same angle for the same hand movement at any frame rate.
//!
//! All of this is plain data: the host side of `gameinput` feeds it and the
//! title's reads drain it. `[controls]` in victory.toml sets and rebinds it.

use std::collections::{BTreeMap, VecDeque};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

use serde::Deserialize;

/// Pad button bits, as the title's gamepad readings carry them.
pub const MENU: u32 = 0x1;
pub const VIEW: u32 = 0x2;
pub const A: u32 = 0x4;
pub const B: u32 = 0x8;
pub const X: u32 = 0x10;
pub const Y: u32 = 0x20;
pub const UP: u32 = 0x40;
pub const DOWN: u32 = 0x80;
pub const LEFT: u32 = 0x100;
pub const RIGHT: u32 = 0x200;
pub const LEFT_SHOULDER: u32 = 0x400;
pub const RIGHT_SHOULDER: u32 = 0x800;
pub const LEFT_STICK: u32 = 0x1000;
pub const RIGHT_STICK: u32 = 0x2000;

/// A gamepad reading: triggers 0 to 1, sticks -1 to 1 (up positive).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
}

const _: () = assert!(size_of::<PadState>() == 28);

impl PadState {
    /// Nothing pressed or deflected.
    pub fn is_idle(&self) -> bool {
        *self == PadState::default()
    }

    /// `self` and `other` as one pad: buttons OR, triggers max, and each
    /// stick from `other` when it is deflected past `PAD_TAKES_OVER`, else
    /// from `self`. (A source that is only resting never takes a stick.)
    pub fn merged(self, other: PadState) -> PadState {
        let pushed = |x: f32, y: f32| x.hypot(y) > PAD_TAKES_OVER;
        let (lx, ly) = if pushed(other.left_x, other.left_y) {
            (other.left_x, other.left_y)
        } else {
            (self.left_x, self.left_y)
        };
        let (rx, ry) = if pushed(other.right_x, other.right_y) {
            (other.right_x, other.right_y)
        } else {
            (self.right_x, self.right_y)
        };
        PadState {
            buttons: self.buttons | other.buttons,
            left_trigger: self.left_trigger.max(other.left_trigger),
            right_trigger: self.right_trigger.max(other.right_trigger),
            left_x: lx,
            left_y: ly,
            right_x: rx,
            right_y: ry,
        }
    }
}

/// Key codes are Windows virtual keys; mouse buttons are theirs too (1, 2,
/// 4, 5, 6) and the wheel takes two codes Windows leaves unassigned.
pub const WHEEL_UP: u8 = 0x0E;
pub const WHEEL_DOWN: u8 = 0x0F;
/// Mouse buttons by virtual key: left, right, middle, back, forward.
pub const MOUSE_BUTTONS: [u8; 5] = [0x01, 0x02, 0x04, 0x05, 0x06];

/// Mouse counts per second that make a full stick at sensitivity 1.
const FULL_RATE: f64 = 2000.0;
/// Title reads closer together than this see the same mouse stick.
const MIN_TAKE_US: u64 = 2_000;
/// The first read's time span, and the longest one (a title that paused;
/// the title's world frames take 1-2 s here for now, measured).
const FIRST_TAKE_US: u64 = 16_667;
const MAX_TAKE_US: u64 = 1_000_000;
/// Counts past a full stick wait at most this long (seconds of full stick).
const CARRY_S: f64 = 0.25;
/// A pad stick past this takes its stick back from the keyboard and mouse.
/// Detection only: the pad's own value is passed on as it is.
const PAD_TAKES_OVER: f32 = 0.2;
/// Wheel steps waiting to be pressed.
const WHEEL_QUEUE: usize = 8;

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Button(u32),
    LeftTrigger,
    RightTrigger,
    /// One stick direction.
    Stick {
        right: bool,
        x: bool,
        positive: bool,
    },
    /// While held, the left stick moves at the walk speed.
    Walk,
    /// Switches between the on-foot and the vehicle layout.
    ToggleVehicle,
    /// Frees the mouse cursor (a click captures it again).
    ReleaseCursor,
}

const fn stick(right: bool, x: bool, positive: bool) -> Output {
    Output::Stick { right, x, positive }
}

const OUTPUTS: [(&str, Output); 27] = [
    ("A", Output::Button(A)),
    ("B", Output::Button(B)),
    ("X", Output::Button(X)),
    ("Y", Output::Button(Y)),
    ("LB", Output::Button(LEFT_SHOULDER)),
    ("RB", Output::Button(RIGHT_SHOULDER)),
    ("LT", Output::LeftTrigger),
    ("RT", Output::RightTrigger),
    ("LS", Output::Button(LEFT_STICK)),
    ("RS", Output::Button(RIGHT_STICK)),
    ("View", Output::Button(VIEW)),
    ("Menu", Output::Button(MENU)),
    ("DpadUp", Output::Button(UP)),
    ("DpadDown", Output::Button(DOWN)),
    ("DpadLeft", Output::Button(LEFT)),
    ("DpadRight", Output::Button(RIGHT)),
    ("LeftUp", stick(false, false, true)),
    ("LeftDown", stick(false, false, false)),
    ("LeftLeft", stick(false, true, false)),
    ("LeftRight", stick(false, true, true)),
    ("RightUp", stick(true, false, true)),
    ("RightDown", stick(true, false, false)),
    ("RightLeft", stick(true, true, false)),
    ("RightRight", stick(true, true, true)),
    ("Walk", Output::Walk),
    ("ToggleVehicle", Output::ToggleVehicle),
    ("ReleaseCursor", Output::ReleaseCursor),
];

/// Key names besides single letters and digits, `F1`-`F24` and `Num0`-`Num9`.
const KEYS: [(&str, u8); 48] = [
    ("MouseLeft", 0x01),
    ("MouseRight", 0x02),
    ("MouseMiddle", 0x04),
    ("Mouse4", 0x05),
    ("Mouse5", 0x06),
    ("WheelUp", WHEEL_UP),
    ("WheelDown", WHEEL_DOWN),
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Esc", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("NumMultiply", 0x6A),
    ("NumAdd", 0x6B),
    ("NumSubtract", 0x6D),
    ("NumDecimal", 0x6E),
    ("NumDivide", 0x6F),
    ("LShift", 0xA0),
    ("RShift", 0xA1),
    ("LCtrl", 0xA2),
    ("RCtrl", 0xA3),
    ("LAlt", 0xA4),
    ("RAlt", 0xA5),
    ("Semicolon", 0xBA),
    ("Equals", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Grave", 0xC0),
    ("LBracket", 0xDB),
    ("Backslash", 0xDC),
    ("RBracket", 0xDD),
    ("Quote", 0xDE),
    ("Shift", 0xA0),
    ("Ctrl", 0xA2),
];

/// The virtual key a key name stands for (case does not matter).
pub fn key_code(name: &str) -> Option<u8> {
    let name = name.trim();
    if let [c] = name.as_bytes()
        && c.is_ascii_alphanumeric()
    {
        return Some(c.to_ascii_uppercase());
    }
    if let Some((_, vk)) = KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(*vk);
    }
    if name.eq_ignore_ascii_case("Alt") {
        return Some(0xA4);
    }
    let numbered = |prefix: &str, first: u8, count: u8| {
        let rest = name.get(prefix.len()..)?;
        if !name.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
            return None;
        }
        let n: u8 = rest.parse().ok()?;
        (n < count).then(|| first + n)
    };
    numbered("Num", 0x60, 10).or_else(|| {
        numbered("F", 0x6F, 25).filter(|&vk| vk > 0x6F) // F1 = 0x70
    })
}

/// A key's name, for messages.
pub fn key_name(vk: u8) -> String {
    if vk.is_ascii_uppercase() || vk.is_ascii_digit() {
        return char::from(vk).to_string();
    }
    match vk {
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => KEYS
            .iter()
            .find(|(_, k)| *k == vk)
            .map_or_else(|| format!("{vk:#04x}"), |(n, _)| (*n).to_string()),
    }
}

fn output(name: &str) -> Option<Output> {
    OUTPUTS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
        .map(|(_, o)| *o)
}

/// Keys and what each does, for one situation (on foot, in a vehicle).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout(BTreeMap<u8, Vec<Output>>);

impl Layout {
    fn outputs(&self, vk: u8) -> &[Output] {
        self.0.get(&vk).map_or(&[], Vec::as_slice)
    }

    /// Binds `key` to `outputs` (none: unbinds it).
    fn bind(&mut self, key: &str, outputs: &[&str]) -> Result<(), String> {
        let vk = key_code(key).ok_or_else(|| format!("unknown key `{key}`"))?;
        let outs = outputs
            .iter()
            .filter(|o| !o.trim().is_empty())
            .map(|o| output(o).ok_or_else(|| format!("unknown pad output `{o}` for key `{key}`")))
            .collect::<Result<Vec<_>, _>>()?;
        if outs.is_empty() {
            self.0.remove(&vk);
        } else {
            self.0.insert(vk, outs);
        }
        Ok(())
    }

    fn of(table: &[(&str, &[&str])]) -> Layout {
        let mut layout = Layout::default();
        for (key, outs) in table {
            layout.bind(key, outs).expect("default bindings use known names");
        }
        layout
    }
}

/// The keys both layouts share: the default PC layout (docs/input.md).
/// Shift sprints (A), Space is X, E and F are Y, Q and R the bumpers, Ctrl
/// clicks the left stick; Enter is A and Esc or Backspace B in menus. Alt
/// frees the mouse. The wheel is the d-pad's up and down.
const COMMON: &[(&str, &[&str])] = &[
    ("A", &["LeftLeft"]),
    ("D", &["LeftRight"]),
    ("LShift", &["A"]),
    ("RShift", &["A"]),
    ("Space", &["X"]),
    ("E", &["Y"]),
    ("F", &["Y"]),
    ("Q", &["LB"]),
    ("R", &["RB"]),
    ("LCtrl", &["LS"]),
    ("RCtrl", &["LS"]),
    ("C", &["RS"]),
    ("Tab", &["View"]),
    ("P", &["Menu"]),
    ("Z", &["Walk"]),
    ("G", &["ToggleVehicle"]),
    ("LAlt", &["ReleaseCursor"]),
    ("RAlt", &["ReleaseCursor"]),
    ("Up", &["DpadUp"]),
    ("Down", &["DpadDown"]),
    ("Left", &["DpadLeft"]),
    ("Right", &["DpadRight"]),
    ("WheelUp", &["DpadUp"]),
    ("WheelDown", &["DpadDown"]),
    ("MouseLeft", &["RT"]),
    ("MouseRight", &["LT"]),
    ("MouseMiddle", &["RS"]),
    ("Enter", &["A"]),
    ("Backspace", &["B"]),
    ("Esc", &["B"]),
];

/// On foot: W/S walk.
const ON_FOOT: &[(&str, &[&str])] = &[("W", &["LeftUp"]), ("S", &["LeftDown"])];

/// In a vehicle (G switches): W/S are throttle and brake on the triggers.
const VEHICLE: &[(&str, &[&str])] = &[("W", &["RT"]), ("S", &["LT"])];

fn default_layout(own: &[(&str, &[&str])]) -> Layout {
    let mut layout = Layout::of(COMMON);
    for (key, outs) in own {
        layout.bind(key, outs).expect("default bindings use known names");
    }
    layout
}
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Off: the old stand-in pad (README's table) and nothing else.
    pub enabled: bool,
    pub sensitivity: f32,
    /// Vertical sensitivity relative to horizontal.
    pub y_scale: f32,
    pub invert_y: bool,
    /// Capture (hide and hold) the cursor while the title window has focus.
    pub capture: bool,
    /// Counts past a full stick are turned later instead of dropped.
    pub carry: bool,
    /// The title's own stick dead zone and response exponent, undone so the
    /// smallest movement turns and the response is linear.
    pub deadzone: f32,
    pub curve: f32,
    pub walk_speed: f32,
    pub on_foot: Layout,
    pub vehicle: Layout,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: true,
            sensitivity: 1.0,
            y_scale: 1.0,
            invert_y: false,
            capture: true,
            carry: true,
            deadzone: 0.0,
            curve: 1.0,
            walk_speed: 0.5,
            on_foot: default_layout(ON_FOOT),
            vehicle: default_layout(VEHICLE),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Binding {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    enabled: Option<bool>,
    mouse_sensitivity: Option<f32>,
    mouse_y_scale: Option<f32>,
    mouse_invert_y: Option<bool>,
    mouse_capture: Option<bool>,
    mouse_carry: Option<bool>,
    stick_deadzone: Option<f32>,
    stick_curve: Option<f32>,
    walk_speed: Option<f32>,
    #[serde(default)]
    on_foot: BTreeMap<String, Binding>,
    #[serde(default)]
    vehicle: BTreeMap<String, Binding>,
}

impl Settings {
    /// The defaults with `[controls]` applied; a layout table replaces only
    /// the keys it names.
    pub fn parse(table: &toml::Table) -> Result<Settings, String> {
        let file: File = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e| format!("[controls]: {e}"))?;
        let mut s = Settings::default();
        let range = |name: &str, v: Option<f32>, lo: f32, hi: f32, into: &mut f32| match v {
            Some(v) if !(lo..=hi).contains(&v) => {
                Err(format!("[controls] {name} = {v} is outside {lo} to {hi}"))
            }
            Some(v) => {
                *into = v;
                Ok(())
            }
            None => Ok(()),
        };
        range("mouse_sensitivity", file.mouse_sensitivity, 0.01, 100.0, &mut s.sensitivity)?;
        range("mouse_y_scale", file.mouse_y_scale, 0.01, 100.0, &mut s.y_scale)?;
        range("stick_deadzone", file.stick_deadzone, 0.0, 0.9, &mut s.deadzone)?;
        range("stick_curve", file.stick_curve, 0.2, 5.0, &mut s.curve)?;
        range("walk_speed", file.walk_speed, 0.05, 1.0, &mut s.walk_speed)?;
        s.enabled = file.enabled.unwrap_or(s.enabled);
        s.invert_y = file.mouse_invert_y.unwrap_or(s.invert_y);
        s.capture = file.mouse_capture.unwrap_or(s.capture);
        s.carry = file.mouse_carry.unwrap_or(s.carry);
        for (layout, binds, name) in [
            (&mut s.on_foot, &file.on_foot, "on_foot"),
            (&mut s.vehicle, &file.vehicle, "vehicle"),
        ] {
            for (key, binding) in binds {
                let outs: Vec<&str> = match binding {
                    Binding::One(o) => vec![o.as_str()],
                    Binding::Many(os) => os.iter().map(String::as_str).collect(),
                };
                layout
                    .bind(key, &outs)
                    .map_err(|e| format!("[controls.{name}]: {e}"))?;
            }
        }
        Ok(s)
    }
}

/// What the window caption says while the mouse is free.
const CAPTION_HINT: &str = " - click to capture the mouse (Alt frees it)";

/// The window caption for `base` (the title's own, with or without our
/// hint): with the hint while the mouse is not `captured`.
pub fn caption(base: &str, captured: bool) -> String {
    let base = base.strip_suffix(CAPTION_HINT).unwrap_or(base);
    if captured {
        base.to_string()
    } else {
        format!("{base}{CAPTION_HINT}")
    }
}

fn flag(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" | "" => Some(false),
        _ => None,
    }
}

impl Settings {
    /// Rebinds from a key map: `key=button[,button...]` lines (`Space=X`,
    /// `MouseLeft=RT,A`; nothing after `=` unbinds the key). `#` starts a
    /// comment. `[on_foot]` and `[vehicle]` lines choose a layout; before
    /// either, a binding goes to both. Returns what could not be read, one
    /// message per line; the lines that can are applied.
    pub fn apply_keymap(&mut self, text: &str) -> Vec<String> {
        let mut problems = Vec::new();
        let (mut foot, mut vehicle) = (true, true);
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let n = n + 1;
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                match name.trim().to_ascii_lowercase().as_str() {
                    "on_foot" | "onfoot" | "foot" => (foot, vehicle) = (true, false),
                    "vehicle" => (foot, vehicle) = (false, true),
                    "all" | "both" => (foot, vehicle) = (true, true),
                    _ => problems.push(format!("line {n}: unknown section `{name}`")),
                }
                continue;
            }
            let Some((key, outs)) = line.split_once('=') else {
                problems.push(format!("line {n}: expected key=button"));
                continue;
            };
            let outs: Vec<&str> = outs.split(',').collect();
            let mut layouts = Vec::new();
            if foot {
                layouts.push(&mut self.on_foot);
            }
            if vehicle {
                layouts.push(&mut self.vehicle);
            }
            for layout in layouts {
                if let Err(e) = layout.bind(key, &outs) {
                    problems.push(format!("line {n}: {e}"));
                    break;
                }
            }
        }
        problems
    }

    /// The environment's say, over `[controls]`: `VICTORY_KBM=0` turns
    /// keyboard and mouse play off, `VICTORY_MOUSE_SENS=<factor>`,
    /// `VICTORY_MOUSE_INVERT_Y=1`, `VICTORY_KEYMAP=<file>`. Returns what
    /// could not be used.
    pub fn apply_env(
        &mut self,
        get: &dyn Fn(&str) -> Option<String>,
        read: &dyn Fn(&str) -> std::io::Result<String>,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(v) = get("VICTORY_KBM") {
            match flag(&v) {
                Some(on) => self.enabled = on,
                None => problems.push(format!("VICTORY_KBM={v}: expected 0 or 1")),
            }
        }
        if let Some(v) = get("VICTORY_MOUSE_SENS") {
            match v.trim().parse::<f32>() {
                Ok(s) if (0.01..=100.0).contains(&s) => self.sensitivity = s,
                _ => problems.push(format!("VICTORY_MOUSE_SENS={v}: expected 0.01 to 100")),
            }
        }
        if let Some(v) = get("VICTORY_MOUSE_INVERT_Y") {
            match flag(&v) {
                Some(on) => self.invert_y = on,
                None => problems.push(format!("VICTORY_MOUSE_INVERT_Y={v}: expected 0 or 1")),
            }
        }
        if let Some(path) = get("VICTORY_KEYMAP").filter(|p| !p.trim().is_empty()) {
            match read(path.trim()) {
                Ok(text) => problems.extend(
                    self.apply_keymap(&text)
                        .into_iter()
                        .map(|p| format!("VICTORY_KEYMAP {path}: {p}")),
                ),
                Err(e) => problems.push(format!("VICTORY_KEYMAP {path}: {e}")),
            }
        }
        problems
    }
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

/// Takes `[controls]` from the config file (none: the defaults). Call it
/// before the title starts; later calls change nothing.
pub fn configure(table: Option<&toml::Table>) -> Result<(), String> {
    let mut s = match table {
        Some(t) => Settings::parse(t)?,
        None => Settings::default(),
    };
    // A bad environment value is logged, never fatal: the game still starts.
    let env = |k: &str| std::env::var(k).ok();
    let read = |p: &str| std::fs::read_to_string(p);
    for problem in s.apply_env(&env, &read) {
        tracing::warn!("{problem}");
    }
    tracing::info!(
        enabled = s.enabled,
        sensitivity = s.sensitivity,
        invert_y = s.invert_y,
        capture = s.capture,
        deadzone = s.deadzone,
        curve = s.curve,
        "keyboard and mouse controls"
    );
    let _ = SETTINGS.set(s);
    Ok(())
}

pub fn settings() -> Settings {
    SETTINGS.get_or_init(Settings::default).clone()
}

/// Mouse motion and wheel steps since they were last drained, added from
/// the host's raw input thread (only while the cursor is captured).
#[derive(Default)]
pub struct Counts {
    x: AtomicI64,
    y: AtomicI64,
    wheel: AtomicI32,
}

impl Counts {
    pub fn add(&self, dx: i64, dy: i64) {
        self.x.fetch_add(dx, Ordering::Relaxed);
        self.y.fetch_add(dy, Ordering::Relaxed);
    }
    /// Steps away from the user are positive, as Windows reports them.
    pub fn add_wheel(&self, steps: i32) {
        self.wheel.fetch_add(steps, Ordering::Relaxed);
    }
    fn drain(&self) -> (i64, i64) {
        (
            self.x.swap(0, Ordering::Relaxed),
            self.y.swap(0, Ordering::Relaxed),
        )
    }
    fn drain_wheel(&self) -> i32 {
        self.wheel.swap(0, Ordering::Relaxed)
    }
}

/// Which device moved a stick last, or was used last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Pad,
    Kbm,
}

/// What a set of held keys does.
#[derive(Clone, Copy, Default, PartialEq)]
struct Derived {
    buttons: u32,
    left_trigger: bool,
    right_trigger: bool,
    left: (f32, f32),
    right: (f32, f32),
    left_moved: bool,
    right_moved: bool,
}

/// A wheel step is a press the title has read once, then a release it has
/// read once, so steps survive any frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pulse {
    Idle,
    Down(u8),
    Gap,
}

/// The mouse as a right stick.
#[derive(Default)]
struct Look {
    /// Counts not yet turned (y as Windows reports it: down positive).
    pending: (f64, f64),
    last_take: Option<u64>,
    value: (f32, f32),
    moved: bool,
}

impl Look {
    fn take(&mut self, now: u64, (dx, dy): (i64, i64), s: &Settings) -> (f32, f32) {
        self.pending.0 += dx as f64;
        self.pending.1 += dy as f64;
        if let Some(last) = self.last_take
            && now.saturating_sub(last) < MIN_TAKE_US
        {
            return self.value;
        }
        let span = self
            .last_take
            .map_or(FIRST_TAKE_US, |last| now.saturating_sub(last).min(MAX_TAKE_US));
        self.last_take = Some(now);
        let dt = span as f64 / 1e6;
        let full = FULL_RATE / f64::from(s.sensitivity);
        let mut ux = self.pending.0 / dt / full;
        let mut uy = -self.pending.1 * f64::from(s.y_scale) / dt / full;
        let mut m = ux.hypot(uy);
        self.moved = m > 0.0;
        if m <= 1.0 {
            self.pending = (0.0, 0.0);
        } else {
            // Full stick in the direction moved; the rest waits (capped).
            let keep = if s.carry { 1.0 - 1.0 / m } else { 0.0 };
            self.pending.0 *= keep;
            self.pending.1 *= keep;
            let cap = CARRY_S * full;
            let left = self.pending.0.hypot(self.pending.1);
            if left > cap {
                self.pending.0 *= cap / left;
                self.pending.1 *= cap / left;
            }
            ux /= m;
            uy /= m;
            m = 1.0;
        }
        if m > 0.0 {
            let d = f64::from(s.deadzone);
            let shaped = d + (1.0 - d) * m.powf(1.0 / f64::from(s.curve));
            ux *= shaped / m;
            uy *= shaped / m;
        }
        if s.invert_y {
            uy = -uy;
        }
        self.value = (ux as f32, uy as f32);
        if dx != 0 || dy != 0 {
            tracing::debug!(
                span_ms = span / 1000,
                dx,
                dy,
                x = self.value.0,
                y = self.value.1,
                waiting = self.pending.0.hypot(self.pending.1).round(),
                "mouse look at a title read"
            );
        }
        self.value
    }
}

/// Keyboard and mouse state between host polls and title reads.
pub struct Controls {
    pub s: Settings,
    vehicle: bool,
    held: Vec<u8>,
    derived: Derived,
    /// Buttons and triggers pressed since the title's last read: they stay
    /// down until it has read them, so a tap shorter than a frame counts.
    latched: u32,
    latched_triggers: (bool, bool),
    wheel: VecDeque<u8>,
    pulse: Pulse,
    look: Look,
    left_owner: Owner,
    right_owner: Owner,
    last_used: Option<Owner>,
    /// A release key (Alt) was pressed: the host frees the cursor.
    pub release_cursor: bool,
    /// Scripted input (auto input, press file, fast boot) as a pad, its
    /// presses latched like keys.
    script: PadState,
}

impl Controls {
    pub fn new(s: Settings) -> Controls {
        Controls {
            s,
            vehicle: false,
            held: Vec::new(),
            derived: Derived::default(),
            latched: 0,
            latched_triggers: (false, false),
            wheel: VecDeque::new(),
            pulse: Pulse::Idle,
            look: Look::default(),
            left_owner: Owner::Kbm,
            right_owner: Owner::Kbm,
            last_used: None,
            release_cursor: false,
            script: PadState::default(),
        }
    }

    fn layout(&self) -> &Layout {
        if self.vehicle {
            &self.s.vehicle
        } else {
            &self.s.on_foot
        }
    }

    #[cfg(test)]
    fn in_vehicle(&self) -> bool {
        self.vehicle
    }

    fn derive(&self, keys: impl Iterator<Item = u8>) -> Derived {
        let mut d = Derived::default();
        let (mut l, mut r) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
        let mut walk = false;
        for vk in keys {
            for out in self.layout().outputs(vk) {
                match *out {
                    Output::Button(b) => d.buttons |= b,
                    Output::LeftTrigger => d.left_trigger = true,
                    Output::RightTrigger => d.right_trigger = true,
                    Output::Stick { right, x, positive } => {
                        let stick = if right { &mut r } else { &mut l };
                        let axis = if x { &mut stick.0 } else { &mut stick.1 };
                        *axis += if positive { 1.0 } else { -1.0 };
                        if right {
                            d.right_moved = true;
                        } else {
                            d.left_moved = true;
                        }
                    }
                    Output::Walk => walk = true,
                    Output::ToggleVehicle | Output::ReleaseCursor => {}
                }
            }
        }
        let round = |(x, y): (f32, f32)| {
            let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
            let m = x.hypot(y);
            if m > 1.0 { (x / m, y / m) } else { (x, y) }
        };
        let speed = if walk { self.s.walk_speed } else { 1.0 };
        let (lx, ly) = round(l);
        Derived {
            left: (lx * speed, ly * speed),
            right: round(r),
            ..d
        }
    }

    /// A host poll: the keys and mouse buttons held now, wheel steps since
    /// the last poll.
    pub fn feed(&mut self, held: &[u8], counts: &Counts) {
        let steps = counts.drain_wheel();
        let key = if steps > 0 { WHEEL_UP } else { WHEEL_DOWN };
        for _ in 0..steps.unsigned_abs() {
            if self.wheel.len() < WHEEL_QUEUE {
                self.wheel.push_back(key);
            }
        }
        let pressed: Vec<u8> = held
            .iter()
            .copied()
            .filter(|vk| !self.held.contains(vk))
            .collect();
        for &vk in &pressed {
            let outs = self.layout().outputs(vk).to_vec();
            if outs.contains(&Output::ToggleVehicle) {
                self.vehicle = !self.vehicle;
                tracing::info!(vehicle = self.vehicle, key = %key_name(vk), "controls layout");
            }
            if outs.contains(&Output::ReleaseCursor) {
                self.release_cursor = true;
            }
        }
        let before = self.derived;
        self.held = held.to_vec();
        self.derived = self.derive(self.held.iter().copied());
        self.latched |= self.derived.buttons & !before.buttons;
        self.latched_triggers.0 |= self.derived.left_trigger && !before.left_trigger;
        self.latched_triggers.1 |= self.derived.right_trigger && !before.right_trigger;
        if self.pulse == Pulse::Idle
            && let Some(key) = self.wheel.pop_front()
        {
            self.pulse = Pulse::Down(key);
        }
    }

    /// The keyboard and mouse pad as it is now (a host poll's reading).
    fn kbm(&self) -> PadState {
        let mut d = self.derived;
        if let Pulse::Down(key) = self.pulse {
            let p = self.derive(std::iter::once(key));
            d.buttons |= p.buttons;
            d.left_trigger |= p.left_trigger;
            d.right_trigger |= p.right_trigger;
        }
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        let (rx, ry) = (
            d.right.0 + self.look.value.0,
            d.right.1 + self.look.value.1,
        );
        let m = rx.hypot(ry);
        let (rx, ry) = if m > 1.0 { (rx / m, ry / m) } else { (rx, ry) };
        PadState {
            buttons: d.buttons | self.latched,
            left_trigger: on(d.left_trigger || self.latched_triggers.0),
            right_trigger: on(d.right_trigger || self.latched_triggers.1),
            left_x: d.left.0,
            left_y: d.left.1,
            right_x: rx,
            right_y: ry,
        }
        .merged(self.script)
    }

    /// Scripted input as it is now (every host poll): its presses are
    /// latched like key presses, so a short one survives a long frame.
    pub fn set_script(&mut self, pad: PadState) {
        self.latched |= pad.buttons & !self.script.buttons;
        self.latched_triggers.0 |= pad.left_trigger > 0.5 && self.script.left_trigger <= 0.5;
        self.latched_triggers.1 |= pad.right_trigger > 0.5 && self.script.right_trigger <= 0.5;
        self.script = pad;
    }

    /// The pad a host poll publishes: `pad` (a host pad, if any) with the
    /// keyboard and mouse on it.
    pub fn peek(&mut self, pad: Option<PadState>) -> PadState {
        self.merge(pad)
    }

    /// The pad the title reads now: the mouse counts since its last read
    /// become the right stick, and what it has now seen is let go.
    pub fn take(&mut self, now: u64, pad: Option<PadState>, counts: &Counts) -> PadState {
        let before = self.look.last_take;
        self.look.take(now, counts.drain(), &self.s);
        let state = self.merge(pad);
        if self.look.last_take != before {
            self.latched = 0;
            self.latched_triggers = (false, false);
            self.pulse = match self.pulse {
                Pulse::Down(_) => Pulse::Gap,
                Pulse::Gap | Pulse::Idle => Pulse::Idle,
            };
        }
        state
    }

    fn merge(&mut self, pad: Option<PadState>) -> PadState {
        let kbm = self.kbm();
        let pushed = |x: f32, y: f32| x.hypot(y) > PAD_TAKES_OVER;
        let script_left = pushed(self.script.left_x, self.script.left_y);
        let script_right = pushed(self.script.right_x, self.script.right_y);
        let kbm_used = !self.held.is_empty()
            || self.look.moved
            || self.pulse != Pulse::Idle
            || !self.script.is_idle();
        let Some(pad) = pad else {
            self.used(kbm_used.then_some(Owner::Kbm));
            return kbm;
        };
        let pad_left = pushed(pad.left_x, pad.left_y);
        let pad_right = pushed(pad.right_x, pad.right_y);
        // A pad that deflects a stick has it; else whatever moves it
        // (keys, mouse, scripted input); else the last owner keeps it.
        if pad_left {
            self.left_owner = Owner::Pad;
        } else if self.derived.left_moved || script_left {
            self.left_owner = Owner::Kbm;
        }
        if pad_right {
            self.right_owner = Owner::Pad;
        } else if self.derived.right_moved || self.look.moved || script_right {
            self.right_owner = Owner::Kbm;
        }
        let pad_used = pad.buttons != 0
            || pad_left
            || pad_right
            || pad.left_trigger > 0.1
            || pad.right_trigger > 0.1;
        self.used(if kbm_used {
            Some(Owner::Kbm)
        } else {
            pad_used.then_some(Owner::Pad)
        });
        let (lx, ly) = match self.left_owner {
            Owner::Kbm => (kbm.left_x, kbm.left_y),
            Owner::Pad => (pad.left_x, pad.left_y),
        };
        let (rx, ry) = match self.right_owner {
            Owner::Kbm => (kbm.right_x, kbm.right_y),
            Owner::Pad => (pad.right_x, pad.right_y),
        };
        PadState {
            buttons: pad.buttons | kbm.buttons,
            left_trigger: pad.left_trigger.max(kbm.left_trigger),
            right_trigger: pad.right_trigger.max(kbm.right_trigger),
            left_x: lx,
            left_y: ly,
            right_x: rx,
            right_y: ry,
        }
    }

    fn used(&mut self, now: Option<Owner>) {
        if let Some(owner) = now
            && self.last_used != Some(owner)
        {
            self.last_used = Some(owner);
            tracing::info!(
                device = match owner {
                    Owner::Pad => "gamepad",
                    Owner::Kbm => "keyboard and mouse",
                },
                "input: now using"
            );
        }
    }

    /// The device used last (for prompts, once they can change).
    #[allow(dead_code)]
    pub fn last_used(&self) -> Option<Owner> {
        self.last_used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk(name: &str) -> u8 {
        key_code(name).unwrap()
    }

    fn controls() -> Controls {
        Controls::new(Settings::default())
    }

    fn held(c: &mut Controls, keys: &[&str]) -> PadState {
        let keys: Vec<u8> = keys.iter().map(|k| vk(k)).collect();
        c.feed(&keys, &Counts::default());
        c.peek(None)
    }

    #[test]
    fn key_names_round_trip() {
        for (name, code) in [
            ("w", b'W'),
            ("7", b'7'),
            ("Space", 0x20),
            ("lshift", 0xA0),
            ("F1", 0x70),
            ("F24", 0x87),
            ("Num5", 0x65),
            ("MouseLeft", 0x01),
            ("WheelDown", WHEEL_DOWN),
            ("Alt", 0xA4),
        ] {
            assert_eq!(key_code(name), Some(code), "{name}");
        }
        assert_eq!(key_code("F0"), None);
        assert_eq!(key_code("F25"), None);
        assert_eq!(key_code("Num10"), None);
        assert_eq!(key_code("Banana"), None);
        assert_eq!(key_name(0x70), "F1");
        assert_eq!(key_name(0xA0), "LShift");
        assert_eq!(key_name(b'Q'), "Q");
    }

    #[test]
    fn wasd_moves_the_left_stick_round_and_walk_slows_it() {
        let mut c = controls();
        let p = held(&mut c, &["W"]);
        assert_eq!((p.left_x, p.left_y), (0.0, 1.0));
        let p = held(&mut c, &["W", "D"]);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!((p.left_x - r).abs() < 1e-6 && (p.left_y - r).abs() < 1e-6);
        let p = held(&mut c, &["A", "D"]);
        assert_eq!((p.left_x, p.left_y), (0.0, 0.0));
        let p = held(&mut c, &["S", "Z"]);
        assert_eq!((p.left_x, p.left_y), (0.0, -0.5));
    }

    #[test]
    fn keys_and_mouse_buttons_press_pad_buttons_and_triggers() {
        let mut c = controls();
        let p = held(&mut c, &["Space", "LShift", "MouseLeft", "MouseRight", "Tab"]);
        assert_eq!(p.buttons, X | A | VIEW);
        assert_eq!((p.left_trigger, p.right_trigger), (1.0, 1.0));
        let mut c = controls();
        let p = held(&mut c, &["Esc"]);
        assert_eq!(p.buttons, B, "Esc is B, it does not free the mouse");
        assert!(!c.release_cursor);
        let p = held(&mut controls(), &["Backspace", "P"]);
        assert_eq!(p.buttons, B | MENU);
        held(&mut c, &["LAlt"]);
        assert!(c.release_cursor, "Alt frees the cursor");
    }

    #[test]
    fn g_switches_to_the_vehicle_layout_and_back() {
        let mut c = controls();
        held(&mut c, &["G"]);
        assert!(c.in_vehicle());
        held(&mut c, &[]);
        let p = held(&mut c, &["W", "A"]);
        assert_eq!(p.right_trigger, 1.0, "W is throttle");
        assert_eq!((p.left_x, p.left_y), (-1.0, 0.0), "A steers, W does not move the stick");
        // Holding G does not switch again.
        held(&mut c, &["G"]);
        held(&mut c, &["G"]);
        assert!(!c.in_vehicle());
    }

    #[test]
    fn a_tap_between_two_reads_is_read_once() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        c.feed(&[vk("Space")], &counts);
        c.feed(&[], &counts);
        assert_eq!(c.peek(None).buttons, X, "held for the title until it reads");
        assert_eq!(c.take(10_000, None, &counts).buttons, X);
        assert_eq!(c.take(20_000, None, &counts).buttons, 0);
        // A quick click shoots once.
        c.feed(&[vk("MouseLeft")], &counts);
        c.feed(&[], &counts);
        assert_eq!(c.take(30_000, None, &counts).right_trigger, 1.0);
        assert_eq!(c.take(40_000, None, &counts).right_trigger, 0.0);
    }

    #[test]
    fn wheel_steps_are_each_a_press_the_title_reads() {
        let mut c = controls();
        let counts = Counts::default();
        counts.add_wheel(2);
        let mut seen = Vec::new();
        for i in 0..6 {
            c.feed(&[], &counts);
            seen.push(c.take(i * 10_000, None, &counts).buttons);
        }
        assert_eq!(seen, [UP, 0, UP, 0, 0, 0]);
        counts.add_wheel(-1);
        c.feed(&[], &counts);
        assert_eq!(c.take(100_000, None, &counts).buttons, DOWN);
    }

    #[test]
    fn mouse_counts_become_a_rate_at_each_read() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        // 2,000 counts/s = full stick at sensitivity 1: 10 counts in 10 ms is half.
        counts.add(10, 0);
        let p = c.take(10_000, None, &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6, "{}", p.right_x);
        // The same distance over a 4x longer frame is a 4x smaller rate: the
        // title turns rate x frame time, so the same angle.
        counts.add(10, 0);
        let p = c.take(50_000, None, &counts);
        assert!((p.right_x - 0.125).abs() < 1e-6, "{}", p.right_x);
        // Mouse up looks up; no motion lets go.
        counts.add(0, -5);
        let p = c.take(60_000, None, &counts);
        assert!((p.right_y - 0.25).abs() < 1e-6 && p.right_x == 0.0);
        assert_eq!(c.take(70_000, None, &counts).right_y, 0.0);
    }

    #[test]
    fn slow_frames_get_the_rate_over_their_whole_span() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        // 1,000 counts over an 800 ms frame: half a stick, not saturated.
        counts.add(1000, 0);
        let p = c.take(800_000, None, &counts);
        assert!((p.right_x - 0.625).abs() < 1e-6, "{}", p.right_x);
        // Past the longest span the title counts as paused.
        counts.add(1000, 0);
        let p = c.take(3_800_000, None, &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6, "{}", p.right_x);
    }

    #[test]
    fn reads_close_together_see_the_same_mouse_stick() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(10, 0);
        let a = c.take(10_000, None, &counts);
        let b = c.take(10_500, None, &counts);
        assert_eq!(a, b);
        counts.add(4, 0);
        let p = c.take(11_000, None, &counts);
        assert_eq!(p, a, "counts wait for the next read");
        let p = c.take(20_000, None, &counts);
        assert!((p.right_x - 0.2).abs() < 1e-6, "{}", p.right_x);
    }

    #[test]
    fn dead_zone_curve_and_inversion_shape_the_mouse_stick() {
        let s = Settings {
            deadzone: 0.2,
            curve: 2.0,
            invert_y: true,
            ..Settings::default()
        };
        let mut c = Controls::new(s);
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(0, -5); // a quarter stick up
        let p = c.take(10_000, None, &counts);
        // 0.2 + 0.8 * sqrt(0.25) = 0.6, then inverted.
        assert!((p.right_y + 0.6).abs() < 1e-6, "{}", p.right_y);
        // One count still clears the dead zone.
        counts.add(1, 0);
        let p = c.take(20_000, None, &counts);
        assert!(p.right_x > 0.2);
    }

    #[test]
    fn a_fast_flick_is_turned_in_full_later() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(50, 0); // 2.5 full sticks in 10 ms
        let xs: Vec<f32> = (1..=4)
            .map(|i| c.take(i * 10_000, None, &counts).right_x)
            .collect();
        assert_eq!(&xs[..2], &[1.0, 1.0]);
        assert!((xs[2] - 0.5).abs() < 1e-6, "{xs:?}");
        assert_eq!(xs[3], 0.0);
        let mut c = Controls::new(Settings {
            carry: false,
            ..Settings::default()
        });
        c.take(0, None, &counts);
        counts.add(50, 0);
        assert_eq!(c.take(10_000, None, &counts).right_x, 1.0);
        assert_eq!(c.take(20_000, None, &counts).right_x, 0.0);
    }

    #[test]
    fn a_resting_pad_never_overrides_the_mouse_and_a_pushed_one_does() {
        let mut c = controls();
        let counts = Counts::default();
        let drift = PadState {
            right_x: 0.1,
            left_y: -0.05,
            buttons: B,
            right_trigger: 0.4,
            ..Default::default()
        };
        c.take(0, Some(drift), &counts);
        counts.add(10, 0);
        c.feed(&[vk("W"), vk("MouseLeft")], &counts);
        let p = c.take(10_000, Some(drift), &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6);
        assert_eq!((p.left_y, p.buttons, p.right_trigger), (1.0, B, 1.0));
        c.feed(&[], &counts);
        assert_eq!(c.take(20_000, Some(drift), &counts).right_x, 0.0, "drift stays out");
        let pushed = PadState {
            right_x: 0.7,
            left_x: -0.9,
            ..Default::default()
        };
        let p = c.take(30_000, Some(pushed), &counts);
        assert_eq!((p.right_x, p.left_x), (0.7, -0.9));
        assert_eq!(c.last_used(), Some(Owner::Pad));
    }

    #[test]
    fn config_rebinds_unbinds_and_refuses_typos() {
        let table: toml::Table = toml::from_str(
            r#"
            mouse_sensitivity = 2.5
            mouse_invert_y = true
            stick_deadzone = 0.24
            [on_foot]
            Space = "A"
            LShift = ""
            Mouse4 = ["LB", "RB"]
            [vehicle]
            W = "RightTrigger_typo"
            "#,
        )
        .unwrap();
        let e = Settings::parse(&table).unwrap_err();
        assert!(e.contains("RightTrigger_typo"), "{e}");
        let mut table = table;
        table.remove("vehicle");
        let s = Settings::parse(&table).unwrap();
        assert_eq!((s.sensitivity, s.invert_y, s.deadzone), (2.5, true, 0.24));
        assert_eq!(s.on_foot.outputs(0x20), &[Output::Button(A)]);
        assert!(s.on_foot.outputs(0xA0).is_empty());
        assert_eq!(
            s.on_foot.outputs(0x05),
            &[Output::Button(LEFT_SHOULDER), Output::Button(RIGHT_SHOULDER)]
        );
        assert_eq!(s.on_foot.outputs(b'W'), Settings::default().on_foot.outputs(b'W'));
        for bad in ["mouse_sensitivty = 1.0", "stick_deadzone = 1.5", "[on_foot]\nBanana = \"A\""] {
            let t: toml::Table = toml::from_str(bad).unwrap();
            assert!(Settings::parse(&t).is_err(), "{bad}");
        }
        let off: toml::Table = toml::from_str("enabled = false").unwrap();
        assert!(!Settings::parse(&off).unwrap().enabled);
    }

    #[test]
    fn keymap_lines_rebind_unbind_and_report_what_they_cannot_read() {
        let mut s = Settings::default();
        let problems = s.apply_keymap(
            "# my keys\n\
             Space = A\n\
             MouseLeft = RT, A   # fire and confirm\n\
             LShift =\n\
             Banana = X\n\
             Tab = Nonsense\n\
             no equals here\n\
             [vehicle]\n\
             W = RT, LB\n\
             [oops]\n",
        );
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems[0].starts_with("line 5") && problems[0].contains("Banana"));
        assert!(problems[1].starts_with("line 6") && problems[1].contains("Nonsense"));
        assert!(problems[2].starts_with("line 7"));
        assert!(problems[3].starts_with("line 10") && problems[3].contains("oops"));
        // Before a section header a binding goes to both layouts.
        for layout in [&s.on_foot, &s.vehicle] {
            assert_eq!(layout.outputs(0x20), &[Output::Button(A)]);
            assert_eq!(layout.outputs(0x01), &[Output::RightTrigger, Output::Button(A)]);
            assert!(layout.outputs(0xA0).is_empty());
        }
        // A section only changes its own layout; the rest keeps its default.
        assert_eq!(s.vehicle.outputs(b'W'), &[Output::RightTrigger, Output::Button(LEFT_SHOULDER)]);
        assert_eq!(s.on_foot.outputs(b'W'), &[stick(false, false, true)]);
        assert_eq!(s.on_foot.outputs(0x09), Settings::default().on_foot.outputs(0x09));
    }

    #[test]
    fn keymap_sections_name_one_layout() {
        let mut s = Settings::default();
        let p = s.apply_keymap("[on_foot]\nH = Y\n[vehicle]\nJ = B\n[all]\nK = X\n");
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(s.on_foot.outputs(b'H'), &[Output::Button(Y)]);
        assert!(s.vehicle.outputs(b'H').is_empty());
        assert_eq!(s.vehicle.outputs(b'J'), &[Output::Button(B)]);
        assert!(s.on_foot.outputs(b'J').is_empty());
        assert_eq!(s.on_foot.outputs(b'K'), s.vehicle.outputs(b'K'));
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_string())
    }

    fn no_files(_: &str) -> std::io::Result<String> {
        Err(std::io::ErrorKind::NotFound.into())
    }

    #[test]
    fn environment_switches_kbm_off_and_sets_the_mouse() {
        let mut s = Settings::default();
        assert!(s.enabled, "on by default");
        let p = s.apply_env(&env(&[]), &no_files);
        assert!(p.is_empty() && s.enabled);
        let p = s.apply_env(
            &env(&[("VICTORY_MOUSE_SENS", "2.5"), ("VICTORY_MOUSE_INVERT_Y", "1")]),
            &no_files,
        );
        assert!(p.is_empty());
        assert_eq!((s.sensitivity, s.invert_y, s.enabled), (2.5, true, true));
        let p = s.apply_env(&env(&[("VICTORY_KBM", "0")]), &no_files);
        assert!(p.is_empty() && !s.enabled);
        s.apply_env(&env(&[("VICTORY_KBM", "1")]), &no_files);
        assert!(s.enabled);
        // The environment wins over the config file.
        let table: toml::Table = toml::from_str("mouse_sensitivity = 3.0").unwrap();
        let mut s = Settings::parse(&table).unwrap();
        s.apply_env(&env(&[("VICTORY_MOUSE_SENS", "0.5")]), &no_files);
        assert_eq!(s.sensitivity, 0.5);
    }

    #[test]
    fn bad_environment_values_are_reported_and_ignored() {
        let mut s = Settings::default();
        let p = s.apply_env(
            &env(&[
                ("VICTORY_KBM", "maybe"),
                ("VICTORY_MOUSE_SENS", "fast"),
                ("VICTORY_MOUSE_INVERT_Y", "2"),
                ("VICTORY_KEYMAP", "missing.txt"),
            ]),
            &no_files,
        );
        assert_eq!(p.len(), 4, "{p:?}");
        assert_eq!(s, Settings::default());
        let p = s.apply_env(&env(&[("VICTORY_MOUSE_SENS", "0")]), &no_files);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn the_keymap_file_is_read_through_the_environment() {
        let mut s = Settings::default();
        let read = |p: &str| -> std::io::Result<String> {
            assert_eq!(p, "keys.txt");
            Ok("Space=B\nBogus=A\n".to_string())
        };
        let p = s.apply_env(&env(&[("VICTORY_KEYMAP", "keys.txt")]), &read);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("keys.txt") && p[0].contains("Bogus"));
        assert_eq!(s.on_foot.outputs(0x20), &[Output::Button(B)]);
    }

    /// The documented default layout (docs/input.md).
    #[test]
    fn the_default_layout_is_the_documented_one() {
        for (keys, buttons) in [
            (&["LShift"][..], A),
            (&["Space"], X),
            (&["E"], Y),
            (&["F"], Y),
            (&["Q"], LEFT_SHOULDER),
            (&["R"], RIGHT_SHOULDER),
            (&["LCtrl"], LEFT_STICK),
            (&["MouseMiddle"], RIGHT_STICK),
            (&["Enter"], A),
            (&["Esc"], B),
            (&["Backspace"], B),
            (&["Up"], UP),
            (&["Down"], DOWN),
            (&["Left"], LEFT),
            (&["Right"], RIGHT),
            (&["Tab"], VIEW),
            (&["P"], MENU),
        ] {
            assert_eq!(held(&mut controls(), keys).buttons, buttons, "{keys:?}");
        }
        let p = held(&mut controls(), &["MouseLeft"]);
        assert_eq!((p.left_trigger, p.right_trigger), (0.0, 1.0));
        let p = held(&mut controls(), &["MouseRight"]);
        assert_eq!((p.left_trigger, p.right_trigger), (1.0, 0.0));
        let p = held(&mut controls(), &["S", "A"]);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!((p.left_x + r).abs() < 1e-6 && (p.left_y + r).abs() < 1e-6);
    }

    #[test]
    fn alt_frees_the_mouse_and_does_not_press_anything() {
        for alt in ["LAlt", "RAlt"] {
            let mut c = controls();
            let p = held(&mut c, &[alt]);
            assert!(c.release_cursor);
            assert_eq!(p.buttons, 0);
        }
    }

    #[test]
    fn mouse_counts_over_a_slow_frame_turn_the_same_angle() {
        // The title reads the pad every 300 ms; the raw input thread adds
        // counts between reads. 600 counts in 300 ms is 2,000/s: full stick
        // for the whole frame, and the angle is stick x frame time.
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        for _ in 0..30 {
            counts.add(20, 0); // 20 counts every 10 ms, none lost between reads
        }
        let p = c.take(300_000, None, &counts);
        assert!((p.right_x - 1.0).abs() < 1e-6, "{}", p.right_x);
        // Half that distance over the same frame is half the rate.
        for _ in 0..30 {
            counts.add(10, 0);
        }
        let p = c.take(600_000, None, &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6, "{}", p.right_x);
        // And no motion is a stick at rest.
        let p = c.take(900_000, None, &counts);
        assert_eq!((p.right_x, p.right_y), (0.0, 0.0));
    }

    #[test]
    fn a_key_tap_and_a_click_inside_one_slow_frame_are_not_lost() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        // Pressed and released between two 300 ms title reads.
        c.feed(&[vk("Enter"), vk("MouseRight")], &counts);
        c.feed(&[vk("Enter")], &counts);
        c.feed(&[], &counts);
        let p = c.take(300_000, None, &counts);
        assert_eq!(p.buttons, A);
        assert_eq!(p.left_trigger, 1.0);
        // Seen once: the next read has let go.
        let p = c.take(600_000, None, &counts);
        assert_eq!((p.buttons, p.left_trigger), (0, 0.0));
        // A key still held across reads stays down.
        c.feed(&[vk("Space")], &counts);
        assert_eq!(c.take(900_000, None, &counts).buttons, X);
        assert_eq!(c.take(1_200_000, None, &counts).buttons, X);
    }

    fn script(buttons: u32) -> PadState {
        PadState {
            buttons,
            ..Default::default()
        }
    }

    #[test]
    fn scripted_input_joins_keys_and_pad_and_none_of_them_is_lost() {
        let mut c = controls();
        let counts = Counts::default();
        let pad = PadState {
            buttons: B,
            right_trigger: 0.5,
            ..Default::default()
        };
        c.take(0, Some(pad), &counts);
        c.feed(&[vk("Space")], &counts);
        c.set_script(script(A));
        let p = c.take(10_000, Some(pad), &counts);
        assert_eq!(p.buttons, B | X | A);
        assert_eq!(p.right_trigger, 0.5);
        // Scripted input with no pad and no keys is the whole reading.
        let mut c = controls();
        c.set_script(script(MENU));
        assert_eq!(c.peek(None).buttons, MENU);
    }

    #[test]
    fn a_scripted_press_shorter_than_a_frame_is_latched_until_read() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        c.set_script(script(A));
        c.set_script(PadState::default()); // 150 ms press, gone before the read
        assert_eq!(c.take(300_000, None, &counts).buttons, A);
        assert_eq!(c.take(600_000, None, &counts).buttons, 0);
        // A scripted stick (a press-file token) takes the stick while held.
        c.set_script(PadState {
            left_y: 1.0,
            ..Default::default()
        });
        let keys = PadState {
            left_y: -0.7,
            ..Default::default()
        };
        let p = c.take(610_000, Some(keys), &counts);
        assert_eq!(p.left_y, -0.7, "a deflected pad has priority over the script");
        let p = c.take(620_000, Some(PadState::default()), &counts);
        assert_eq!(p.left_y, 1.0, "a resting one does not");
    }

    #[test]
    fn a_physical_pad_has_priority_for_a_stick_it_pushes() {
        let mut c = controls();
        let counts = Counts::default();
        c.feed(&[vk("W")], &counts);
        c.take(0, None, &counts);
        let pad = PadState {
            left_y: -0.8,
            ..Default::default()
        };
        // The pad pushed its stick: it wins over W.
        assert_eq!(c.take(10_000, Some(pad), &counts).left_y, -0.8);
        // Once it rests, the key still held has the stick.
        let p = c.take(20_000, Some(PadState::default()), &counts);
        assert_eq!(p.left_y, 1.0);
        // With nothing held, a resting pad's stick is the stick.
        c.feed(&[], &counts);
        let p = c.take(30_000, Some(PadState::default()), &counts);
        assert_eq!(p.left_y, 0.0);
    }

    #[test]
    fn merged_pads_or_buttons_max_triggers_and_prefer_the_pushed_stick() {
        let a = PadState {
            buttons: A,
            left_trigger: 0.3,
            left_x: 0.1,
            right_y: 0.9,
            ..Default::default()
        };
        let b = PadState {
            buttons: B,
            left_trigger: 0.8,
            left_x: -1.0,
            right_y: 0.05,
            ..Default::default()
        };
        let m = a.merged(b);
        assert_eq!(m.buttons, A | B);
        assert_eq!(m.left_trigger, 0.8);
        assert_eq!(m.left_x, -1.0, "b pushes its left stick");
        assert_eq!(m.right_y, 0.9, "b only rests its right stick");
        assert!(PadState::default().is_idle() && !a.is_idle());
    }

    #[test]
    fn the_caption_hints_while_free_and_is_the_title_s_own_when_captured() {
        let free = caption("Game", false);
        assert!(free.starts_with("Game") && free.contains("click"));
        assert_eq!(caption(&free, true), "Game");
        assert_eq!(caption(&free, false), free, "no hint on a hint");
        assert_eq!(caption("Game", true), "Game");
    }

    #[test]
    fn mouse_sensitivity_and_inversion_scale_the_stick() {
        let counts = Counts::default();
        let run = |s: Settings| {
            let mut c = Controls::new(s);
            c.take(0, None, &counts);
            counts.add(5, 5);
            c.take(10_000, None, &counts)
        };
        let base = run(Settings::default());
        let fast = run(Settings {
            sensitivity: 2.0,
            ..Settings::default()
        });
        assert!((fast.right_x - 2.0 * base.right_x).abs() < 1e-6);
        let inverted = run(Settings {
            invert_y: true,
            ..Settings::default()
        });
        assert_eq!(inverted.right_y, -base.right_y);
        assert_eq!(inverted.right_x, base.right_x);
        assert!(base.right_y < 0.0, "mouse down looks down");
    }}
