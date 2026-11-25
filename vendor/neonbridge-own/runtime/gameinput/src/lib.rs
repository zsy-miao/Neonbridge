//! GameInput v0 polling and ordered snapshots. The SDK supplies the typed COM tables.
mod abi;
mod callbacks;
mod sample;

use sample::{DesktopPad, KEYS, MOUSE, MouseMotion, PAD, PORTS, Pad, Sample};
use std::collections::VecDeque;
use std::ptr;
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicU32, Ordering},
};
use std::time::Instant;
use windows_sys::Win32::Foundation::{E_INVALIDARG, E_NOINTERFACE, E_POINTER, S_OK};
use windows_sys::core::{GUID, IID_IUnknown};

const NO_READING: i32 = 0x838a0003u32 as i32;
const STALE: i32 = 0x838a0004u32 as i32;
const VIRTUAL: usize = PORTS + 2;
const DEVICES: usize = VIRTUAL + 1;
const HISTORY: usize = 256;
const DEVICE_HISTORY: usize = 32;
fn device_history() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("TUXBOX_GI_DEVICE_HISTORY").is_none_or(|v| v != "0"));
    *ON
}
const INPUT_IID: GUID = GUID::from_u128(0x11be2a7e_4254_445a_9c09_ffc40f006918);
const DEVICE_IID: GUID = GUID::from_u128(0x31dd86fb_4c1b_408a_868f_439b3cd47125);
const READING_IID: GUID = GUID::from_u128(0x2156947a_e1fa_4de0_a30b_d812931dbd8d);


#[repr(C)]
struct Object {
    table: &'static u8,
    value: Value,
}

enum Value {
    Input(Mutex<Box<Pool>>),
    Reading(Reading),
    Device { slot: usize, status: AtomicU32 },
}

struct Reading {
    device: Arc<Object>,
    kind: u32,
    number: u64,
    timestamp: u64,
    sample: Sample,
}

impl Object {
    fn reading(&self) -> &Reading {
        let Value::Reading(data) = &self.value else {
            unreachable!("reading vtable")
        };
        data
    }
}

struct Pool {
    devices: [Arc<Object>; DEVICES],
    latest: [Option<Arc<Object>>; DEVICES],
    history: VecDeque<Arc<Object>>,
    /// The last DEVICE_HISTORY readings of each device (TUXBOX_GI_DEVICE_HISTORY).
    device_history: [VecDeque<Arc<Object>>; DEVICES],
    number: u64,
    mouse: MouseMotion,
    scan_at: u64,
    pads: [Option<Pad>; PORTS],
    desktop: Option<DesktopPad>,
    auto_start: AutoStart,
}

/// One injected A press so the title's player-selection prompt ("Account
/// Picker / Screen Narration / <gamertag>"), which waits on A, passes without
/// a key press. The prompt follows the silent sign-in (xgameruntime sets
/// TUXBOX_SIGNED_IN at XUserAddAsync), so the press waits for that plus
/// `delay`, holds A across several polls (at least 400 ms) and releases. If the
/// title did not read the A while it was held, it tries again after 3 s, at
/// most 3 times; once the title has read it, it never presses again, so menus
/// never see an injected A. A real pad button stops it too.
/// TUXBOX_AUTO_START=0 turns it off; a number sets the delay in seconds after
/// sign-in (default 3).
enum AutoStart {
    Off,
    Waiting { delay: u64, since: Option<u64>, tries: u32 },
    Pressing { since: u64, polls: u32, tries: u32 },
    Done,
}

const TRIES: u32 = 3;

/// Buttons injected by `neonbridge ctl press/hold` and when they end (gi_time µs; u64::MAX holds).
static INJECT_BUTTONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static INJECT_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The control channel's pad input: `buttons` for `ms` milliseconds (0 releases,
/// u32::MAX holds until released). Returns 1 when a pad exists to carry it.
#[unsafe(no_mangle)]
pub extern "system" fn TuxBoxPadInject(buttons: u32, ms: u32) -> u32 {
    use std::sync::atomic::Ordering::Relaxed;
    let until = match ms {
        0 => 0,
        u32::MAX => u64::MAX,
        _ => gi_time() + u64::from(ms) * 1000,
    };
    INJECT_BUTTONS.store(buttons, Relaxed);
    INJECT_UNTIL.store(until, Relaxed);
    eprintln!("TuxBox gameinput: ctl: buttons {buttons:#x} {}", match ms { 0 => "released".into(), u32::MAX => "held".into(), _ => format!("for {ms} ms") });
    1
}

fn injected(now: u64) -> u32 {
    use std::sync::atomic::Ordering::Relaxed;
    if now < INJECT_UNTIL.load(Relaxed) { INJECT_BUTTONS.load(Relaxed) } else { 0 }
}

/// Set by gi_pad when the title reads a pad sample with A held.
static A_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl AutoStart {
    fn from_env() -> Self {
        let delay = match std::env::var("TUXBOX_AUTO_START") {
            Ok(v) if v.trim() == "0" => return Self::Off,
            Ok(v) => v.trim().parse::<u64>().unwrap_or(3),
            Err(_) => 3,
        };
        Self::Waiting { delay: delay * 1_000_000, since: None, tries: 0 }
    }

    /// Advance once per pad poll; `real` is the pad's own button mask. True while A is held.
    fn step(&mut self, now: u64, real: u32) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        if real != 0 && !matches!(self, Self::Off | Self::Done) {
            eprintln!("TuxBox gameinput: auto start: stopped, a real button ({real:#x}) was pressed");
            *self = Self::Done;
            return false;
        }
        match self {
            Self::Off | Self::Done => false,
            Self::Waiting { delay, since, tries } => {
                if since.is_none() {
                    if std::env::var_os("TUXBOX_SIGNED_IN").is_none_or(|v| v.is_empty()) {
                        return false;
                    }
                    *since = Some(now);
                }
                if now - since.unwrap_or(now) < *delay {
                    return false;
                }
                *tries += 1;
                eprintln!("TuxBox gameinput: auto start: pressing A once for the player-selection prompt (try {tries}/{TRIES}; TUXBOX_AUTO_START=0 turns this off)");
                A_READ.store(false, Relaxed);
                *self = Self::Pressing { since: now, polls: 1, tries: *tries };
                true
            }
            Self::Pressing { since, polls, tries } => {
                *polls += 1;
                if *polls <= 4 || now - *since < 400_000 {
                    return true;
                }
                // Debug runs: TUXBOX_AUTO_PRESSES=N keeps pressing A every 3 s, N presses
                // in all, to walk past the menus (default 1: one press the title read).
                let wanted = std::env::var("TUXBOX_AUTO_PRESSES").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1);
                if A_READ.load(Relaxed) && *tries < wanted {
                    eprintln!("TuxBox gameinput: auto start: the title read the A; press {tries} of {wanted} (TUXBOX_AUTO_PRESSES)");
                    *self = Self::Waiting { delay: 3_000_000, since: Some(now), tries: *tries };
                } else if A_READ.load(Relaxed) {
                    eprintln!("TuxBox gameinput: auto start: the title read the A; no further presses");
                    *self = Self::Done;
                } else if *tries >= TRIES.max(wanted) {
                    eprintln!("TuxBox gameinput: auto start: the title never read the A after {TRIES} tries; giving up");
                    *self = Self::Done;
                } else {
                    *self = Self::Waiting { delay: 3_000_000, since: Some(now), tries: *tries };
                }
                false
            }
        }
    }
}

fn kind(slot: usize) -> u32 {
    match slot {
        0 => KEYS,
        1 => MOUSE,
        _ => PAD,
    }
}

impl Pool {
    fn new() -> Self {
        Self {
            devices: std::array::from_fn(|slot| {
                Arc::new(Object {
                    table: abi::DEVICE.first(),
                    value: Value::Device {
                        slot,
                        status: AtomicU32::new(0),
                    },
                })
            }),
            latest: Default::default(),
            history: VecDeque::with_capacity(HISTORY),
            device_history: Default::default(),
            number: 0,
            mouse: MouseMotion::default(),
            scan_at: 0,
            pads: [None; PORTS],
            // On by default, as documented; TUXBOX_KEYBOARD_GAMEPAD=0 leaves only XInput.
            desktop: (std::env::var_os("TUXBOX_KEYBOARD_GAMEPAD").is_none_or(|v| v != "0"))
                .then(DesktopPad::default),
            auto_start: AutoStart::from_env(),
        }
    }

    fn update_devices(&mut self, now: u64, poll_pads: bool) -> (bool, Option<usize>) {
        let scan = now >= self.scan_at;
        if scan || poll_pads {
            for (port, pad) in self.pads.iter_mut().enumerate() {
                if scan || pad.is_some() {
                    *pad = sample::gamepad(port);
                }
            }
            if scan {
                self.scan_at = now + 250_000;
            }
        }
        let active = sample::foreground();
        let desktop_slot = self.desktop.as_ref().map(|_| {
            self.pads
                .iter()
                .position(Option::is_some)
                .map_or(VIRTUAL, |i| i + 2)
        });
        for (slot, device) in self.devices.iter().enumerate() {
            let Value::Device { status, .. } = &device.value else {
                unreachable!()
            };
            let connected = match slot {
                0 | 1 => true,
                VIRTUAL => desktop_slot == Some(VIRTUAL),
                _ => self.pads[slot - 2].is_some(),
            };
            // Pads stay connected and readable unfocused; only keyboard and
            // mouse follow the foreground, as VIctory-fxceless gameinput.rs
            // (GPL-3.0, credit) does. TUXBOX_PAD_ANY_FOCUS=0 gates all.
            let live = active || slot >= 2 && pad_any_focus();
            status.store(
                if connected {
                    1 | (u32::from(live) << 1)
                } else {
                    0
                },
                Ordering::Relaxed,
            );
            if !connected || !live {
                self.latest[slot] = None;
            }
        }
        if !active && let Some(desktop) = self.desktop.as_mut() {
            desktop.abandon_aim();
        }
        (active, desktop_slot)
    }

    fn refresh(&mut self, mask: u32) {
        let now = gi_time();
        let (active, desktop_slot) = self.update_devices(now, mask & PAD != 0);
        // Auto presses (and ctl presses) are synthetic: they reach the title's
        // primary pad whether or not its window is foreground, with every real
        // input left out, so a run the user is not watching still walks the
        // menus (test168: 0 presses, the window behind the user's). Real pad,
        // keyboard and mouse sampling stays gated on the foreground.
        // NEONBRIDGE_AUTO_PRESS_ANY_FOCUS=0 presses only while foreground.
        let synthetic = !active && mask & PAD != 0
            && (!matches!(self.auto_start, AutoStart::Off | AutoStart::Done) || injected(now) != 0)
            && std::env::var("NEONBRIDGE_AUTO_PRESS_ANY_FOCUS").as_deref() != Ok("0");
        let pads_only = !active && !synthetic && pad_any_focus();
        if !active && !synthetic && !pads_only {
            self.mouse.previous = None;
            return;
        }
        if pads_only {
            self.mouse.previous = None;
        }
        if synthetic {
            self.mouse.previous = None;
        }
        // The title's primary pad: the keyboard-merged pad, else the first XInput pad.
        let primary = desktop_slot.or_else(|| self.pads.iter().position(Option::is_some).map(|i| i + 2));
        let real = if synthetic { 0 } else { match primary {
            Some(slot) => self.pads.get(slot.wrapping_sub(2)).copied().flatten().map_or(0, |pad| pad.buttons),
            _ => 0,
        } };
        if synthetic && let Some(slot) = primary
            && let Value::Device { status, .. } = &self.devices[slot].value
        {
            status.store(3, Ordering::Relaxed);
        }
        let press_a = mask & PAD != 0 && primary.is_some() && self.auto_start.step(now, real);
        // The auto press is silent until it fires: say once, 20 s into the
        // title's pad polls, what it still waits for (test168 set
        // TUXBOX_AUTO_PRESSES=12 and logged no press and no reason).
        if mask & PAD != 0 {
            static FIRST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            static TOLD: std::sync::Once = std::sync::Once::new();
            let first = match FIRST.compare_exchange(0, now.max(1), std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed) {
                Ok(_) => now.max(1),
                Err(f) => f,
            };
            if now.saturating_sub(first) >= 20_000_000 && matches!(self.auto_start, AutoStart::Waiting { since: None, .. }) || primary.is_none() && now.saturating_sub(first) >= 20_000_000 {
                TOLD.call_once(|| {
                    let reason = if primary.is_none() {
                        "the title has no primary pad (no keyboard pad, no XInput pad)"
                    } else {
                        "no sign-in yet (TUXBOX_SIGNED_IN unset: XUserAddAsync was never called)"
                    };
                    eprintln!("TuxBox gameinput: auto start: no A pressed 20 s into the pad polls: {reason}");
                });
            }
        }
        let inject = injected(now);
        for slot in 0..DEVICES {
            let kind = kind(slot);
            if kind & mask == 0 || synthetic && primary != Some(slot) || pads_only && slot < 2 {
                continue;
            }
            let data = match slot {
                0 => sample::keyboard(),
                1 => self.mouse.sample(),
                _ if desktop_slot == Some(slot) => {
                    let pad = self
                        .pads
                        .get(slot - 2)
                        .copied()
                        .flatten()
                        .unwrap_or_default();
                    if synthetic { Sample::Pad(Default::default()) } else if pads_only { Sample::Pad(pad) } else { Sample::Pad(self.desktop.as_mut().unwrap().merge(pad)) }
                }
                VIRTUAL => continue,
                _ => match self.pads[slot - 2] {
                    Some(_) if synthetic => Sample::Pad(Default::default()),
                    Some(pad) => Sample::Pad(pad),
                    None => continue,
                },
            };
            let data = match data {
                Sample::Pad(mut pad) if (press_a || inject != 0) && primary == Some(slot) => {
                    if press_a {
                        pad.buttons |= 0x4; // GameInputGamepadA
                    }
                    pad.buttons |= inject;
                    Sample::Pad(pad)
                }
                other => other,
            };
            if self.latest[slot]
                .as_ref()
                .is_some_and(|s| s.reading().sample == data)
            {
                continue;
            }
            self.number += 1;
            let reading = Arc::new(Object {
                table: abi::READING.first(),
                value: Value::Reading(Reading {
                    device: self.devices[slot].clone(),
                    kind,
                    number: self.number,
                    timestamp: now,
                    sample: data,
                }),
            });
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(reading.clone());
            let own = &mut self.device_history[slot];
            if own.len() == DEVICE_HISTORY {
                own.pop_front();
            }
            own.push_back(reading.clone());
            self.latest[slot] = Some(reading);
        }
    }
}

#[unsafe(no_mangle)]
extern "system" fn gi_time() -> u64 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_micros() as u64 + 1
}

// COM callers hold a live reference to this on entry. Arc owns every object
// and the immutable vtable pointer remains at offset zero for its lifetime.
#[unsafe(no_mangle)]
unsafe extern "system" fn gi_add(this: *const Object) -> u32 {
    unsafe { Arc::increment_strong_count(this) };
    let owned = std::mem::ManuallyDrop::new(unsafe { Arc::from_raw(this) });
    Arc::strong_count(&owned) as u32
}

#[unsafe(no_mangle)]
unsafe extern "system" fn gi_release(this: *const Object) -> u32 {
    let owned = unsafe { Arc::from_raw(this) };
    (Arc::strong_count(&owned) - 1) as u32
}

#[unsafe(no_mangle)]
unsafe extern "system" fn gi_query(
    this: &Object,
    iid: Option<&GUID>,
    out: *mut *const Object,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { out.write(ptr::null()) };
    let Some(iid) = iid else { return E_POINTER };
    let kind = match this.value {
        Value::Input(_) => INPUT_IID,
        Value::Device { .. } => DEVICE_IID,
        Value::Reading(_) => READING_IID,
    };
    if ![kind, IID_IUnknown].iter().any(|id| {
        id.data1 == iid.data1
            && id.data2 == iid.data2
            && id.data3 == iid.data3
            && id.data4 == iid.data4
    }) {
        return E_NOINTERFACE;
    }
    unsafe {
        gi_add(this);
        out.write(this)
    };
    S_OK
}

static MANAGER: LazyLock<Arc<Object>> = LazyLock::new(|| {
    Arc::new(Object {
        table: abi::INPUT.first(),
        value: Value::Input(Mutex::new(Box::new(Pool::new()))),
    })
});

fn pool() -> &'static Mutex<Box<Pool>> {
    let Value::Input(state) = &MANAGER.value else {
        unreachable!()
    };
    state
}

#[unsafe(export_name = "GameInputCreate")]
unsafe extern "system" fn create(out: *mut *const Object) -> i32 {
    static CREATED: std::sync::Once = std::sync::Once::new();
    CREATED.call_once(|| eprintln!("TuxBox gameinput: TIMELINE: GameInputCreate"));
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { out.write(Arc::into_raw(Arc::clone(&MANAGER))) };
    S_OK
}

// direction is 0 for the latest snapshot, +1/-1 for an adjacent snapshot.
// Foreign device and reading arguments are compared by identity without dereferencing them.
#[unsafe(no_mangle)]
unsafe extern "system" fn gi_read(
    this: &Object,
    mask: u32,
    device: *const Object,
    reference: *const Object,
    direction: i32,
    out: *mut *const Object,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { out.write(ptr::null()) };
    let Value::Input(state) = &this.value else {
        return E_INVALIDARG;
    };
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if !device.is_null() && !state.devices.iter().any(|d| Arc::as_ptr(d) == device) {
        return E_INVALIDARG;
    }
    let mask = if mask == 0 { KEYS | MOUSE | PAD } else { mask };
    // The auto A press only advances inside the title's pad reads: these two
    // lines tell a title that never reached its pad (stuck before the
    // player-selection prompt) from a press that was blocked (test170: signed
    // in, 83 s, no press line and no reason line).
    static FIRST_READ: std::sync::Once = std::sync::Once::new();
    FIRST_READ.call_once(|| eprintln!("TuxBox gameinput: TIMELINE: first GameInput read (mask {mask:#x})"));
    if mask & PAD != 0 {
        static FIRST_PAD: std::sync::Once = std::sync::Once::new();
        FIRST_PAD.call_once(|| eprintln!("TuxBox gameinput: TIMELINE: first pad read"));
    }
    state.refresh(mask);
    // ztrx gameinput.rs:609-613 / 645-647 (TUXBOX_GI_ZTRX_API).
    if abi::ztrx_api() && !device.is_null() {
        let device_obj = unsafe { &*device };
        if direction == 0 {
            if let Value::Device { status, .. } = &device_obj.value
                && status.load(Ordering::Relaxed) & 1 == 0
            {
                return abi::DEVICE_DISCONNECTED;
            }
        } else if !reference.is_null()
            && let Value::Reading(reading) = unsafe { &(*reference).value }
            && Arc::as_ptr(&reading.device) != device
        {
            return E_INVALIDARG;
        }
    }
    let matches = |r: &&Arc<Object>| {
        let r = r.reading();
        r.kind & mask != 0 && (device.is_null() || Arc::as_ptr(&r.device) == device)
    };
    let found = if direction == 0 {
        state
            .latest
            .iter()
            .flatten()
            .filter(matches)
            .max_by_key(|r| r.reading().number)
    } else {
        if reference.is_null() {
            return E_INVALIDARG;
        }
        // Each device keeps its own last 32 readings and a reference is
        // stale only once older than its device's oldest, as VIctory-fxceless
        // gameinput.rs (GPL-3.0, credit) does: in one shared 256 a busy mouse
        // pushed the pad's previous reading out. TUXBOX_GI_DEVICE_HISTORY=0
        // walks the shared history again.
        if device_history() {
            let reference = unsafe { &*reference };
            let Value::Reading(reading) = &reference.value else { return E_INVALIDARG };
            let Value::Device { slot, .. } = &reading.device.value else { return E_INVALIDARG };
            let own = &state.device_history[*slot];
            if own.front().is_none_or(|oldest| reading.number < oldest.reading().number) {
                return STALE;
            }
            let number = reading.number;
            let found = if direction > 0 {
                own.iter().filter(|r| r.reading().number > number).find(matches)
            } else {
                own.iter().rev().filter(|r| r.reading().number < number).find(matches)
            };
            let Some(found) = found else { return NO_READING };
            unsafe { out.write(Arc::into_raw(found.clone())) };
            return S_OK;
        }
        let Some(index) = state
            .history
            .iter()
            .position(|r| Arc::as_ptr(r) == reference)
        else {
            return STALE;
        };
        if direction > 0 {
            state.history.range(index + 1..).find(matches)
        } else {
            state.history.range(..index).rev().find(matches)
        }
    };
    let Some(reading) = found else {
        return NO_READING;
    };
    unsafe { out.write(Arc::into_raw(reading.clone())) };
    S_OK
}

#[unsafe(no_mangle)]
extern "system" fn gi_kind(this: &Object) -> u32 {
    this.reading().kind
}
#[unsafe(no_mangle)]
extern "system" fn gi_sequence(this: &Object, kind: u32) -> u64 {
    let r = this.reading();
    if (kind == 0 && !abi::ztrx_api()) || kind & r.kind != 0 {
        r.number
    } else {
        0
    }
}
#[unsafe(no_mangle)]
extern "system" fn gi_timestamp(this: &Object) -> u64 {
    this.reading().timestamp
}
#[unsafe(no_mangle)]
unsafe extern "system" fn gi_device(this: &Object, out: *mut *const Object) {
    if !out.is_null() {
        unsafe { out.write(Arc::into_raw(this.reading().device.clone())) };
    }
}
#[unsafe(no_mangle)]
extern "system" fn gi_slot(this: &Object) -> u32 {
    let Value::Device { slot, .. } = &this.value else {
        unreachable!()
    };
    *slot as u32
}
#[unsafe(no_mangle)]
extern "system" fn gi_status(this: &Object) -> u32 {
    let Value::Device { status, .. } = &this.value else {
        unreachable!()
    };
    pool()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .update_devices(gi_time(), false);
    status.load(Ordering::Relaxed)
}
#[unsafe(no_mangle)]
extern "system" fn gi_key_count(this: &Object) -> u32 {
    let Sample::Keys(bits) = &this.reading().sample else {
        return 0;
    };
    bits.iter().map(|b| b.count_ones()).sum()
}
#[unsafe(no_mangle)]
unsafe extern "system" fn gi_keys(this: &Object, count: u32, out: *mut sample::Key) -> u32 {
    let Sample::Keys(bits) = &this.reading().sample else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    let mut used = 0;
    for vk in 0..=u8::MAX {
        if used == count {
            break;
        }
        if bits[usize::from(vk / 8)] & (1 << (vk % 8)) != 0 {
            // SAFETY: the caller supplied space for count GameInput key records.
            unsafe { out.add(used as usize).write(sample::key(vk)) };
            used += 1;
        }
    }
    used
}
#[unsafe(no_mangle)]
unsafe extern "system" fn gi_mouse(this: &Object, out: Option<&mut sample::Mouse>) -> bool {
    if let (Sample::Mouse(data), Some(out)) = (&this.reading().sample, out) {
        *out = *data;
        true
    } else {
        false
    }
}
#[unsafe(no_mangle)]
unsafe extern "system" fn gi_pad(this: &Object, out: Option<&mut sample::Pad>) -> bool {
    if let (Sample::Pad(data), Some(out)) = (&this.reading().sample, out) {
        note_pad(data.buttons);
        if data.buttons & 0x4 != 0 {
            A_READ.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        *out = *data;
        true
    } else {
        false
    }
}

/// TUXBOX_LOG=stderr: that the title reads gamepad state at all, and each new
/// button mask it sees (A = 0x4 is Enter/left click, B = 0x8 is Esc), so a run
/// log shows whether input reached the title.
fn note_pad(buttons: u32) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    if std::env::var_os("TUXBOX_LOG").as_deref() != Some(std::ffi::OsStr::new("stderr")) {
        return;
    }
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 24 && !seen.contains(&buttons) {
        seen.push(buttons);
        if buttons == 0 {
            eprintln!("TuxBox gameinput: title reads gamepad state");
        } else {
            eprintln!("TuxBox gameinput: title read gamepad buttons {buttons:#x}");
        }
    }
}

fn pad_any_focus() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("TUXBOX_PAD_ANY_FOCUS").is_none_or(|v| v != "0"));
    *ON
}
