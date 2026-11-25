//! Keyboard and mouse as the gamepad (`keyboard_mouse` in `xcl.toml`, on by default).
//!
//! Console titles read only gamepads. While the XCL window has the focus the keyboard and
//! the mouse play as the one gamepad XCL reports (on top of a real controller; a controller
//! stick that is deflected wins over WASD / the mouse):
//!
//! Click into the window to capture the mouse (cursor hidden and clipped to the window, raw
//! deltas drive the camera). Alt, Alt+Tab or losing the focus release it; a click captures
//! again. While it is free the window title says so. Escape stays B for the game.
//!
//! | PC | gamepad |
//! |---|---|
//! | W A S D (physical keys) | left stick |
//! | mouse movement (captured) | right stick (camera) |
//! | left / right mouse button | RT (shoot) / LT (aim) |
//! | wheel up / down | D-pad up / down (one press a notch) |
//! | middle mouse button | right stick click (RS) |
//! | Left Shift | A (sprint) |
//! | Space | X (jump) |
//! | E, F | Y (enter / leave a vehicle) |
//! | Q / R | LB / RB |
//! | Left Ctrl | left stick click (LS, crouch) |
//! | Enter | A (menus: confirm) |
//! | Escape, Backspace | B |
//! | arrows / Tab / P | D-pad / View / Menu |
//!
//! Every key but WASD can be rebound with a text file: `XCL_KEYMAP=<path>`, one `KEY=BUTTON`
//! per line (`#` comments). Keys: A-Z (physical position), 0-9, F1-F12, SPACE ENTER ESC
//! BACKSPACE TAB CAPSLOCK SHIFT RSHIFT CTRL RCTRL UP DOWN LEFT RIGHT. Buttons: A B X Y LB RB
//! LT RT LS RS VIEW MENU UP DOWN LEFT RIGHT, or NONE to unbind the key. A key listed in the
//! file replaces its default binding. `XCL_MOUSE_SENS=<f>` (default: `mouse_sensitivity` of
//! xcl.toml, 1.0) scales the camera speed; `XCL_MOUSE_INVERT_Y=1` inverts it.
//!
//! The mouse is read with Raw Input on the XCL window (movement in mouse counts, not the
//! cursor); its speed becomes a stick deflection – the stick turns the camera at a rate,
//! so a mouse moving at a given speed turns it at a matching one (`mouse_sensitivity`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use super::gameinput::{
    GamepadState, A, B, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT, DPAD_UP, LEFT_SHOULDER, LEFT_STICK, MENU, RIGHT_SHOULDER, RIGHT_STICK, VIEW, X, Y,
};

/// a controller stick deflected past this beats the keyboard / mouse on that stick
const PAD_PRIORITY: f32 = 0.1;

/// The stick `pad` reads (a real controller) or, when it rests, `other` (keys / mouse).
pub fn pick_stick(pad: (f32, f32), other: (f32, f32)) -> (f32, f32) {
    if pad.0 * pad.0 + pad.1 * pad.1 > PAD_PRIORITY * PAD_PRIORITY {
        pad
    } else if other != (0.0, 0.0) {
        other
    } else {
        pad
    }
}

// ---------------------------------------------------------------------------------------
// the mapping (pure, tested)
// ---------------------------------------------------------------------------------------

/// The left stick from the movement keys: full deflection, diagonals on the unit circle.
pub fn stick_from_keys(up: bool, left: bool, down: bool, right: bool) -> (f32, f32) {
    let x = (right as i32 - left as i32) as f32;
    let y = (up as i32 - down as i32) as f32;
    let len = (x * x + y * y).sqrt();
    if len == 0.0 {
        (0.0, 0.0)
    } else {
        (x / len, y / len)
    }
}

/// mouse counts a second that give full deflection at sensitivity 1
const FULL_SPEED: f32 = 4000.0;
/// slower than this (counts a second) the stick rests (sensor noise, a hand on the mouse)
const REST_SPEED: f32 = 20.0;
/// deflection the slowest movement starts at: past the inner dead zone games give sticks
const START: f32 = 0.15;
/// time constant of the speed's smoothing (s): the mouse reports in bursts, a stick is read
/// once a frame
const SMOOTHING: f32 = 0.03;

/// The right stick for a mouse velocity (counts a second, screen axes: y grows downwards).
/// Linear in speed (no acceleration), clamped to 1; `invert_y` flips the vertical axis.
pub fn stick_from_mouse(vx: f32, vy: f32, sensitivity: f32, invert_y: bool) -> (f32, f32) {
    let speed = (vx * vx + vy * vy).sqrt();
    if speed < REST_SPEED || sensitivity <= 0.0 {
        return (0.0, 0.0);
    }
    let m = (speed * sensitivity / FULL_SPEED).min(1.0);
    let out = START + (1.0 - START) * m;
    let sy = if invert_y { 1.0 } else { -1.0 };
    (vx / speed * out, sy * vy / speed * out)
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// `XCL_MOUSE_SENS` (a positive number) or `mouse_sensitivity` of the config.
fn mouse_sens() -> f32 {
    static ENV: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    ENV.get_or_init(|| std::env::var("XCL_MOUSE_SENS").ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| *v > 0.0))
        .unwrap_or_else(|| crate::state::config().mouse_sensitivity)
}

fn mouse_invert_y() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| env_flag("XCL_MOUSE_INVERT_Y"))
}

// ---------------------------------------------------------------------------------------
// the key map (pure, tested)
// ---------------------------------------------------------------------------------------

/// A key: a virtual key, or a physical key position (scancode, QWERTY virtual key fallback)
/// so that letters stay where they are on AZERTY and the like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Vk(i32),
    Sc(u32, i32),
}

/// What a key presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Button(u32),
    LeftTrigger,
    RightTrigger,
}

pub type Keymap = Vec<(Key, Action)>;

/// The default layout (see the table at the top). Parsed like a user's file.
pub const DEFAULT_KEYMAP: &str = "\
SHIFT=A
SPACE=X
E=Y
F=Y
Q=LB
R=RB
CTRL=LS
ENTER=A
ESC=B
BACKSPACE=B
TAB=VIEW
P=MENU
UP=UP
DOWN=DOWN
LEFT=LEFT
RIGHT=RIGHT
";

fn letter_scancode(c: char) -> Option<(u32, i32)> {
    let rows = [("QWERTYUIOP", 0x10u32), ("ASDFGHJKL", 0x1E), ("ZXCVBNM", 0x2C)];
    rows.iter().find_map(|(row, base)| row.find(c).map(|i| (base + i as u32, c as i32)))
}

pub fn parse_key(name: &str) -> Option<Key> {
    let n = name.trim().to_ascii_uppercase();
    let mut chars = n.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if let Some((sc, vk)) = letter_scancode(c) {
            return Some(Key::Sc(sc, vk));
        }
        if c.is_ascii_digit() {
            return Some(Key::Vk(c as i32));
        }
    }
    if let Some(f) = n.strip_prefix('F').and_then(|d| d.parse::<i32>().ok()) {
        if (1..=12).contains(&f) {
            return Some(Key::Vk(0x70 + f - 1));
        }
    }
    let vk = match n.as_str() {
        "SPACE" => 0x20,
        "ENTER" | "RETURN" => 0x0D,
        "ESC" | "ESCAPE" => 0x1B,
        "BACKSPACE" => 0x08,
        "TAB" => 0x09,
        "CAPSLOCK" => 0x14,
        "SHIFT" | "LSHIFT" => 0xA0,
        "RSHIFT" => 0xA1,
        "CTRL" | "LCTRL" => 0xA2,
        "RCTRL" => 0xA3,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        _ => return None,
    };
    Some(Key::Vk(vk))
}

/// `None`: not a button name; `Some(None)`: NONE (unbind).
pub fn parse_action(name: &str) -> Option<Option<Action>> {
    let b = |v| Some(Some(Action::Button(v)));
    match name.trim().to_ascii_uppercase().as_str() {
        "A" => b(A),
        "B" => b(B),
        "X" => b(X),
        "Y" => b(Y),
        "LB" => b(LEFT_SHOULDER),
        "RB" => b(RIGHT_SHOULDER),
        "LS" => b(LEFT_STICK),
        "RS" => b(RIGHT_STICK),
        "VIEW" => b(VIEW),
        "MENU" => b(MENU),
        "UP" | "DPAD_UP" => b(DPAD_UP),
        "DOWN" | "DPAD_DOWN" => b(DPAD_DOWN),
        "LEFT" | "DPAD_LEFT" => b(DPAD_LEFT),
        "RIGHT" | "DPAD_RIGHT" => b(DPAD_RIGHT),
        "LT" => Some(Some(Action::LeftTrigger)),
        "RT" => Some(Some(Action::RightTrigger)),
        "NONE" => Some(None),
        _ => None,
    }
}

/// Apply `key=button` lines of `text` to `map`: a key listed replaces its earlier binding.
/// Returns a message for each line that could not be used (blank and `#` lines are fine).
pub fn parse_keymap_into(map: &mut Keymap, text: &str) -> Vec<String> {
    let mut errors = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, a)) = line.split_once('=') else {
            errors.push(format!("line {}: expected KEY=BUTTON, got \"{line}\"", i + 1));
            continue;
        };
        let (Some(key), Some(action)) = (parse_key(k), parse_action(a)) else {
            errors.push(format!("line {}: unknown key or button in \"{line}\"", i + 1));
            continue;
        };
        map.retain(|(existing, _)| *existing != key);
        if let Some(action) = action {
            map.push((key, action));
        }
    }
    errors
}

/// The default layout, then the file named by `XCL_KEYMAP` on top.
fn keymap() -> &'static Keymap {
    static MAP: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = Keymap::new();
        parse_keymap_into(&mut map, DEFAULT_KEYMAP);
        if let Ok(path) = std::env::var("XCL_KEYMAP") {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    for e in parse_keymap_into(&mut map, &text) {
                        crate::xlog_warn!("XCL_KEYMAP {path}: {e}");
                    }
                    crate::xlog_info!("XCL_KEYMAP: key map from {path}");
                }
                Err(e) => crate::xlog_warn!("XCL_KEYMAP {path}: {e}; the default layout stays"),
            }
        }
        map
    })
}

/// The mouse's velocity, smoothed: counts since the last update over `dt` seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Velocity {
    pub x: f32,
    pub y: f32,
}

impl Velocity {
    pub fn update(&mut self, dx: i32, dy: i32, dt: f32) {
        let dt = dt.clamp(0.001, 0.1);
        let a = 1.0 - (-dt / SMOOTHING).exp();
        self.x += a * (dx as f32 / dt - self.x);
        self.y += a * (dy as f32 / dt - self.y);
    }
}

/// press time of a wheel notch and the gap after it (µs): long enough for a title that reads
/// the gamepad once a frame to see each press
const NOTCH_HOLD_US: u64 = 50_000;
const NOTCH_GAP_US: u64 = 50_000;

/// Wheel notches as D-pad up / down presses, one after the other.
#[derive(Debug, Default)]
pub struct Wheel {
    /// notches still to press (+ up, - down)
    pending: i32,
    /// the press under way: (button, µs since it started)
    current: Option<(u32, u64)>,
}

impl Wheel {
    pub fn notches(&mut self, n: i32) {
        self.pending = (self.pending + n).clamp(-8, 8);
    }

    /// The D-pad button held now, `dt` seconds after the last call.
    pub fn step(&mut self, dt: f32) -> u32 {
        if let Some((b, t)) = self.current.as_mut() {
            *t += (dt.max(0.0) * 1e6).round() as u64;
            if *t < NOTCH_HOLD_US {
                return *b;
            }
            if *t < NOTCH_HOLD_US + NOTCH_GAP_US {
                return 0;
            }
            self.current = None;
        }
        if self.pending == 0 {
            return 0;
        }
        let b = if self.pending > 0 { DPAD_UP } else { DPAD_DOWN };
        self.pending -= self.pending.signum();
        self.current = Some((b, 0));
        b
    }
}

// ---------------------------------------------------------------------------------------
// input from the PC
// ---------------------------------------------------------------------------------------

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
    fn GetKeyboardLayout(id_thread: u32) -> *mut c_void;
    fn MapVirtualKeyExW(code: u32, map_type: u32, dwhkl: *mut c_void) -> u32;
    fn RegisterRawInputDevices(devices: *const RawInputDevice, n: u32, size: u32) -> i32;
    fn GetRawInputData(input: isize, command: u32, data: *mut c_void, size: *mut u32, header: u32) -> u32;
    fn ClipCursor(rect: *const Rect) -> i32;
    fn GetClientRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
    fn ClientToScreen(hwnd: *mut c_void, point: *mut Point) -> i32;
    fn SetCursor(cursor: *mut c_void) -> *mut c_void;
}

#[repr(C)]
struct RawInputDevice {
    usage_page: u16,
    usage: u16,
    flags: u32,
    target: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}

/// RAWINPUTHEADER (24 bytes) + RAWMOUSE
#[repr(C)]
#[derive(Default)]
struct RawMouseInput {
    kind: u32,
    size: u32,
    device: usize,
    wparam: usize,
    flags: u16,
    _pad: u16,
    button_flags: u16,
    button_data: u16,
    raw_buttons: u32,
    last_x: i32,
    last_y: i32,
    extra: u32,
}

const WM_INPUT: u32 = 0x00FF;
const WM_ACTIVATE: u32 = 0x0006;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_SETCURSOR: u32 = 0x0020;
const WM_SIZE: u32 = 0x0005;
const WM_MOVE: u32 = 0x0003;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_ENTERSIZEMOVE: u32 = 0x0231;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPEMOUSE: u32 = 0;
const MOUSE_MOVE_ABSOLUTE: u16 = 1;
const RI_MOUSE_WHEEL: u16 = 0x0400;
const HTCLIENT: isize = 1;
const MAPVK_VSC_TO_VK_EX: u32 = 3;

/// Physical AT Set 1 keyboard scancodes for the left-hand cluster, paired with their
/// QWERTY virtual-key fallbacks if `MapVirtualKeyExW` returns 0.
#[cfg(test)]
const SC_Q: (u32, i32) = (0x10, 0x51);
const SC_W: (u32, i32) = (0x11, 0x57);
#[cfg(test)]
const SC_E: (u32, i32) = (0x12, 0x45);
const SC_A: (u32, i32) = (0x1E, 0x41);
const SC_S: (u32, i32) = (0x1F, 0x53);
const SC_D: (u32, i32) = (0x20, 0x44);

/// the XCL window (raw input goes there; the cursor is held in it)
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// the mouse is captured: a click captured it; Alt, Alt+Tab or a lost focus release it
static CAPTURED: AtomicBool = AtomicBool::new(false);
/// the click that captured the mouse is still down: it is not a shot
static SWALLOW_LMB: AtomicBool = AtomicBool::new(false);

/// The mouse is captured (the window title says how to capture it otherwise).
pub fn captured() -> bool {
    CAPTURED.load(Ordering::Acquire)
}

/// Suffix of the window title while the mouse is free ("" when captured or disabled).
pub fn title_hint() -> &'static str {
    if enabled() && WINDOW.load(Ordering::Acquire) != 0 && !captured() {
        " (click to capture mouse, Alt to release)"
    } else {
        ""
    }
}
/// the user is dragging or resizing the window (WM_ENTERSIZEMOVE … WM_EXITSIZEMOVE):
/// the cursor must stay free, or the 1-pixel clip teleports it back mid-drag
static SIZING: AtomicBool = AtomicBool::new(false);
/// mouse counts not yet taken by `apply`
static DX: AtomicI32 = AtomicI32::new(0);
static DY: AtomicI32 = AtomicI32::new(0);
/// wheel movement not yet taken (WHEEL_DELTA = 120 a notch)
static WHEEL: AtomicI32 = AtomicI32::new(0);

struct Mouse {
    last: Instant,
    velocity: Velocity,
    wheel: Wheel,
    wheel_rest: i32,
}

impl Mouse {
    fn reset(&mut self, now: Instant) {
        self.last = now;
        self.velocity = Velocity::default();
        self.wheel = Wheel::default();
        self.wheel_rest = 0;
    }
}

static MOUSE: Mutex<Option<Mouse>> = Mutex::new(None);

fn reset_mouse_state() {
    DX.store(0, Ordering::Relaxed);
    DY.store(0, Ordering::Relaxed);
    WHEEL.store(0, Ordering::Relaxed);
    let mut g = MOUSE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = g.as_mut() {
        m.reset(Instant::now());
    }
}

pub fn enabled() -> bool {
    crate::state::config().keyboard_mouse
}

/// The XCL window is up: the mouse reports to it.
pub fn attach(hwnd: *mut c_void) {
    if !enabled() {
        return;
    }
    WINDOW.store(hwnd as isize, Ordering::Release);
    // generic desktop mouse
    let dev = RawInputDevice { usage_page: 1, usage: 2, flags: 0, target: hwnd };
    let ok = unsafe { RegisterRawInputDevices(&dev, 1, std::mem::size_of::<RawInputDevice>() as u32) } != 0;
    crate::xlog_info!(
        "keyboard and mouse play as the gamepad (WASD left stick, mouse right stick once captured by a click, mouse buttons triggers, wheel D-pad){}",
        if ok { "" } else { " – raw mouse input unavailable, the mouse is not read" }
    );
}

unsafe fn hold_cursor(hwnd: *mut c_void, on: bool) {
    // a drag/resize loop owns the cursor: clipping it to the window's centre would teleport
    // it back on every WM_MOVE of the drag
    if on && SIZING.load(Ordering::Acquire) {
        if CAPTURED.swap(false, Ordering::AcqRel) {
            ClipCursor(std::ptr::null());
        }
        return;
    }
    if !on {
        if CAPTURED.swap(false, Ordering::AcqRel) {
            ClipCursor(std::ptr::null());
        }
        return;
    }
    // the cursor stays in the client area, hidden (WM_SETCURSOR); raw deltas are not clipped
    let mut r = Rect::default();
    GetClientRect(hwnd, &mut r);
    let mut tl = Point { x: r.left, y: r.top };
    let mut br = Point { x: r.right, y: r.bottom };
    ClientToScreen(hwnd, &mut tl);
    ClientToScreen(hwnd, &mut br);
    let at = Rect { left: tl.x, top: tl.y, right: br.x, bottom: br.y };
    ClipCursor(&at);
    CAPTURED.store(true, Ordering::Release);
    SetCursor(std::ptr::null_mut());
}

/// The process is ending: the cursor is let go (a held cursor outlives its window).
pub fn release() {
    unsafe { hold_cursor(WINDOW.load(Ordering::Acquire) as *mut c_void, false) };
}

/// The XCL window's messages: true when handled (the window procedure returns 0).
pub unsafe fn on_message(hwnd: *mut c_void, msg: u32, wp: usize, lp: isize) -> bool {
    if WINDOW.load(Ordering::Acquire) != hwnd as isize {
        return false;
    }
    match msg {
        WM_INPUT => {
            let mut raw = RawMouseInput::default();
            let mut size = std::mem::size_of::<RawMouseInput>() as u32;
            let n = GetRawInputData(lp, RID_INPUT, &mut raw as *mut _ as *mut c_void, &mut size, 24);
            if n != u32::MAX && n as usize >= std::mem::size_of::<RawMouseInput>() && raw.kind == RIM_TYPEMOUSE && CAPTURED.load(Ordering::Acquire) {
                if raw.flags & MOUSE_MOVE_ABSOLUTE == 0 {
                    DX.fetch_add(raw.last_x, Ordering::Relaxed);
                    DY.fetch_add(raw.last_y, Ordering::Relaxed);
                }
                if raw.button_flags & RI_MOUSE_WHEEL != 0 {
                    WHEEL.fetch_add(raw.button_data as i16 as i32, Ordering::Relaxed);
                }
            }
            false // DefWindowProc cleans up
        }
        WM_ACTIVATE => {
            // activation does not capture (a click does); losing it releases
            if wp & 0xFFFF == 0 {
                hold_cursor(hwnd, false);
                reset_mouse_state();
            }
            crate::qol::focus_changed(wp & 0xFFFF != 0);
            false
        }
        WM_KILLFOCUS => {
            hold_cursor(hwnd, false);
            reset_mouse_state();
            false
        }
        WM_ENTERSIZEMOVE => {
            SIZING.store(true, Ordering::Release);
            hold_cursor(hwnd, false);
            reset_mouse_state();
            false
        }
        WM_EXITSIZEMOVE => {
            // the drag released the mouse: a click captures it again
            SIZING.store(false, Ordering::Release);
            false
        }
        WM_LBUTTONDOWN => {
            if !CAPTURED.load(Ordering::Acquire) && GetForegroundWindow() == hwnd {
                reset_mouse_state();
                hold_cursor(hwnd, true);
                if CAPTURED.load(Ordering::Acquire) {
                    SWALLOW_LMB.store(true, Ordering::Release);
                }
            }
            false
        }
        WM_SIZE | WM_MOVE => {
            // the clip follows the window while captured
            if CAPTURED.load(Ordering::Acquire) && GetForegroundWindow() == hwnd {
                CAPTURED.store(false, Ordering::Release);
                hold_cursor(hwnd, true);
            }
            false
        }
        WM_SETCURSOR if CAPTURED.load(Ordering::Acquire) && (lp & 0xFFFF) == HTCLIENT => {
            SetCursor(std::ptr::null_mut());
            true
        }
        _ => false,
    }
}

fn key(vk: i32) -> bool {
    // SAFETY: `GetAsyncKeyState` accepts any virtual-key code with no memory invariants.
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

/// Active keyboard layout (`HKL`) of `hwnd`'s thread, falling back to the current thread.
fn keyboard_layout_for_window(hwnd: *mut c_void) -> *mut c_void {
    // SAFETY: `GetWindowThreadProcessId` and `GetKeyboardLayout` accept null/valid handles
    // and thread ID 0 (calling thread) with no pointer aliasing requirements.
    unsafe {
        let tid = if hwnd.is_null() { 0 } else { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
        let hkl = GetKeyboardLayout(tid);
        if hkl.is_null() && tid != 0 {
            GetKeyboardLayout(0)
        } else {
            hkl
        }
    }
}

/// Translate a physical hardware scancode to a virtual-key code for `layout`, falling
/// back to `fallback_vk` (the QWERTY VK) when `layout` is null or unmapped.
pub(crate) fn scancode_to_vk(scancode: u32, fallback_vk: i32, layout: *mut c_void) -> i32 {
    if layout.is_null() {
        return fallback_vk;
    }
    // SAFETY: `MapVirtualKeyExW` is a pure Win32 lookup on `scancode` and `layout`, returning 0
    // when no translation exists.
    let vk = unsafe { MapVirtualKeyExW(scancode, MAPVK_VSC_TO_VK_EX, layout) };
    if vk != 0 {
        vk as i32
    } else {
        fallback_vk
    }
}

/// The keyboard and the mouse on top of `s` (the real controller), while the XCL window
/// has the focus.
pub fn apply(s: &mut GamepadState) {
    if !enabled() {
        return;
    }
    let hwnd = WINDOW.load(Ordering::Acquire);
    let focused = hwnd != 0 && unsafe { GetForegroundWindow() } as isize == hwnd;
    if !focused {
        // Alt+Tab, another window: the cursor is free, nothing is pressed
        // SAFETY: `hold_cursor(..., false)` only releases the cursor clip when captured.
        unsafe { hold_cursor(hwnd as *mut c_void, false) };
        reset_mouse_state();
        return;
    }
    let layout = keyboard_layout_for_window(hwnd as *mut c_void);
    let sc_key = |(sc, fb): (u32, i32)| key(scancode_to_vk(sc, fb, layout));
    // Alt releases the mouse (Esc stays B for the game); a click captures it again
    if CAPTURED.load(Ordering::Acquire) && key(0x12) {
        // SAFETY: releases the cursor clip only.
        unsafe { hold_cursor(hwnd as *mut c_void, false) };
        reset_mouse_state();
    }
    // movement keys: physical WASD cluster -> left stick (a deflected real stick wins)
    let (lx, ly) = pick_stick((s.left_x, s.left_y), stick_from_keys(sc_key(SC_W), sc_key(SC_A), sc_key(SC_S), sc_key(SC_D)));
    s.left_x = lx;
    s.left_y = ly;
    for (k, action) in keymap() {
        let down = match *k {
            Key::Vk(vk) => key(vk),
            Key::Sc(sc, fb) => sc_key((sc, fb)),
        };
        if !down {
            continue;
        }
        match *action {
            Action::Button(b) => s.buttons |= b,
            Action::LeftTrigger => s.left_trigger = 1.0,
            Action::RightTrigger => s.right_trigger = 1.0,
        }
    }
    if key(0x04) && CAPTURED.load(Ordering::Acquire) {
        s.buttons |= RIGHT_STICK; // middle mouse button
    }
    if SWALLOW_LMB.load(Ordering::Acquire) {
        if !key(0x01) {
            SWALLOW_LMB.store(false, Ordering::Release);
        }
    } else if key(0x01) && CAPTURED.load(Ordering::Acquire) {
        s.right_trigger = 1.0;
    }
    if key(0x02) && CAPTURED.load(Ordering::Acquire) {
        s.left_trigger = 1.0;
    }
    let mut g = MOUSE.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(|| Mouse { last: Instant::now(), velocity: Velocity::default(), wheel: Wheel::default(), wheel_rest: 0 });
    let now = Instant::now();
    let dt = now.duration_since(m.last).as_secs_f32();
    m.last = now;
    m.velocity.update(DX.swap(0, Ordering::Relaxed), DY.swap(0, Ordering::Relaxed), dt);
    let (rx, ry) = pick_stick((s.right_x, s.right_y), stick_from_mouse(m.velocity.x, m.velocity.y, mouse_sens(), mouse_invert_y()));
    s.right_x = rx;
    s.right_y = ry;
    m.wheel_rest += WHEEL.swap(0, Ordering::Relaxed);
    let notches = m.wheel_rest / 120;
    m.wheel_rest -= notches * 120;
    m.wheel.notches(notches);
    s.buttons |= m.wheel.step(dt.min(0.1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_keys_make_a_unit_stick() {
        assert_eq!(stick_from_keys(false, false, false, false), (0.0, 0.0));
        assert_eq!(stick_from_keys(true, false, false, false), (0.0, 1.0));
        assert_eq!(stick_from_keys(false, true, false, false), (-1.0, 0.0));
        let (x, y) = stick_from_keys(true, false, false, true);
        assert!((x - y).abs() < 1e-6 && (x * x + y * y - 1.0).abs() < 1e-6);
        // opposite keys cancel
        assert_eq!(stick_from_keys(true, true, true, true), (0.0, 0.0));
    }

    #[test]
    fn mouse_speed_becomes_a_deflection() {
        // resting
        assert_eq!(stick_from_mouse(5.0, 0.0, 1.0, false), (0.0, 0.0));
        // slow movement starts past the dead zone, to the right
        let (x, y) = stick_from_mouse(100.0, 0.0, 1.0, false);
        assert!(x > START && x < 0.2 && y == 0.0);
        // fast movement: full deflection (clamped); the mouse moving down tilts the stick down
        let (x, y) = stick_from_mouse(0.0, 10_000.0, 1.0, false);
        assert!(x == 0.0 && (y + 1.0).abs() < 1e-6);
        let (x, y) = stick_from_mouse(1e9, 1e9, 1.0, false);
        assert!((x * x + y * y).sqrt() <= 1.0 + 1e-6);
        // sensitivity scales the speed
        let (a, _) = stick_from_mouse(1000.0, 0.0, 1.0, false);
        let (b, _) = stick_from_mouse(1000.0, 0.0, 2.0, false);
        assert!(b > a);
        // no acceleration: doubling the speed doubles the deflection above the start offset
        let (c, _) = stick_from_mouse(500.0, 0.0, 1.0, false);
        let (d, _) = stick_from_mouse(1000.0, 0.0, 1.0, false);
        assert!(((d - START) / (c - START) - 2.0).abs() < 1e-4);
        // invert flips the vertical axis only
        let (x, y) = stick_from_mouse(300.0, 300.0, 1.0, false);
        let (xi, yi) = stick_from_mouse(300.0, 300.0, 1.0, true);
        assert!(x == xi && y == -yi && y < 0.0);
    }

    #[test]
    fn stopping_the_mouse_recenters_the_stick() {
        let mut v = Velocity::default();
        for _ in 0..30 {
            v.update(20, 0, 0.004);
        }
        assert!(stick_from_mouse(v.x, v.y, 1.0, false).0 > 0.5);
        for _ in 0..100 {
            v.update(0, 0, 0.004);
        }
        assert_eq!(stick_from_mouse(v.x, v.y, 1.0, false), (0.0, 0.0));
    }

    #[test]
    fn a_deflected_controller_stick_wins() {
        assert_eq!(pick_stick((0.5, 0.0), (0.0, 1.0)), (0.5, 0.0));
        assert_eq!(pick_stick((0.05, 0.0), (0.0, 1.0)), (0.0, 1.0)); // pad at rest
        assert_eq!(pick_stick((0.05, 0.02), (0.0, 0.0)), (0.05, 0.02));
    }

    #[test]
    fn default_keymap_parses_cleanly() {
        let mut m = Keymap::new();
        assert!(parse_keymap_into(&mut m, DEFAULT_KEYMAP).is_empty());
        assert!(m.contains(&(Key::Vk(0xA0), Action::Button(A))));
        assert!(m.contains(&(Key::Vk(0x20), Action::Button(X))));
        assert!(m.contains(&(Key::Sc(0x12, 0x45), Action::Button(Y)))); // E
        assert!(m.contains(&(Key::Sc(0x21, 0x46), Action::Button(Y)))); // F
        assert!(m.contains(&(Key::Vk(0x1B), Action::Button(B))));
    }

    #[test]
    fn keymap_file_overrides_defaults() {
        let mut m = Keymap::new();
        parse_keymap_into(&mut m, DEFAULT_KEYMAP);
        let errors = parse_keymap_into(
            &mut m,
            "# my layout\n\
             space = a   # jump on A\n\
             f=none\n\
             g = rt\n\
             F5=lt\n\
             bogus=A\n\
             tab=nowhere\n\
             no equals sign\n",
        );
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].contains("line 6") && errors[1].contains("line 7") && errors[2].contains("line 8"));
        assert!(m.contains(&(Key::Vk(0x20), Action::Button(A))));
        assert!(!m.contains(&(Key::Vk(0x20), Action::Button(X))));
        assert!(!m.iter().any(|(k, _)| *k == Key::Sc(0x21, 0x46))); // F unbound
        assert!(m.contains(&(Key::Sc(0x22, 0x47), Action::RightTrigger)));
        assert!(m.contains(&(Key::Vk(0x74), Action::LeftTrigger)));
        // untouched defaults stay; the bad tab line left TAB=VIEW alone
        assert!(m.contains(&(Key::Vk(0x09), Action::Button(VIEW))));
    }

    #[test]
    fn key_and_button_names() {
        assert_eq!(parse_key("w"), Some(Key::Sc(0x11, 0x57)));
        assert_eq!(parse_key(" Z "), Some(Key::Sc(0x2C, 0x5A)));
        assert_eq!(parse_key("5"), Some(Key::Vk(0x35)));
        assert_eq!(parse_key("F12"), Some(Key::Vk(0x7B)));
        assert_eq!(parse_key("F13"), None);
        assert_eq!(parse_key("alt"), None);
        assert_eq!(parse_action("lb"), Some(Some(Action::Button(LEFT_SHOULDER))));
        assert_eq!(parse_action("NONE"), Some(None));
        assert_eq!(parse_action("jump"), None);
    }

    #[test]
    fn velocity_follows_the_counts_and_settles() {
        let mut v = Velocity::default();
        for _ in 0..50 {
            v.update(8, -4, 0.004); // 2000 counts/s right, 1000 up
        }
        assert!((v.x - 2000.0).abs() < 20.0 && (v.y + 1000.0).abs() < 10.0);
        for _ in 0..50 {
            v.update(0, 0, 0.004);
        }
        assert!(v.x.abs() < REST_SPEED && v.y.abs() < REST_SPEED);
    }

    #[test]
    fn wheel_notches_press_bumpers_one_by_one() {
        let mut w = Wheel::default();
        w.notches(2);
        w.notches(-1); // net one notch up
        let mut presses = Vec::new();
        let mut last = 0;
        for _ in 0..100 {
            let b = w.step(0.01);
            if b != 0 && b != last {
                presses.push(b);
            }
            last = b;
        }
        assert_eq!(presses, vec![DPAD_UP]);
        w.notches(-2);
        let held: Vec<u32> = (0..30).map(|_| w.step(0.01)).collect();
        // held for NOTCH_HOLD, released for NOTCH_GAP, then the second press
        assert_eq!(held.iter().filter(|&&b| b == DPAD_DOWN).count(), 10);
        assert!(held[..5].iter().all(|&b| b == DPAD_DOWN) && held[5..10].iter().all(|&b| b == 0));
    }

    #[test]
    fn scancodes_map_to_distinct_vks_and_fall_back_safely() {
        const MAPVK_VK_TO_VSC: u32 = 0;
        const MAPVK_VSC_TO_VK: u32 = 1;
        let layout = keyboard_layout_for_window(std::ptr::null_mut());
        assert!(!layout.is_null(), "current thread should have an active Windows keyboard layout");

        let cluster = [SC_Q, SC_W, SC_E, SC_A, SC_S, SC_D];
        let mut seen_vks = Vec::new();
        for &(sc, fb) in &cluster {
            let vk = scancode_to_vk(sc, fb, layout);
            // SAFETY: pure Win32 lookup on the current thread's HKL.
            let vk_basic = unsafe { MapVirtualKeyExW(sc, MAPVK_VSC_TO_VK, layout) } as i32;
            assert_eq!(vk, vk_basic, "MAPVK_VSC_TO_VK and MAPVK_VSC_TO_VK_EX should agree for scancode {sc:#x}");
            assert_ne!(vk, 0);
            assert!(!seen_vks.contains(&vk), "scancode {sc:#x} collided with another key's VK {vk:#x}");
            seen_vks.push(vk);

            // Round-trip VK -> scancode verifies we resolved the physical key position.
            // SAFETY: pure Win32 lookup on the current thread's HKL.
            let roundtrip = unsafe { MapVirtualKeyExW(vk as u32, MAPVK_VK_TO_VSC, layout) };
            assert_eq!(roundtrip, sc, "VK {vk:#x} should round-trip back to physical scancode {sc:#x}");
        }

        // Verify MAPVK_VSC_TO_VK_EX distinguishes Left Shift (0x2A -> VK_LSHIFT 0xA0) whereas
        // MAPVK_VSC_TO_VK collapses it to generic VK_SHIFT (0x10).
        // SAFETY: pure Win32 lookup on the current thread's HKL.
        let lshift_basic = unsafe { MapVirtualKeyExW(0x2A, MAPVK_VSC_TO_VK, layout) };
        let lshift_ex = scancode_to_vk(0x2A, 0xA0, layout) as u32;
        assert_eq!(lshift_basic, 0x10);
        assert_eq!(lshift_ex, 0xA0);

        // Fallback when layout is null or scancode has no mapping.
        assert_eq!(scancode_to_vk(SC_W.0, SC_W.1, std::ptr::null_mut()), SC_W.1);
        assert_eq!(scancode_to_vk(0xFFFF, SC_Q.1, layout), SC_Q.1);
    }

    #[test]
    fn focus_loss_resets_mouse_velocity_and_wheel_state() {
        // Seed raw deltas and active MOUSE state (velocity, in-flight & pending wheel notches, remainder).
        DX.store(250, Ordering::Relaxed);
        DY.store(-180, Ordering::Relaxed);
        WHEEL.store(240, Ordering::Relaxed);
        {
            let mut g = MOUSE.lock().unwrap_or_else(|e| e.into_inner());
            let mut v = Velocity::default();
            for _ in 0..20 {
                v.update(40, -20, 0.004);
            }
            let mut w = Wheel::default();
            w.notches(3);
            assert_eq!(w.step(0.01), DPAD_UP);
            *g = Some(Mouse {
                last: Instant::now(),
                velocity: v,
                wheel: w,
                wheel_rest: 90,
            });
            let m = g.as_ref().unwrap();
            assert!(m.velocity.x.abs() > REST_SPEED && m.velocity.y.abs() > REST_SPEED);
        }

        // With no focused XCL window (`WINDOW == 0`), `apply` treats focus as lost and resets everything.
        let mut s = GamepadState::default();
        apply(&mut s);
        assert_eq!(s, GamepadState::default());
        assert_eq!(DX.load(Ordering::Relaxed), 0);
        assert_eq!(DY.load(Ordering::Relaxed), 0);
        assert_eq!(WHEEL.load(Ordering::Relaxed), 0);

        let mut g = MOUSE.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.as_mut().expect("mouse state should remain allocated");
        assert_eq!(m.velocity, Velocity::default());
        assert_eq!(m.wheel_rest, 0);
        for _ in 0..20 {
            assert_eq!(m.wheel.step(0.01), 0, "stale wheel notches must not fire after refocusing");
        }
    }
}
