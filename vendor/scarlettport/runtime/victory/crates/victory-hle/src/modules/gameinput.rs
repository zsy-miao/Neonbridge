//! `gameinput.dll`: input devices from the host. Gamepads come from XInput
//! (host slots 0-3). The keyboard and mouse play as a pad (`controls`): on
//! the first connected host pad, or on one stand-in gamepad while none is
//! connected. With `[controls] enabled = false` the stand-in pad is the old
//! fixed layout (`FALLBACK_BUTTONS`). The keyboard is a device of its own
//! too. A poll thread turns host state into readings (a new one per change)
//! and runs device callbacks on connect and disconnect; a title's pad read
//! makes one too, carrying the mouse motion since its last read.
#![allow(non_snake_case)]
// Title code calls these through the objects, passing pointers the real API
// would trust.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use super::controls::{Controls, Counts};
use super::xasync::{E_INVALIDARG, E_NOTIMPL};
use super::xgameruntime::{HRESULT, S_OK};
use crate::{HleExport, HleModule};

/// The documented errors.
pub const DEVICE_DISCONNECTED: HRESULT = 0x838A_0001_u32 as i32;
pub const DEVICE_NOT_FOUND: HRESULT = 0x838A_0002_u32 as i32;
pub const READING_NOT_FOUND: HRESULT = 0x838A_0003_u32 as i32;
pub const REFERENCE_TOO_OLD: HRESULT = 0x838A_0004_u32 as i32;
const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;

/// Input kinds (bits) our devices have.
const KEYBOARD_KIND: u32 = 0x10;
const GAMEPAD_KIND: u32 = 0x4_0000;
/// Device status of a connected device: connected, input enabled.
const STATUS_CONNECTED: u32 = 0x3;
const ASYNC_ENUMERATION: u32 = 1;

/// Gamepad buttons; the title maps each bit to one of its pad buttons.
pub(super) use victory_core::kbm::{
    A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, PadState, RIGHT, RIGHT_SHOULDER,
    RIGHT_STICK, UP, VIEW, X, Y,
};

/// Devices: host pad slots, the keyboard's stand-in pad, the keyboard.
const PADS: usize = 4;
const FALLBACK: usize = PADS;
const KEYBOARD: usize = PADS + 1;
const DEVICES: usize = PADS + 2;

/// Readings kept per device for next/previous lookups.
const HISTORY: usize = 32;
const POLL: Duration = Duration::from_millis(8);
/// `VICTORY_PACE_INPUT=1`: a title read finding the host's input older than
/// this samples it again first, so a frame starts from input as fresh as
/// possible instead of up to [`POLL`] old.
const LATE_SAMPLE_NS: u64 = 1_000_000;

/// The keyboard's stand-in gamepad (virtual key, button); README lists it.
/// Sticks: W/A/S/D left, I/J/K/L right; triggers Z and C.
const FALLBACK_BUTTONS: [(u8, u32); 14] = [
    (0x1B, MENU), // Esc
    (0x09, VIEW), // Tab
    (0x0D, A),    // Enter
    (0x08, B),    // Backspace
    (b'X', X),
    (b'Y', Y),
    (0x26, UP), // arrows
    (0x28, DOWN),
    (0x25, LEFT),
    (0x27, RIGHT),
    (b'Q', LEFT_SHOULDER),
    (b'E', RIGHT_SHOULDER),
    (b'F', LEFT_STICK),
    (b'H', RIGHT_STICK),
];

/// One key held down: scan code (0xE0xx when extended), character, key.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct KeyState {
    scan_code: u32,
    code_point: u32,
    virtual_key: u8,
    dead_key: bool,
}

const _: () = assert!(size_of::<KeyState>() == 12);

/// Host input, or a fake in tests.
trait Source: Send {
    /// The pad in host slot `slot`, `None` when the slot is empty.
    fn pad(&mut self, slot: usize) -> Option<PadState>;
    /// Keys held down; none while the title's window lacks focus.
    fn keys(&mut self) -> Vec<KeyState>;
    /// Mouse buttons held down (`controls::MOUSE_BUTTONS`); none while the
    /// cursor is not captured.
    fn mouse_buttons(&mut self) -> Vec<u8> {
        Vec::new()
    }
    /// Motor speeds: low frequency (left), high frequency (right).
    fn rumble(&mut self, slot: usize, low: u16, high: u16);
}

fn fallback(keys: &[KeyState]) -> PadState {
    let on = |vk: u8| {
        if keys.iter().any(|k| k.virtual_key == vk) {
            1.0
        } else {
            0.0
        }
    };
    PadState {
        buttons: FALLBACK_BUTTONS
            .iter()
            .filter(|(vk, _)| on(*vk) != 0.0)
            .fold(0, |all, (_, bit)| all | bit),
        left_trigger: on(b'Z'),
        right_trigger: on(b'C'),
        left_x: on(b'D') - on(b'A'),
        left_y: on(b'W') - on(b'S'),
        right_x: on(b'L') - on(b'J'),
        right_y: on(b'I') - on(b'K'),
    }
}

#[repr(C)]
struct Table<const N: usize>([*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for Table<N> {}

/// The input object; one per hub.
#[repr(C)]
pub struct Object {
    vtable: &'static Table<22>,
    hub: *const Hub,
}

#[repr(C)]
pub struct Device {
    vtable: &'static Table<21>,
    hub: *const Hub,
    index: usize,
    info: DeviceInfo,
}

/// What a device says about itself. The title reads `id` (+0x20); the rest
/// follows the public layout and is zero where we have nothing to say.
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)] // read by title code
struct DeviceInfo {
    size: u32,
    vendor: u16,
    product: u16,
    revision: u16,
    interface: u8,
    collection: u8,
    usage: [u16; 2],
    versions: [u16; 8],
    id: [u8; 32],
    root_id: [u8; 32],
    family: i32,
    capabilities: u32,
    supported_input: u32,
    rumble_motors: u32,
    /// Counts and optional detail pointers.
    rest: [u64; 26],
}

const _: () = assert!(std::mem::offset_of!(DeviceInfo, id) == 0x20);

fn kind(index: usize) -> u32 {
    if index == KEYBOARD {
        KEYBOARD_KIND
    } else {
        GAMEPAD_KIND
    }
}

impl Device {
    fn new(index: usize) -> Device {
        let mut id = [0; 32];
        id[..7].copy_from_slice(b"victory");
        id[31] = index as u8 + 1;
        let info = DeviceInfo {
            size: size_of::<DeviceInfo>() as u32,
            id,
            root_id: id,
            supported_input: kind(index),
            // Low and high frequency.
            rumble_motors: if index < PADS { 0x3 } else { 0 },
            ..Default::default()
        };
        Device {
            vtable: &DEVICE,
            hub: std::ptr::null(),
            index,
            info,
        }
    }

    fn hub(&self) -> &'static Hub {
        // SAFETY: set by `Hub::new`; hubs are never freed.
        unsafe { &*self.hub }
    }
}

#[derive(PartialEq)]
enum Value {
    Pad(PadState),
    Keys(Vec<KeyState>),
}

#[repr(C)]
pub struct Reading {
    vtable: &'static Table<25>,
    refs: AtomicU32,
    device: *const Device,
    sequence: u64,
    timestamp: u64,
    value: Value,
}

impl Reading {
    /// The index of the device it is from.
    fn hub_index(&self) -> usize {
        // SAFETY: readings point at one of their hub's devices.
        unsafe { (*self.device).index }
    }

    fn kind(&self) -> u32 {
        match self.value {
            Value::Pad(_) => GAMEPAD_KIND,
            Value::Keys(_) => KEYBOARD_KIND,
        }
    }
}

/// A counted reference to a reading.
struct Ref(NonNull<Reading>);

impl Ref {
    fn new(reading: Reading) -> Ref {
        Ref(NonNull::from(Box::leak(Box::new(reading))))
    }

    /// Another reference, for the title.
    fn share(&self) -> *const Reading {
        reading_add_ref(self.0.as_ptr());
        self.0.as_ptr()
    }
}

impl Deref for Ref {
    type Target = Reading;
    fn deref(&self) -> &Reading {
        // SAFETY: we hold a reference, so it is alive.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for Ref {
    fn drop(&mut self) {
        reading_release(self.0.as_ptr());
    }
}

type DeviceCallback = extern "system" fn(u64, *mut c_void, *const Device, u64, u32, u32);

#[derive(Clone, Copy)]
struct Registration {
    token: u64,
    /// One device's address, or 0 for any.
    device: usize,
    kind: u32,
    status: u32,
    context: usize,
    callback: DeviceCallback,
}

impl Registration {
    fn matches(&self, devices: &[Device; DEVICES], index: usize) -> bool {
        kind(index) & self.kind != 0
            && (self.device == 0 || self.device == &devices[index] as *const Device as usize)
    }

    fn event(&self, device: usize, timestamp: u64, current: u32, previous: u32) -> Event {
        Event {
            registration: *self,
            device,
            timestamp,
            current,
            previous,
        }
    }
}

struct Event {
    registration: Registration,
    device: usize,
    timestamp: u64,
    current: u32,
    previous: u32,
}

#[derive(Default)]
struct Slot {
    connected: bool,
    sequence: u64,
    history: VecDeque<Ref>,
}

struct State {
    slots: [Slot; DEVICES],
    callbacks: Vec<Registration>,
    /// Asynchronous enumerations, delivered by the next poll.
    pending: Vec<Event>,
    /// Device callbacks running now: token and thread.
    running: Vec<(u64, ThreadId)>,
    /// The keyboard and mouse as a pad; `None`: the old stand-in pad.
    controls: Option<Controls>,
    /// The last poll's host pads and keys, for pad reads in between.
    host_pads: [Option<PadState>; PADS],
    keys: Vec<KeyState>,
    /// Scripted input as a pad (none when nothing is scripted now).
    script: Option<PadState>,
}

pub struct Hub {
    object: Object,
    devices: [Device; DEVICES],
    /// Apart from the state: XInput can be slow, and the title reads
    /// readings every frame.
    source: Mutex<Box<dyn Source>>,
    /// Mouse motion and wheel steps, added by the host's raw input thread.
    counts: Arc<Counts>,
    state: Mutex<State>,
    /// A device callback returned.
    returned: Condvar,
}

fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// SAFETY: the back pointers are set before the hub is shared and never
// change; what changes sits behind the mutex or is atomic.
unsafe impl Send for Hub {}
// SAFETY: as above.
unsafe impl Sync for Hub {}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Microseconds, the clock of every timestamp we hand out.
fn now() -> u64 {
    START.elapsed().as_micros() as u64
}

impl Hub {
    fn new(
        source: Box<dyn Source>,
        counts: Arc<Counts>,
        settings: super::controls::Settings,
    ) -> &'static Hub {
        cursor::ENABLED.store(settings.enabled && settings.capture, Ordering::Relaxed);
        let hub = Box::into_raw(Box::new(Hub {
            object: Object {
                vtable: &INPUT,
                hub: std::ptr::null(),
            },
            devices: std::array::from_fn(Device::new),
            source: Mutex::new(source),
            counts,
            state: Mutex::new(State {
                slots: Default::default(),
                callbacks: Vec::new(),
                pending: Vec::new(),
                running: Vec::new(),
                controls: settings.enabled.then(|| Controls::new(settings)),
                host_pads: [None; PADS],
                keys: Vec::new(),
                script: None,
            }),
            returned: Condvar::new(),
        }));
        // SAFETY: just allocated and never freed; nothing else sees it yet.
        unsafe {
            (*hub).object.hub = hub;
            for device in &mut (*hub).devices {
                device.hub = hub;
            }
            &*hub
        }
    }

    /// A hub over the host's input, polled from now on.
    fn start(
        source: Box<dyn Source>,
        counts: Arc<Counts>,
        settings: super::controls::Settings,
    ) -> &'static Hub {
        let hub = Hub::new(source, counts, settings);
        hub.poll();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(POLL);
                hub.poll();
            }
        });
        hub
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Reads the host once; device callbacks run on the calling thread.
    fn poll(&self) {
        let events = self.sample();
        self.deliver(events);
    }

    /// Reads the host once on the title's thread, just before it reads
    /// (`VICTORY_PACE_INPUT`). No callback runs here: connection events wait
    /// for the polling thread to deliver them.
    fn poll_late(&self) {
        let events = self.sample();
        if !events.is_empty() {
            let mut state = self.state();
            let mut events = events;
            events.append(&mut state.pending);
            state.pending = events;
        }
    }

    /// Reads the host and records it; the callbacks due.
    fn sample(&self) -> Vec<Event> {
        let at = victory_core::pace::now_ns();
        let (pads, keys, buttons) = {
            let mut source = lock(&self.source);
            let pads: [_; PADS] = std::array::from_fn(|slot| source.pad(slot));
            (pads, source.keys(), source.mouse_buttons())
        };
        victory_core::pace::input_sampled(at);
        {
            let mut state = self.state();
            let events = state.poll(&self.devices, pads, keys, &buttons, &self.counts);
            if let Some(c) = &mut state.controls
                && std::mem::take(&mut c.release_cursor)
            {
                cursor::release("Alt key");
            }
            events
        }
    }

    /// Runs callbacks, never under our lock: they call back into us. One
    /// unregistered since the event was taken does not run, and
    /// `UnregisterCallback` waits for one that runs.
    fn deliver(&self, events: Vec<Event>) {
        let me = std::thread::current().id();
        for e in events {
            let r = e.registration;
            {
                let mut state = self.state();
                if !state.callbacks.iter().any(|c| c.token == r.token) {
                    continue;
                }
                state.running.push((r.token, me));
            }
            (r.callback)(
                r.token,
                r.context as *mut c_void,
                &self.devices[e.device],
                e.timestamp,
                e.current,
                e.previous,
            );
            let mut state = self.state();
            if let Some(i) = state.running.iter().position(|&x| x == (r.token, me)) {
                state.running.swap_remove(i);
            }
            self.returned.notify_all();
        }
    }

    fn index(&self, device: *const Device) -> Option<usize> {
        self.devices.iter().position(|d| std::ptr::eq(d, device))
    }
}

impl State {
    fn poll(
        &mut self,
        devices: &[Device; DEVICES],
        pads: [Option<PadState>; PADS],
        keys: Vec<KeyState>,
        mouse_buttons: &[u8],
        counts: &Counts,
    ) -> Vec<Event> {
        let now = now();
        let mut events = std::mem::take(&mut self.pending);
        // Scripted input (auto input, press file, fast boot) is a third
        // source beside the host pads and the keyboard and mouse: it never
        // goes through the key layout and never replaces another source.
        let mut script = Vec::new();
        auto_input_inject(&mut script);
        let script_pad = (!script.is_empty()).then(|| fallback(&script));
        if let Some(c) = &mut self.controls {
            let held: Vec<u8> = keys
                .iter()
                .map(|k| k.virtual_key)
                .chain(mouse_buttons.iter().copied())
                .collect();
            c.feed(&held, counts);
            c.set_script(script_pad.unwrap_or_default());
        }
        self.script = script_pad;
        self.host_pads = pads;
        self.keys.clone_from(&keys);
        self.publish(devices, now, None, &mut events);
        let mut keys = keys;
        keys.extend(script);
        self.update(devices, KEYBOARD, Some(Value::Keys(keys)), now, &mut events);
        events
    }

    /// The pad devices' readings: the host pads, the keyboard and mouse on
    /// the first connected one or else on the stand-in pad. `take`: a title
    /// read, which drains the mouse counts.
    fn publish(
        &mut self,
        devices: &[Device; DEVICES],
        now: u64,
        take: Option<&Counts>,
        events: &mut Vec<Event>,
    ) {
        let pads = self.host_pads;
        let first = pads.iter().position(Option::is_some);
        let mut values = [None; PADS + 1];
        values[..PADS].copy_from_slice(&pads);
        let target = first.unwrap_or(FALLBACK);
        match &mut self.controls {
            Some(c) => {
                let base = first.and_then(|i| pads[i]);
                values[target] = Some(match take {
                    Some(counts) => c.take(now, base, counts),
                    None => c.peek(base),
                });
            }
            // Keyboard and mouse off: the stand-in pad when no pad is
            // connected; scripted input joins whichever pad there is.
            None => {
                let base = match first {
                    Some(i) => pads[i],
                    None => Some(fallback(&self.keys)),
                };
                values[target] = match (base, self.script) {
                    (Some(pad), Some(script)) => Some(pad.merged(script)),
                    (Some(pad), None) => Some(pad),
                    (None, script) => script,
                };
            }
        }
        for (index, value) in values.into_iter().enumerate() {
            self.update(devices, index, value.map(Value::Pad), now, events);
        }
    }

    /// A title read of a pad: with keyboard and mouse, a new reading with the
    /// mouse motion since the last read.
    fn title_read(&mut self, devices: &[Device; DEVICES], counts: &Counts) {
        if self.controls.is_none() {
            return;
        }
        let mut events = Vec::new();
        self.publish(devices, now(), Some(counts), &mut events);
        // Connections only change on host polls; keep anything for the next.
        self.pending.append(&mut events);
    }

    /// Records a device's state: a reading when it changed, callbacks when
    /// the device came or went.
    fn update(
        &mut self,
        devices: &[Device; DEVICES],
        index: usize,
        value: Option<Value>,
        now: u64,
        events: &mut Vec<Event>,
    ) {
        let slot = &mut self.slots[index];
        let was = slot.connected;
        slot.connected = value.is_some();
        if let Some(value) = value
            && (!was || slot.history.back().is_none_or(|r| r.value != value))
        {
            slot.sequence += 1;
            if slot.history.len() == HISTORY {
                slot.history.pop_front();
            }
            slot.history.push_back(Ref::new(Reading {
                vtable: &READING,
                refs: AtomicU32::new(1),
                device: &devices[index],
                sequence: slot.sequence,
                timestamp: now,
                value,
            }));
        }
        if was != slot.connected {
            tracing::info!(device = index, connected = !was, "input device");
            let (current, previous) = if was {
                (0, STATUS_CONNECTED)
            } else {
                (STATUS_CONNECTED, 0)
            };
            events.extend(
                self.callbacks
                    .iter()
                    .filter(|r| r.matches(devices, index) && r.status & STATUS_CONNECTED != 0)
                    .map(|r| r.event(index, now, current, previous)),
            );
        }
    }
}

/// VICTORY_PRESS_FILE=<path>: buttons written to that file ("A", "DOWN DOWN A") are held one
/// after another for 150 ms each, then the file is emptied.
fn live_press(keys: &mut Vec<KeyState>) {
    use std::sync::{Mutex, OnceLock};
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    static Q: Mutex<(Option<Instant>, Vec<u8>, Option<Instant>)> = Mutex::new((None, Vec::new(), None));
    let Some(path) = PATH.get_or_init(|| std::env::var("VICTORY_PRESS_FILE").ok()) else { return };
    let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if q.1.is_empty() && q.0.is_none_or(|t| now - t >= Duration::from_millis(50)) {
        q.0 = Some(now);
        if let Ok(s) = std::fs::read_to_string(path) {
            if !s.trim().is_empty() {
                let _ = std::fs::write(path, "");
                q.1 = s
                    .split_whitespace()
                    .rev()
                    .filter_map(|b| match b.to_ascii_uppercase().as_str() {
                        "A" => Some(0x0D),
                        "B" => Some(0x08),
                        "UP" => Some(0x26),
                        "DOWN" => Some(0x28),
                        "LEFT" => Some(0x25),
                        "RIGHT" => Some(0x27),
                        "MENU" | "START" => Some(0x1B),
                        "VIEW" => Some(0x09),
                        "LS_UP" => Some(b'W'),
                        "LS_DOWN" => Some(b'S'),
                        "LS_LEFT" => Some(b'A'),
                        "LS_RIGHT" => Some(b'D'),
                        "RS_UP" => Some(b'I'),
                        "RS_DOWN" => Some(b'K'),
                        "RS_LEFT" => Some(b'J'),
                        "RS_RIGHT" => Some(b'L'),
                        "X" => Some(b'X'),
                        "Y" => Some(b'Y'),
                        _ => None,
                    })
                    .collect();
                q.2 = Some(now);
            }
        }
    }
    let Some(&vk) = q.1.last() else { return };
    let held = q.2.map_or(Duration::ZERO, |t| now - t);
    // Stick tokens (LS_*, RS_*) are held 1.5 s, buttons 150 ms.
    let hold = if b"WASDIJKL".contains(&vk) { 1500 } else { 150 };
    if held < Duration::from_millis(hold) {
        keys.push(KeyState { scan_code: 0, code_point: 0, virtual_key: vk, dead_key: false });
    } else if held >= Duration::from_millis(hold + 150) {
        q.1.pop();
        q.2 = Some(now);
    }
}

/// VICTORY_FAST_BOOT (testing only): A on every other presented frame, and
/// START on one frame in eight (skips cutscenes), from the first frame until
/// the title is in the world; then nothing, for good.
fn fast_boot_keys(keys: &mut Vec<KeyState>) {
    let key = |scan: u16, vk: u8| KeyState {
        scan_code: scan.into(),
        code_point: u32::from(vk),
        virtual_key: vk,
        dead_key: false,
    };
    // Just in the world: A (Enter) on every other reading, so the intro
    // cutscene's skip prompt sees presses (frames take seconds there).
    if victory_core::fastboot::skipping() {
        static READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 2 == 0 {
            keys.push(key(0x1C, 0x0D));
        }
        return;
    }
    if !victory_core::fastboot::active() {
        return;
    }
    let frame = victory_core::fastboot::frame();
    if frame % 2 == 0 {
        keys.push(key(0x1C, 0x0D));
    } else if frame % 8 == 1 {
        keys.push(key(0x01, 0x1B));
    }
}

/// VICTORY_AUTO_INPUT: inject A-button presses every ~2 s for the first
/// 120 s, so menus advance without manual interaction.
fn auto_input_inject(keys: &mut Vec<KeyState>) {
    live_press(keys);
    if victory_core::fastboot::enabled() {
        fast_boot_keys(keys);
        return;
    }
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static START: OnceLock<Instant> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("VICTORY_AUTO_INPUT").is_some()) {
        return;
    }
    let start = *START.get_or_init(Instant::now);
    let elapsed = start.elapsed();
    if elapsed > Duration::from_secs(120) {
        return;
    }
    let cycle = elapsed.as_millis() % 2000;
    if cycle < 100 {
        keys.push(KeyState {
            scan_code: 0x1C,
            code_point: 0x0D,
            virtual_key: 0x0D,
            dead_key: false,
        });
    }
}

#[cfg(windows)]
fn host(counts: &Arc<Counts>, settings: &super::controls::Settings) -> Box<dyn Source> {
    if settings.enabled {
        host::raw_mouse(counts.clone());
    }
    Box::new(host::Host::new(counts.clone()))
}

#[cfg(not(windows))]
fn host(_: &Arc<Counts>, _: &super::controls::Settings) -> Box<dyn Source> {
    struct Nothing;
    impl Source for Nothing {
        fn pad(&mut self, _: usize) -> Option<PadState> {
            None
        }
        fn keys(&mut self) -> Vec<KeyState> {
            Vec::new()
        }
        fn rumble(&mut self, _: usize, _: u16, _: u16) {}
    }
    Box::new(Nothing)
}

static HUB: LazyLock<&'static Hub> = LazyLock::new(|| {
    let settings = super::controls::settings();
    let counts = Arc::new(Counts::default());
    Hub::start(host(&counts, &settings), counts, settings)
});

/// The mouse cursor while the title window has focus: captured (hidden,
/// held inside the window, its motion turns the camera) or free. The host
/// poll and the title window's procedure (`lifecycle`) keep it. Every
/// release is logged with its reason (`VICTORY_LOG=info`).
pub(crate) mod cursor {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Capture is wanted at all (`[controls]` on, `mouse_capture`).
    pub(crate) static ENABLED: AtomicBool = AtomicBool::new(false);
    pub(crate) static CAPTURED: AtomicBool = AtomicBool::new(false);
    /// The title window has focus (kept by the host poll).
    pub(crate) static FOCUSED: AtomicBool = AtomicBool::new(false);
    /// The title's own top-level window (the last one clicked or focused):
    /// what the cursor is clipped to, never a splash or overlay window of
    /// ours that happens to be in front.
    pub(crate) static WINDOW: AtomicUsize = AtomicUsize::new(0);

    pub(crate) fn captured() -> bool {
        CAPTURED.load(Ordering::Relaxed)
    }

    pub(crate) fn capture(why: &str) {
        if ENABLED.load(Ordering::Relaxed) && !CAPTURED.swap(true, Ordering::Relaxed) {
            tracing::info!(why, "mouse cursor captured");
        }
    }

    pub(crate) fn release(why: &str) {
        if CAPTURED.swap(false, Ordering::Relaxed) {
            tracing::info!(why, "mouse cursor released");
        }
    }

    /// A click in the title window captures. Buttons held while it is
    /// captured are no shots until they are let go (`host`).
    pub(crate) fn click() {
        capture("click");
    }

    /// Whether `window` (a handle; 0 is none) belongs to this process.
    #[cfg(windows)]
    pub(crate) fn ours(window: usize) -> bool {
        use windows_sys::Win32::System::Threading::GetCurrentProcessId;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
        let mut process = 0;
        // SAFETY: a plain query; a stale handle gives process 0.
        unsafe { GetWindowThreadProcessId(window as _, &mut process) };
        process == unsafe { GetCurrentProcessId() }
    }

    /// A window of the title gained the focus or a click: it is the one to
    /// hold the cursor in (top-level windows only).
    #[cfg(windows)]
    pub(crate) fn note_window(window: usize) {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetParent;
        // SAFETY: a plain query.
        if window != 0 && unsafe { GetParent(window as _) }.is_null() {
            WINDOW.store(window, Ordering::Relaxed);
        }
    }

    /// The window focus moved from the title window to `to`: a window of
    /// ours (the splash, an overlay, the title's own dialogs) or none (the
    /// system is between two windows) keeps the capture, another process's
    /// window frees it.
    #[cfg(windows)]
    pub(crate) fn focus_moved(to: usize, event: &str) {
        if to != 0 && !ours(to) {
            release(&format!("{event}: focus went to another program's window {to:#x}"));
        } else {
            tracing::debug!(to, event, "focus moved inside the program; the mouse stays captured");
        }
    }

    /// Holds the cursor inside `window`'s client area; false when it has no
    /// area (minimized).
    #[cfg(windows)]
    pub(crate) fn clip(window: usize) -> bool {
        use windows_sys::Win32::Foundation::{POINT, RECT};
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
        use windows_sys::Win32::UI::WindowsAndMessaging::{ClipCursor, GetClientRect};
        // SAFETY: plain queries on a window handle; a stale one just fails.
        unsafe {
            let hwnd = window as _;
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) == 0 {
                return false;
            }
            let mut a = POINT { x: client.left, y: client.top };
            let mut b = POINT { x: client.right, y: client.bottom };
            ClientToScreen(hwnd, &mut a);
            ClientToScreen(hwnd, &mut b);
            if b.x > a.x && b.y > a.y {
                let rect = RECT { left: a.x, top: a.y, right: b.x, bottom: b.y };
                ClipCursor(&rect);
                return true;
            }
        }
        false
    }

    /// The window the cursor is held in: the title's, else the foreground's.
    #[cfg(windows)]
    pub(crate) fn target() -> usize {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindow};
        let window = WINDOW.load(Ordering::Relaxed);
        // SAFETY: plain queries.
        if window != 0 && unsafe { IsWindow(window as _) } != 0 {
            window
        } else {
            unsafe { GetForegroundWindow() as usize }
        }
    }
}
type This = *const Object;
type Out = *mut *const c_void;

/// The DLL's only export (ordinal 1): the input object, static and shared.
pub extern "system" fn GameInputCreate(out: *mut This) -> HRESULT {
    tracing::info!("GameInputCreate");
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: checked non-null; the title passes room for a pointer.
    unsafe { out.write(&HUB.object) };
    S_OK
}

fn hub(this: This) -> &'static Hub {
    // SAFETY: `this` is a hub's object; hubs are never freed.
    unsafe { &*(*this).hub }
}

/// Clears a COM out pointer and returns `hr`.
fn fail(out: Out, hr: HRESULT) -> HRESULT {
    if !out.is_null() {
        // SAFETY: checked non-null; the title passes room for a pointer.
        unsafe { out.write(std::ptr::null()) };
    }
    hr
}

/// Gives the title a reference to `reading`, or fails as with none.
fn hand(out: Out, reading: Option<&Ref>) -> HRESULT {
    match reading {
        _ if out.is_null() => E_INVALIDARG,
        Some(reading) => {
            // SAFETY: checked non-null; the title passes room for a pointer.
            unsafe { out.write(reading.share().cast()) };
            S_OK
        }
        None => fail(out, READING_NOT_FOUND),
    }
}

extern "system" fn no_interface(_: *const c_void, _iid: *const c_void, out: Out) -> HRESULT {
    tracing::warn!("input QueryInterface not implemented");
    fail(out, E_NOINTERFACE)
}
/// The input object and devices live as long as the process.
extern "system" fn add_ref(_: *const c_void) -> u32 {
    1
}

extern "system" fn timestamp(_: This) -> u64 {
    now()
}

extern "system" fn current_reading(
    this: This,
    kind: u32,
    device: *const Device,
    out: Out,
) -> HRESULT {
    let hub = hub(this);
    if victory_core::pace::switches().input
        && victory_core::pace::now_ns().saturating_sub(victory_core::pace::last_sampled()) >= LATE_SAMPLE_NS
    {
        hub.poll_late();
    }
    let mut state = hub.state();
    if kind & GAMEPAD_KIND != 0 {
        state.title_read(&hub.devices, &hub.counts);
    }
    let state = state;
    let newest = |i: usize| {
        state.slots[i]
            .history
            .back()
            .filter(|r| r.kind() & kind != 0)
    };
    let found = if device.is_null() {
        (0..DEVICES)
            .filter(|&i| state.slots[i].connected)
            .filter_map(newest)
            .max_by_key(|r| r.timestamp)
    } else {
        match hub.index(device) {
            None => return fail(out, E_INVALIDARG),
            Some(i) if !state.slots[i].connected => return fail(out, DEVICE_DISCONNECTED),
            Some(i) => newest(i),
        }
    };
    tracing::trace!(
        kind = format_args!("{kind:#x}"),
        device = ?hub.index(device),
        sequence = found.map(|r| r.sequence),
        "current reading"
    );
    if found.is_some() {
        victory_core::pace::input_read();
    }
    hand(out, found)
}

/// The reading after (`forward`) or before `reference` on its device.
fn relative(
    this: This,
    reference: *const Reading,
    kind: u32,
    device: *const Device,
    out: Out,
    forward: bool,
) -> HRESULT {
    if reference.is_null() {
        return fail(out, E_INVALIDARG);
    }
    let hub = hub(this);
    // SAFETY: a reading we handed out and the title still holds.
    let reference = unsafe { &*reference };
    // shortcut: with no device given only the reference's device is walked,
    // not every device's readings in time order; the title walks
    // only keyboard readings (one device) and the current pad's previous one.
    let Some(index) = hub.index(reference.device) else {
        return fail(out, E_INVALIDARG);
    };
    if !device.is_null() && !std::ptr::eq(device, reference.device) {
        return fail(out, E_INVALIDARG);
    }
    let state = hub.state();
    let history = &state.slots[index].history;
    if history
        .front()
        .is_none_or(|oldest| reference.sequence < oldest.sequence)
    {
        return fail(out, REFERENCE_TOO_OLD);
    }
    let wanted = |r: &&Ref| r.kind() & kind != 0;
    let found = if forward {
        history
            .iter()
            .filter(|r| r.sequence > reference.sequence)
            .find(wanted)
    } else {
        history
            .iter()
            .rev()
            .filter(|r| r.sequence < reference.sequence)
            .find(wanted)
    };
    hand(out, found)
}
extern "system" fn next_reading(
    this: This,
    reference: *const Reading,
    kind: u32,
    device: *const Device,
    out: Out,
) -> HRESULT {
    relative(this, reference, kind, device, out, true)
}
extern "system" fn previous_reading(
    this: This,
    reference: *const Reading,
    kind: u32,
    device: *const Device,
    out: Out,
) -> HRESULT {
    relative(this, reference, kind, device, out, false)
}
/// The device's reading at `time`, among those still kept.
extern "system" fn temporal_reading(
    this: This,
    time: u64,
    device: *const Device,
    out: Out,
) -> HRESULT {
    let hub = hub(this);
    let Some(index) = hub.index(device) else {
        return fail(out, E_INVALIDARG);
    };
    let state = hub.state();
    let history = &state.slots[index].history;
    hand(out, history.iter().rev().find(|r| r.timestamp <= time))
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn token(out: *mut u64) -> HRESULT {
    if !out.is_null() {
        // SAFETY: checked non-null.
        unsafe { out.write(NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)) };
    }
    S_OK
}

// shortcut: reading callbacks never run; the title registers none
// (it polls).
extern "system" fn register_reading(
    _: This,
    _device: *const c_void,
    _kind: u32,
    _threshold: f32,
    _context: *mut c_void,
    _callback: *const c_void,
    out: *mut u64,
) -> HRESULT {
    tracing::warn!("input reading callbacks are never called");
    token(out)
}

/// Device callbacks run on our poll thread when a device comes or goes.
/// Enumeration reports the devices already there: blocking on the caller's
/// thread before we return (the title registers holding a lock its callback
/// takes again), asynchronous on the next poll.
extern "system" fn register_device(
    this: This,
    device: *const Device,
    kind: u32,
    status: u32,
    enumeration: u32,
    context: *mut c_void,
    callback: Option<DeviceCallback>,
    out: *mut u64,
) -> HRESULT {
    let Some(callback) = callback else {
        return E_INVALIDARG;
    };
    let hub = hub(this);
    let registration = Registration {
        token: NEXT_TOKEN.fetch_add(1, Ordering::Relaxed),
        device: device as usize,
        kind,
        status,
        context: context as usize,
        callback,
    };
    let mut found = Vec::new();
    {
        let mut state = hub.state();
        if enumeration != 0 && status & STATUS_CONNECTED != 0 {
            let now = now();
            found = (0..DEVICES)
                .filter(|&i| state.slots[i].connected && registration.matches(&hub.devices, i))
                .map(|i| registration.event(i, now, STATUS_CONNECTED, 0))
                .collect();
        }
        tracing::info!(
            kind = format_args!("{kind:#x}"),
            enumeration,
            devices = found.len(),
            "input device callback"
        );
        state.callbacks.push(registration);
        if enumeration == ASYNC_ENUMERATION {
            state.pending.append(&mut found);
        }
    }
    if !out.is_null() {
        // SAFETY: checked non-null.
        unsafe { out.write(registration.token) };
    }
    hub.deliver(found);
    S_OK
}
/// Guide button and keyboard layout callbacks: neither ever changes here.
extern "system" fn register_other(
    _: This,
    _device: *const c_void,
    _context: *mut c_void,
    _callback: *const c_void,
    out: *mut u64,
) -> HRESULT {
    token(out)
}

fn forget(this: This, token: u64) {
    let mut state = hub(this).state();
    state.callbacks.retain(|r| r.token != token);
    state.pending.retain(|e| e.registration.token != token);
}
extern "system" fn stop_callback(this: This, token: u64) {
    forget(this, token);
}
/// Stops the callback, then waits up to `timeout` µs for a call of it that
/// is running; false on time-out, or from inside that callback (it cannot
/// wait for itself), as documented. Either way it is never called again.
extern "system" fn unregister_callback(this: This, token: u64, timeout: u64) -> bool {
    forget(this, token);
    let hub = hub(this);
    let me = std::thread::current().id();
    let deadline = Instant::now().checked_add(Duration::from_micros(timeout));
    let mut state = hub.state();
    loop {
        let running = state.running.iter().filter(|r| r.0 == token);
        if running.clone().any(|r| r.1 == me) {
            return false;
        }
        if running.count() == 0 {
            return true;
        }
        state = match deadline {
            None => hub.returned.wait(state).unwrap_or_else(PoisonError::into_inner),
            Some(deadline) => {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return false;
                };
                let waited = hub.returned.wait_timeout(state, left);
                waited.unwrap_or_else(PoisonError::into_inner).0
            }
        };
    }
}

/// Dispatcher and aggregate device: no title of ours asks for them.
extern "system" fn create_dispatcher(_: This, out: Out) -> HRESULT {
    tracing::warn!("input dispatcher not implemented");
    fail(out, E_NOTIMPL)
}
extern "system" fn create_aggregate(_: This, _kind: u32, out: Out) -> HRESULT {
    tracing::warn!("aggregate input device not implemented");
    fail(out, E_NOTIMPL)
}
extern "system" fn find_by_id(this: This, id: *const [u8; 32], out: Out) -> HRESULT {
    if id.is_null() || out.is_null() {
        return fail(out, E_INVALIDARG);
    }
    // SAFETY: checked non-null; the title passes a device id.
    let id = unsafe { &*id };
    match hub(this).devices.iter().find(|d| d.info.id == *id) {
        Some(device) => {
            // SAFETY: checked non-null.
            unsafe { out.write((device as *const Device).cast()) };
            S_OK
        }
        None => fail(out, DEVICE_NOT_FOUND),
    }
}
/// Lookup by object, platform handle or platform string: our devices have
/// none of these.
extern "system" fn find_device(_: This, _value: *const c_void, out: Out) -> HRESULT {
    fail(out, DEVICE_NOT_FOUND)
}
extern "system" fn enable_oem_device(
    _: This,
    _vendor: u16,
    _product: u16,
    _interface: u8,
    _collection: u8,
) -> HRESULT {
    S_OK
}
extern "system" fn set_focus_policy(_: This, _policy: u32) {}

static INPUT: Table<22> = Table([
    no_interface as *const (),        // 0x00
    add_ref as *const (),             // 0x08 AddRef
    add_ref as *const (),             // 0x10 Release
    timestamp as *const (),           // 0x18
    current_reading as *const (),     // 0x20
    next_reading as *const (),        // 0x28
    previous_reading as *const (),    // 0x30
    temporal_reading as *const (),    // 0x38
    register_reading as *const (),    // 0x40
    register_device as *const (),     // 0x48
    register_other as *const (),      // 0x50 guide button
    register_other as *const (),      // 0x58 keyboard layout
    stop_callback as *const (),       // 0x60
    unregister_callback as *const (), // 0x68
    create_dispatcher as *const (),   // 0x70
    create_aggregate as *const (),    // 0x78
    find_by_id as *const (),          // 0x80
    find_device as *const (),         // 0x88 by object
    find_device as *const (),         // 0x90 by platform handle
    find_device as *const (),         // 0x98 by platform string
    enable_oem_device as *const (),   // 0xa0
    set_focus_policy as *const (),    // 0xa8
]);

extern "system" fn device_info(this: *const Device) -> *const DeviceInfo {
    // SAFETY: one of our devices.
    unsafe { &(*this).info }
}
extern "system" fn device_status(this: *const Device) -> u32 {
    // SAFETY: one of our devices.
    let device = unsafe { &*this };
    if device.hub().state().slots[device.index].connected {
        STATUS_CONNECTED
    } else {
        0
    }
}
// shortcut: trigger motors are dropped (XInput has none). The host API that
// drives them without a redistributable is Windows.Gaming.Input (WinRT
// `Gamepad.Vibration`), which needs WinRT bindings this crate lacks outside
// diagnostics builds; the title sends trigger levels in slot 0x50.
extern "system" fn set_rumble(this: *const Device, params: *const [f32; 4]) {
    // SAFETY: one of our devices.
    let device = unsafe { &*this };
    if device.index >= PADS {
        return;
    }
    let [low, high, ..] = if params.is_null() {
        [0.0; 4]
    } else {
        // SAFETY: checked non-null; four motor levels.
        unsafe { *params }
    };
    let speed = |level: f32| (level.clamp(0.0, 1.0) * f32::from(u16::MAX)) as u16;
    lock(&device.hub().source).rumble(device.index, speed(low), speed(high));
}
extern "system" fn unsupported<const SLOT: usize>(_: *const c_void) -> HRESULT {
    tracing::warn!(
        slot = format_args!("{SLOT:#x}"),
        "input device method not implemented"
    );
    E_NOTIMPL
}

static DEVICE: Table<21> = Table([
    no_interface as *const (),        // 0x00
    add_ref as *const (),             // 0x08 AddRef
    add_ref as *const (),             // 0x10 Release
    device_info as *const (),         // 0x18
    device_status as *const (),       // 0x20
    unsupported::<0x28> as *const (), // battery
    unsupported::<0x30> as *const (), // force feedback
    unsupported::<0x38> as *const (), //
    unsupported::<0x40> as *const (), //
    unsupported::<0x48> as *const (), // haptics
    set_rumble as *const (),          // 0x50
    unsupported::<0x58> as *const (), // input synchronization
    unsupported::<0x60> as *const (), //
    unsupported::<0x68> as *const (), // power off
    unsupported::<0x70> as *const (), // raw device access
    unsupported::<0x78> as *const (), //
    unsupported::<0x80> as *const (), //
    unsupported::<0x88> as *const (), //
    unsupported::<0x90> as *const (), //
    unsupported::<0x98> as *const (), //
    unsupported::<0xa0> as *const (), //
]);

fn reading<'a>(this: *const Reading) -> &'a Reading {
    // SAFETY: a reading we handed out and the title still holds.
    unsafe { &*this }
}
extern "system" fn reading_add_ref(this: *const Reading) -> u32 {
    reading(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn reading_release(this: *const Reading) -> u32 {
    let left = reading(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the last reference is gone; `Ref::new` boxed it.
        drop(unsafe { Box::from_raw(this.cast_mut()) });
    }
    left
}
extern "system" fn input_kind(this: *const Reading) -> u32 {
    reading(this).kind()
}
extern "system" fn sequence_number(this: *const Reading, kind: u32) -> u64 {
    let reading = reading(this);
    if reading.kind() & kind != 0 {
        reading.sequence
    } else {
        0
    }
}
extern "system" fn reading_timestamp(this: *const Reading) -> u64 {
    reading(this).timestamp
}
extern "system" fn reading_device(this: *const Reading, out: *mut *const Device) {
    if !out.is_null() {
        // SAFETY: checked non-null; devices need no reference.
        unsafe { out.write(reading(this).device) };
    }
}
extern "system" fn key_count(this: *const Reading) -> u32 {
    match &reading(this).value {
        Value::Keys(keys) => keys.len() as u32,
        Value::Pad(_) => 0,
    }
}
extern "system" fn key_state(this: *const Reading, count: u32, out: *mut KeyState) -> u32 {
    let Value::Keys(keys) = &reading(this).value else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    let n = keys.len().min(count as usize);
    // SAFETY: the title passes room for `count` keys.
    unsafe { std::ptr::copy_nonoverlapping(keys.as_ptr(), out, n) };
    n as u32
}
extern "system" fn gamepad_state(this: *const Reading, out: *mut PadState) -> bool {
    match &reading(this).value {
        Value::Pad(state) if !out.is_null() => {
            log_title_read(reading(this), state);
            // SAFETY: checked non-null.
            unsafe { out.write(*state) };
            true
        }
        _ => false,
    }
}
/// What the title reads, when it differs from what it read before:
/// `VICTORY_LOG=victory_hle::modules::gameinput=debug`.
fn log_title_read(reading: &Reading, state: &PadState) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    static LAST: Mutex<Option<PadState>> = Mutex::new(None);
    let mut last = lock(&LAST);
    if *last != Some(*state) {
        *last = Some(*state);
        tracing::debug!(
            device = reading.hub_index(),
            sequence = reading.sequence,
            buttons = format_args!("{:#06x}", state.buttons),
            lt = state.left_trigger,
            rt = state.right_trigger,
            left = format_args!("{:+.3} {:+.3}", state.left_x, state.left_y),
            right = format_args!("{:+.3} {:+.3}", state.right_x, state.right_y),
            "title read pad"
        );
    }
}

/// Kinds our devices do not have: no count, no state.
extern "system" fn nothing(_: *const c_void) -> usize {
    0
}

static READING: Table<25> = Table([
    no_interface as *const (),      // 0x00
    reading_add_ref as *const (),   // 0x08
    reading_release as *const (),   // 0x10
    input_kind as *const (),        // 0x18
    sequence_number as *const (),   // 0x20
    reading_timestamp as *const (), // 0x28
    reading_device as *const (),    // 0x30
    nothing as *const (),           // 0x38 raw report
    nothing as *const (),           // 0x40 controller axes, buttons, switches
    nothing as *const (),           // 0x48
    nothing as *const (),           // 0x50
    nothing as *const (),           // 0x58
    nothing as *const (),           // 0x60
    nothing as *const (),           // 0x68
    key_count as *const (),         // 0x70
    key_state as *const (),         // 0x78
    nothing as *const (),           // 0x80 mouse
    nothing as *const (),           // 0x88 touch
    nothing as *const (),           // 0x90
    nothing as *const (),           // 0x98 motion
    nothing as *const (),           // 0xa0 arcade stick
    nothing as *const (),           // 0xa8 flight stick
    gamepad_state as *const (),     // 0xb0
    nothing as *const (),           // 0xb8 racing wheel
    nothing as *const (),           // 0xc0 UI navigation
]);

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "gameinput",
        dlls: &["gameinput.dll"],
        // Titles import it by ordinal only.
        exports: vec![HleExport {
            name: "#1",
            address: GameInputCreate as *const () as usize,
        }],
    }
}

/// Host input: XInput pads, the keyboard while our process has focus.
#[cfg(windows)]
mod host {
    use std::time::{Duration, Instant};

    use std::sync::atomic::Ordering;

    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MAPVK_VK_TO_VSC_EX, MapVirtualKeyW,
    };
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
        RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
    };
    use windows_sys::Win32::UI::Input::XboxController::{
        XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK,
        XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT,
        XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_LEFT_THUMB,
        XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_RIGHT_THUMB, XINPUT_GAMEPAD_START,
        XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y, XINPUT_STATE, XINPUT_VIBRATION, XInputGetState,
        XInputSetState,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ClipCursor, CreateWindowExW, DefWindowProcW, DispatchMessageW,
        GetForegroundWindow, GetMessageW, GetWindowTextW, HWND_MESSAGE,
        MSG, RegisterClassW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_INPUT, WM_SETTEXT,
        WNDCLASSW,
    };

    use crate::modules::controls::MOUSE_BUTTONS;
    use super::*;

    /// How long no foreground window at all is not yet a loss of focus.
    const NO_FOREGROUND: Duration = Duration::from_millis(1500);

    /// Asking XInput about an empty slot is slow; look again this often.
    const EMPTY_SLOT_RECHECK: Duration = Duration::from_secs(1);

    const BUTTONS: [(u16, u32); 14] = [
        (XINPUT_GAMEPAD_START, MENU),
        (XINPUT_GAMEPAD_BACK, VIEW),
        (XINPUT_GAMEPAD_A, A),
        (XINPUT_GAMEPAD_B, B),
        (XINPUT_GAMEPAD_X, X),
        (XINPUT_GAMEPAD_Y, Y),
        (XINPUT_GAMEPAD_DPAD_UP, UP),
        (XINPUT_GAMEPAD_DPAD_DOWN, DOWN),
        (XINPUT_GAMEPAD_DPAD_LEFT, LEFT),
        (XINPUT_GAMEPAD_DPAD_RIGHT, RIGHT),
        (XINPUT_GAMEPAD_LEFT_SHOULDER, LEFT_SHOULDER),
        (XINPUT_GAMEPAD_RIGHT_SHOULDER, RIGHT_SHOULDER),
        (XINPUT_GAMEPAD_LEFT_THUMB, LEFT_STICK),
        (XINPUT_GAMEPAD_RIGHT_THUMB, RIGHT_STICK),
    ];

    /// Raw values, no dead zone: titles apply their own.
    pub(super) fn convert(pad: &XINPUT_GAMEPAD) -> PadState {
        let stick = |v: i16| (f32::from(v) / 32767.0).max(-1.0);
        PadState {
            buttons: BUTTONS
                .iter()
                .filter(|(host, _)| pad.wButtons & host != 0)
                .fold(0, |all, (_, bit)| all | bit),
            left_trigger: f32::from(pad.bLeftTrigger) / 255.0,
            right_trigger: f32::from(pad.bRightTrigger) / 255.0,
            left_x: stick(pad.sThumbLX),
            left_y: stick(pad.sThumbLY),
            right_x: stick(pad.sThumbRX),
            right_y: stick(pad.sThumbRY),
        }
    }

    /// The title window's caption, with a hint while the mouse is free.
    #[derive(Default)]
    struct Caption {
        window: usize,
        /// The two captions, kept alive for good: a window that hangs may
        /// read the text after our call has given up.
        plain: &'static [u16],
        hinted: &'static [u16],
        shown: Option<bool>,
    }

    pub(super) struct Host {
        caption: Caption,
        /// Since when the foreground window has been none (the system is
        /// between two windows: one is being shown, hidden or destroyed).
        no_foreground: Option<Instant>,
        /// Why focus was last judged lost (for the release log).
        lost: String,
        empty_until: [Option<Instant>; PADS],
        /// Where scripted mouse motion goes (diagnostics).
        #[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
        counts: Arc<Counts>,
        was_focused: bool,
        was_captured: bool,
        clipped: bool,
        /// Mouse buttons held when the cursor was captured (the click that
        /// captured it): no shots until let go.
        swallowed: Vec<u8>,
        #[cfg(feature = "diagnostics")]
        kbm: kbm::Script,
    }

    impl Host {
        pub(super) fn new(counts: Arc<Counts>) -> Host {
            Host {
                caption: Caption::default(),
                no_foreground: None,
                lost: String::new(),
                empty_until: [None; PADS],
                counts,
                was_focused: false,
                was_captured: false,
                clipped: false,
                swallowed: Vec::new(),
                #[cfg(feature = "diagnostics")]
                kbm: kbm::Script::from_env(),
            }
        }

        /// A window of the title is in front. No foreground window at all
        /// is a gap while windows change (the splash fading and going away,
        /// an overlay shown): focus is kept through it for `NO_FOREGROUND`.
        fn focus(&mut self) -> bool {
            // SAFETY: plain query.
            let front = unsafe { GetForegroundWindow() } as usize;
            if front == 0 {
                let since = *self.no_foreground.get_or_insert_with(Instant::now);
                if since.elapsed() < NO_FOREGROUND {
                    return self.was_focused;
                }
                self.lost = "no foreground window for too long".into();
                return false;
            }
            self.no_foreground = None;
            let ours = cursor::ours(front);
            if !ours {
                self.lost = format!("another program's window {front:#x} is in front");
            }
            ours
        }

        /// While the mouse is not captured the window caption says how to
        /// capture it (`kbm::caption`); captured, the title's own again.
        fn title_hint(&mut self, focus: bool) {
            if !cursor::ENABLED.load(Ordering::Relaxed) {
                return;
            }
            let caption = &mut self.caption;
            if focus {
                // SAFETY: plain query.
                let hwnd = cursor::target();
                if hwnd != caption.window {
                    let mut text = [0u16; 256];
                    // SAFETY: room for 256 units; the window is ours.
                    let n = unsafe { GetWindowTextW(hwnd as _, text.as_mut_ptr(), 256) };
                    let base = String::from_utf16_lossy(&text[..n.max(0) as usize]);
                    let base = victory_core::kbm::caption(&base, true);
                    let wide = |s: String| -> &'static [u16] {
                        Box::leak(s.encode_utf16().chain([0]).collect::<Vec<_>>().into_boxed_slice())
                    };
                    caption.hinted = wide(victory_core::kbm::caption(&base, false));
                    caption.plain = wide(base);
                    caption.window = hwnd;
                    caption.shown = None;
                }
            }
            if caption.window == 0 {
                return;
            }
            let captured = focus && cursor::captured();
            if caption.shown != Some(captured) {
                let text = if captured { caption.plain } else { caption.hinted };
                let mut done = 0;
                // SAFETY: a null-terminated caption that lives for good; a
                // window that is gone or hung just fails or times out.
                let ok = unsafe {
                    SendMessageTimeoutW(
                        caption.window as _,
                        WM_SETTEXT,
                        0,
                        text.as_ptr() as isize,
                        SMTO_ABORTIFHUNG,
                        250,
                        &mut done,
                    )
                };
                // A hung window is tried again on a later poll.
                caption.shown = (ok != 0).then_some(captured);
            }
        }

        /// Frees the cursor when the title window loses focus, and holds a
        /// captured one inside the window.
        fn cursor(&mut self, focus: bool) {
            cursor::FOCUSED.store(focus, Ordering::Relaxed);
            // Focus loss (Alt-Tab, another window) frees the cursor; only a
            // click in the window captures it again.
            if !focus {
                let why = std::mem::take(&mut self.lost);
                cursor::release(&why);
            }
            self.was_focused = focus;
            self.title_hint(focus);
            if focus && cursor::captured() {
                // Re-asserted every poll: the system resets the clip when
                // another window activates.
                self.clipped = cursor::clip(cursor::target()) || self.clipped;
            } else if self.clipped {
                // SAFETY: lets go of the clip we set.
                unsafe { ClipCursor(std::ptr::null()) };
                self.clipped = false;
            }
        }
    }

    /// Mouse motion and the wheel from raw input (no pointer acceleration),
    /// on a thread and message-only window of its own. Counted only while the
    /// cursor is captured in the focused title window.
    pub(super) fn raw_mouse(counts: Arc<Counts>) {
        let spawned = std::thread::Builder::new()
            .name("victory-mouse".into())
            .spawn(move || {
                let class: Vec<u16> = "victory.mouse\0".encode_utf16().collect();
                // SAFETY: a window class and a message-only window of this
                // thread; the registration names that window.
                let hwnd = unsafe {
                    let instance = GetModuleHandleW(std::ptr::null());
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(DefWindowProcW),
                        hInstance: instance,
                        lpszClassName: class.as_ptr(),
                        ..std::mem::zeroed()
                    };
                    RegisterClassW(&wc);
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        std::ptr::null(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        HWND_MESSAGE,
                        std::ptr::null_mut(),
                        instance,
                        std::ptr::null(),
                    )
                };
                let device = RAWINPUTDEVICE {
                    usUsagePage: 1, // generic desktop
                    usUsage: 2,     // mouse
                    dwFlags: RIDEV_INPUTSINK,
                    hwndTarget: hwnd,
                };
                // SAFETY: one valid device description.
                let ok = !hwnd.is_null()
                    && unsafe {
                        RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32)
                    } != 0;
                if !ok {
                    tracing::warn!("raw mouse input unavailable: no mouse look");
                    return;
                }
                tracing::info!("raw mouse input on");
                let mut wheel = 0;
                // SAFETY: the standard message loop of this thread's window.
                unsafe {
                    let mut msg: MSG = std::mem::zeroed();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        if msg.message == WM_INPUT {
                            raw_input(msg.lParam as HRAWINPUT, &counts, &mut wheel);
                        }
                        DispatchMessageW(&msg);
                    }
                }
            });
        if spawned.is_err() {
            tracing::warn!("no raw mouse thread: no mouse look");
        }
    }

    /// One raw input event; `wheel` keeps the part of a step a fine-grained
    /// wheel has turned.
    fn raw_input(handle: HRAWINPUT, counts: &Counts, wheel: &mut i32) {
        /// Relative motion is the default; absolute comes from tablets,
        /// remote desktops and virtual machines.
        const MOUSE_MOVE_ABSOLUTE: u16 = 0x1;
        const RI_MOUSE_WHEEL: u16 = 0x0400;
        // SAFETY: zeroed is a valid RAWINPUT; `size` is its room.
        let mut raw: RAWINPUT = unsafe { std::mem::zeroed() };
        let mut size = size_of::<RAWINPUT>() as u32;
        // SAFETY: the handle of a WM_INPUT this thread received.
        let got = unsafe {
            GetRawInputData(
                handle,
                RID_INPUT,
                (&raw mut raw).cast(),
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if got == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE {
            return;
        }
        if !(cursor::FOCUSED.load(Ordering::Relaxed) && cursor::captured()) {
            *wheel = 0;
            return;
        }
        // SAFETY: a mouse event, so the mouse member is the one written.
        let mouse = unsafe { raw.data.mouse };
        if mouse.usFlags & MOUSE_MOVE_ABSOLUTE == 0 {
            counts.add(i64::from(mouse.lLastX), i64::from(mouse.lLastY));
        }
        // SAFETY: both members of the button union are plain integers.
        let buttons = unsafe { mouse.Anonymous.Anonymous };
        if buttons.usButtonFlags & RI_MOUSE_WHEEL != 0 {
            *wheel += i32::from(buttons.usButtonData as i16);
            let steps = *wheel / 120;
            *wheel -= steps * 120;
            if steps != 0 {
                counts.add_wheel(steps);
            }
        }
    }

    impl Source for Host {
        fn pad(&mut self, slot: usize) -> Option<PadState> {
            if self.empty_until[slot].is_some_and(|until| Instant::now() < until) {
                return None;
            }
            let mut state = XINPUT_STATE::default();
            // SAFETY: room for the state.
            if unsafe { XInputGetState(slot as u32, &mut state) } != 0 {
                self.empty_until[slot] = Some(Instant::now() + EMPTY_SLOT_RECHECK);
                return None;
            }
            self.empty_until[slot] = None;
            #[allow(unused_mut)]
            let mut pad = convert(&state.Gamepad);
            // The scripted keys (`VICTORY_KEYS`) also press the first
            // pad's buttons, by the stand-in pad's table: with a real pad
            // connected the stand-in is not there, and a title that listens
            // to the pad (the title's start screen) saw nothing.
            #[cfg(feature = "diagnostics")]
            if slot == 0 {
                for vk in scripted() {
                    if let Some(&(_, button)) = super::FALLBACK_BUTTONS.iter().find(|(key, _)| *key == vk) {
                        pad.buttons |= button;
                    }
                }
            }
            Some(pad)
        }

        fn keys(&mut self) -> Vec<KeyState> {
            let focus = self.focus();
            self.cursor(focus);
            #[cfg(feature = "diagnostics")]
            let mut down = {
                let mut down = scripted();
                down.extend(self.kbm.keys(&self.counts));
                down
            };
            #[cfg(not(feature = "diagnostics"))]
            let mut down = Vec::new();
            if focus {
                // Left/right modifier keys only, as devices report them; no
                // mouse buttons (below 0x08).
                let held: Vec<u8> = (0x08..=0xFE_u8)
                    .filter(|vk| !(0x10..=0x12).contains(vk) && !down.contains(vk))
                    // SAFETY: plain queries.
                    .filter(|&vk| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0)
                    .collect();
                down.extend(held);
            }
            down.into_iter()
                .map(|vk| KeyState {
                    // SAFETY: plain query.
                    scan_code: unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC_EX) },
                    virtual_key: vk,
                    // shortcut: no characters (code point 0);
                    // the title reads scan codes and virtual keys only.
                    ..Default::default()
                })
                .collect()
        }

        fn mouse_buttons(&mut self) -> Vec<u8> {
            let captured = cursor::FOCUSED.load(Ordering::Relaxed) && cursor::captured();
            let mut down = Vec::new();
            if captured {
                let held: Vec<u8> = MOUSE_BUTTONS
                    .into_iter()
                    // SAFETY: plain queries.
                    .filter(|&vk| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0)
                    .collect();
                if !self.was_captured {
                    self.swallowed.clone_from(&held);
                }
                self.swallowed.retain(|b| held.contains(b));
                down = held
                    .into_iter()
                    .filter(|b| !self.swallowed.contains(b))
                    .collect();
            }
            self.was_captured = captured;
            #[cfg(feature = "diagnostics")]
            down.extend(self.kbm.buttons());
            down
        }

        fn rumble(&mut self, slot: usize, low: u16, high: u16) {
            let speeds = XINPUT_VIBRATION {
                wLeftMotorSpeed: low,
                wRightMotorSpeed: high,
            };
            // SAFETY: a valid vibration; an empty slot just fails.
            unsafe { XInputSetState(slot as u32, &speeds) };
        }
    }

    /// Debugging aid: `VICTORY_KEYS=<seconds>:<key>,...` holds each key for
    /// 0.2 s that long after input starts, with or without focus. A key is a
    /// letter, a digit, or `Enter`, `Esc`, `Tab`, `Back`, `Up`, `Down`,
    /// `Left`, `Right` (the stand-in pad's table maps them to buttons).
    #[cfg(feature = "diagnostics")]
    fn scripted() -> Vec<u8> {
        static SCRIPT: LazyLock<(Instant, Vec<(Duration, u8)>)> = LazyLock::new(|| {
            let list = std::env::var("VICTORY_KEYS").unwrap_or_default();
            let keys = list.split(',').filter_map(|entry| {
                let (at, key) = entry.split_once(':')?;
                let vk = match key.trim() {
                    "Enter" => 0x0D,
                    "Esc" => 0x1B,
                    "Tab" => 0x09,
                    "Back" => 0x08,
                    "Up" => 0x26,
                    "Down" => 0x28,
                    "Left" => 0x25,
                    "Right" => 0x27,
                    k if k.len() == 1 && k.as_bytes()[0].is_ascii_alphanumeric() => {
                        k.as_bytes()[0].to_ascii_uppercase()
                    }
                    _ => return None,
                };
                Some((
                    Duration::try_from_secs_f64(at.trim().parse().ok()?).ok()?,
                    vk,
                ))
            });
            (Instant::now(), keys.collect())
        });
        let (start, keys) = &*SCRIPT;
        let now = start.elapsed();
        keys.iter()
            .filter(|&&(at, _)| (at..at + Duration::from_millis(200)).contains(&now))
            .map(|&(_, vk)| vk)
            .collect()
    }

    /// Debugging aid: `VICTORY_KBM_SCRIPT=<seconds>:<key>[*<hold seconds>],...`
    /// plays keyboard and mouse input into the controls layer as if held
    /// (with or without focus or capture). A key is a `[controls]` key name
    /// (`W`, `LShift`, `MouseLeft`...) held 0.2 s unless a hold is given;
    /// `MouseX=<counts>` / `MouseY=<counts>` move the mouse that far, spread
    /// over the hold; `Wheel=<steps>` turns the wheel (up positive).
    #[cfg(feature = "diagnostics")]
    mod kbm {
        use std::time::{Duration, Instant};

        use super::super::Counts;
        use crate::modules::controls::key_code;

        enum What {
            Key(u8),
            Mouse(f64, f64),
            Wheel(i32),
        }

        pub(super) struct Script {
            start: Instant,
            last: Duration,
            entries: Vec<(Duration, Duration, What)>,
            buttons: Vec<u8>,
        }

        impl Script {
            pub(super) fn from_env() -> Script {
                let list = std::env::var("VICTORY_KBM_SCRIPT").unwrap_or_default();
                let entries = list
                    .split(',')
                    .filter(|e| !e.trim().is_empty())
                    .filter_map(|entry| {
                        let parsed = parse(entry);
                        if parsed.is_none() {
                            tracing::warn!(entry, "VICTORY_KBM_SCRIPT entry not understood");
                        }
                        parsed
                    })
                    .collect();
                Script {
                    start: Instant::now(),
                    last: Duration::ZERO,
                    entries,
                    buttons: Vec::new(),
                }
            }

            /// Keys held now; adds the mouse motion since the last poll.
            pub(super) fn keys(&mut self, counts: &Counts) -> Vec<u8> {
                let now = self.start.elapsed();
                let last = std::mem::replace(&mut self.last, now);
                let mut keys = Vec::new();
                self.buttons.clear();
                for (at, hold, what) in &self.entries {
                    let end = *at + *hold;
                    match *what {
                        What::Key(vk) if (*at..end).contains(&now) => {
                            if vk <= 0x06 {
                                self.buttons.push(vk);
                            } else {
                                keys.push(vk);
                            }
                        }
                        What::Key(_) => {}
                        What::Mouse(x, y) => {
                            let from = last.max(*at);
                            let to = now.min(end);
                            if to > from && !hold.is_zero() {
                                let part = (to - from).as_secs_f64() / hold.as_secs_f64();
                                counts.add((x * part).round() as i64, (y * part).round() as i64);
                            }
                        }
                        What::Wheel(steps) => {
                            if last < *at && *at <= now {
                                counts.add_wheel(steps);
                            }
                        }
                    }
                }
                keys
            }

            /// Mouse buttons held now (from the last `keys`).
            pub(super) fn buttons(&self) -> Vec<u8> {
                self.buttons.clone()
            }
        }

        fn parse(entry: &str) -> Option<(Duration, Duration, What)> {
            let (at, rest) = entry.split_once(':')?;
            let (what, hold) = match rest.split_once('*') {
                Some((what, hold)) => (what, hold.trim().parse::<f64>().ok()?),
                None => (rest, 0.2),
            };
            let what = match what.trim().split_once('=') {
                Some((axis, n)) => {
                    let n: f64 = n.trim().parse().ok()?;
                    match axis.trim() {
                        "MouseX" => What::Mouse(n, 0.0),
                        "MouseY" => What::Mouse(0.0, n),
                        "Wheel" => What::Wheel(n as i32),
                        _ => return None,
                    }
                }
                None => What::Key(key_code(what)?),
            };
            Some((
                Duration::try_from_secs_f64(at.trim().parse().ok()?).ok()?,
                Duration::try_from_secs_f64(hold).ok()?,
                what,
            ))
        }
    }

    #[test]
    fn host_pads_convert_to_console_pads() {
        let pad = XINPUT_GAMEPAD {
            wButtons: XINPUT_GAMEPAD_A | XINPUT_GAMEPAD_START | XINPUT_GAMEPAD_DPAD_LEFT,
            bLeftTrigger: 255,
            sThumbLX: -32768,
            sThumbLY: 32767,
            ..Default::default()
        };
        let state = convert(&pad);
        assert_eq!(state.buttons, A | MENU | LEFT);
        assert_eq!(
            (
                state.left_trigger,
                state.left_x,
                state.left_y,
                state.right_x
            ),
            (1.0, -1.0, 1.0, 0.0)
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::HleResolver;
    use crate::HostPolicy;
    use victory_loader::Provenance;
    use victory_pe::ImportSymbol;

    #[derive(Default)]
    struct FakeHost {
        pads: [Option<PadState>; PADS],
        keys: Vec<KeyState>,
        buttons: Vec<u8>,
        rumble: Vec<(usize, u16, u16)>,
    }

    struct Fake(Arc<Mutex<FakeHost>>);

    impl Source for Fake {
        fn pad(&mut self, slot: usize) -> Option<PadState> {
            self.0.lock().unwrap().pads[slot]
        }
        fn keys(&mut self) -> Vec<KeyState> {
            self.0.lock().unwrap().keys.clone()
        }
        fn mouse_buttons(&mut self) -> Vec<u8> {
            self.0.lock().unwrap().buttons.clone()
        }
        fn rumble(&mut self, slot: usize, low: u16, high: u16) {
            self.0.lock().unwrap().rumble.push((slot, low, high));
        }
    }

    /// A hub with the old stand-in pad (`[controls] enabled = false`).
    fn fake() -> (&'static Hub, Arc<Mutex<FakeHost>>) {
        fake_with(super::super::controls::Settings {
            enabled: false,
            ..Default::default()
        })
    }

    /// A hub with keyboard and mouse controls (the defaults).
    fn fake_kbm() -> (&'static Hub, Arc<Mutex<FakeHost>>) {
        fake_with(Default::default())
    }

    fn fake_with(
        settings: super::super::controls::Settings,
    ) -> (&'static Hub, Arc<Mutex<FakeHost>>) {
        let host = Arc::new(Mutex::new(FakeHost::default()));
        let counts = Arc::new(Counts::default());
        let hub = Hub::new(Box::new(Fake(Arc::clone(&host))), counts, settings);
        hub.poll();
        (hub, host)
    }

    fn key(virtual_key: u8) -> KeyState {
        KeyState {
            scan_code: u32::from(virtual_key) + 0x100,
            virtual_key,
            ..Default::default()
        }
    }

    fn slot<F: Copy, T>(object: *const T, offset: usize) -> F {
        // SAFETY: our objects start with their table, which has the slot.
        let f = unsafe { *(*(object as *const *const *const ())).add(offset / 8) };
        // SAFETY: the caller names the slot's signature.
        unsafe { std::mem::transmute_copy(&f) }
    }

    /// Device callbacks seen: device index, current and previous status.
    type Log = Mutex<Vec<(usize, u32, u32)>>;

    extern "system" fn record(
        _token: u64,
        context: *mut c_void,
        device: *const Device,
        _timestamp: u64,
        current: u32,
        previous: u32,
    ) {
        // SAFETY: the test passes its log and one of the hub's devices.
        let (log, index) = unsafe { (&*(context as *const Log), (*device).index) };
        log.lock().unwrap().push((index, current, previous));
    }

    fn register(hub: &Hub, kind: u32, enumeration: u32, log: &Log) -> u64 {
        type Register = extern "system" fn(
            This,
            *const Device,
            u32,
            u32,
            u32,
            *mut c_void,
            Option<DeviceCallback>,
            *mut u64,
        ) -> HRESULT;
        let mut token = 0;
        let context = log as *const Log as *mut c_void;
        let register = slot::<Register, _>(&hub.object, 0x48);
        let hr = register(
            &hub.object,
            std::ptr::null(),
            kind,
            1,
            enumeration,
            context,
            Some(record),
            &mut token,
        );
        assert_eq!(hr, S_OK);
        assert_ne!(token, 0);
        token
    }

    fn result(hr: HRESULT, out: *const c_void) -> Result<*const Reading, HRESULT> {
        if hr == S_OK {
            Ok(out.cast())
        } else {
            assert!(out.is_null());
            Err(hr)
        }
    }

    fn current(hub: &Hub, kind: u32, device: usize) -> Result<*const Reading, HRESULT> {
        type Current = extern "system" fn(This, u32, *const Device, Out) -> HRESULT;
        let mut out = std::ptr::null();
        let hr = slot::<Current, _>(&hub.object, 0x20)(
            &hub.object,
            kind,
            &hub.devices[device],
            &mut out,
        );
        result(hr, out)
    }

    fn walk(hub: &Hub, offset: usize, from: *const Reading) -> Result<*const Reading, HRESULT> {
        type Walk = extern "system" fn(This, *const Reading, u32, *const Device, Out) -> HRESULT;
        let mut out = std::ptr::null();
        let device = reading(from).device;
        let hr =
            slot::<Walk, _>(&hub.object, offset)(&hub.object, from, GAMEPAD_KIND, device, &mut out);
        result(hr, out)
    }

    fn pad(reading: *const Reading) -> PadState {
        let mut state = PadState::default();
        let get =
            slot::<extern "system" fn(*const Reading, *mut PadState) -> bool, _>(reading, 0xb0);
        assert!(get(reading, &mut state));
        state
    }

    fn stamp(reading: *const Reading) -> (u64, u64) {
        let time = slot::<extern "system" fn(*const Reading) -> u64, _>(reading, 0x28);
        let sequence = slot::<extern "system" fn(*const Reading, u32) -> u64, _>(reading, 0x20);
        (time(reading), sequence(reading, GAMEPAD_KIND))
    }

    fn release(reading: *const Reading) {
        slot::<extern "system" fn(*const Reading) -> u32, _>(reading, 0x10)(reading);
    }

    #[test]
    fn ordinal_one_resolves_to_create() {
        let r = HleResolver::new(HostPolicy::none());
        assert_eq!(
            r.classify("GameInput.dll", &ImportSymbol::Ordinal(1)),
            Some(Provenance::Hle)
        );
        assert_eq!(r.classify("GameInput.dll", &ImportSymbol::Ordinal(2)), None);
    }

    #[test]
    fn pads_connect_report_and_disconnect() {
        let (hub, host) = fake();
        let log = Log::default();
        register(hub, GAMEPAD_KIND, 2, &log);
        // Blocking enumeration ran before registration returned: with no
        // host pad the keyboard's stand-in is the only one.
        assert_eq!(*log.lock().unwrap(), [(FALLBACK, STATUS_CONNECTED, 0)]);

        host.lock().unwrap().pads[1] = Some(PadState {
            buttons: A,
            left_x: 0.5,
            ..Default::default()
        });
        hub.poll();
        assert_eq!(
            log.lock().unwrap()[1..],
            [(1, STATUS_CONNECTED, 0), (FALLBACK, 0, STATUS_CONNECTED)]
        );

        let first = current(hub, GAMEPAD_KIND, 1).unwrap();
        assert_eq!(pad(first).buttons, A);
        assert_eq!(pad(first).left_x, 0.5);
        // No change, no new reading.
        hub.poll();
        let again = current(hub, GAMEPAD_KIND, 1).unwrap();
        assert_eq!(again, first);
        release(again);

        std::thread::sleep(Duration::from_millis(2));
        host.lock().unwrap().pads[1].as_mut().unwrap().buttons = B;
        hub.poll();
        let second = current(hub, GAMEPAD_KIND, 1).unwrap();
        assert_eq!(pad(second).buttons, B);
        let (t1, s1) = stamp(first);
        let (t2, s2) = stamp(second);
        assert!(t2 > t1 && s2 == s1 + 1);
        assert!(t2 <= now());

        // The title's edge check: previous of current, and back again.
        assert_eq!(walk(hub, 0x30, second), Ok(first));
        assert_eq!(walk(hub, 0x28, first), Ok(second));
        assert_eq!(walk(hub, 0x28, second), Err(READING_NOT_FOUND));
        assert_eq!(walk(hub, 0x30, first), Err(READING_NOT_FOUND));
        for r in [first, first, second, second] {
            release(r);
        }

        host.lock().unwrap().pads[1] = None;
        hub.poll();
        assert_eq!(
            log.lock().unwrap()[3..],
            [(1, 0, STATUS_CONNECTED), (FALLBACK, STATUS_CONNECTED, 0)]
        );
        assert_eq!(current(hub, GAMEPAD_KIND, 1), Err(DEVICE_DISCONNECTED));
        // The stand-in is back, at rest.
        let rest = current(hub, GAMEPAD_KIND, FALLBACK).unwrap();
        assert_eq!(pad(rest), PadState::default());
        release(rest);
    }

    #[test]
    fn keys_feed_the_keyboard_and_the_stand_in_pad() {
        let (hub, host) = fake();
        let log = Log::default();
        register(hub, KEYBOARD_KIND, 2, &log);
        assert_eq!(*log.lock().unwrap(), [(KEYBOARD, STATUS_CONNECTED, 0)]);

        let keys = vec![key(b'W'), key(0x0D), key(b'L'), key(b'C')];
        host.lock().unwrap().keys = keys.clone();
        hub.poll();
        let r = current(hub, KEYBOARD_KIND, KEYBOARD).unwrap();
        let count = slot::<extern "system" fn(*const Reading) -> u32, _>(r, 0x70);
        let state =
            slot::<extern "system" fn(*const Reading, u32, *mut KeyState) -> u32, _>(r, 0x78);
        assert_eq!(count(r), 4);
        let mut got = [KeyState::default(); 8];
        assert_eq!(state(r, 8, got.as_mut_ptr()), 4);
        assert_eq!(got[..4], keys[..]);
        release(r);

        let r = current(hub, GAMEPAD_KIND, FALLBACK).unwrap();
        assert_eq!(
            pad(r),
            PadState {
                buttons: A,
                left_y: 1.0,
                right_x: 1.0,
                right_trigger: 1.0,
                ..Default::default()
            }
        );
        release(r);
        // A keyboard reading is no gamepad reading.
        assert_eq!(current(hub, GAMEPAD_KIND, KEYBOARD), Err(READING_NOT_FOUND));
    }

    #[test]
    fn keyboard_and_mouse_drive_the_stand_in_pad_without_a_host_pad() {
        let (hub, host) = fake_kbm();
        {
            let mut h = host.lock().unwrap();
            h.keys = vec![key(b'W'), key(0x20)]; // W, Space
            h.buttons = vec![0x01]; // left mouse
        }
        hub.poll();
        let r = current(hub, GAMEPAD_KIND, FALLBACK).unwrap();
        let p = pad(r);
        assert_eq!((p.buttons, p.left_y, p.right_trigger), (X, 1.0, 1.0));
        release(r);
        // The keyboard device still has its keys.
        let r = current(hub, KEYBOARD_KIND, KEYBOARD).unwrap();
        let count = slot::<extern "system" fn(*const Reading) -> u32, _>(r, 0x70);
        assert_eq!(count(r), 2);
        release(r);
    }

    #[test]
    fn keyboard_and_mouse_ride_on_a_connected_host_pad() {
        let (hub, host) = fake_kbm();
        {
            let mut h = host.lock().unwrap();
            h.pads[1] = Some(PadState {
                buttons: B,
                left_trigger: 0.5,
                ..Default::default()
            });
            h.keys = vec![key(b'D'), key(0x20)];
            h.buttons = vec![0x02]; // right mouse: aim
        }
        hub.poll();
        let r = current(hub, GAMEPAD_KIND, 1).unwrap();
        let p = pad(r);
        assert_eq!(
            (p.buttons, p.left_x, p.left_trigger),
            (B | X, 1.0, 1.0),
            "pad and keyboard together"
        );
        release(r);
        assert_eq!(
            current(hub, GAMEPAD_KIND, FALLBACK),
            Err(DEVICE_DISCONNECTED),
            "no stand-in pad beside a host pad"
        );
    }

    #[test]
    fn a_title_pad_read_carries_the_mouse_motion_since_its_last_read() {
        let (hub, _host) = fake_kbm();
        let r = current(hub, GAMEPAD_KIND, FALLBACK).unwrap();
        let first = stamp(r).0;
        release(r);
        std::thread::sleep(Duration::from_millis(5));
        // Below a full stick (no carry into the next read).
        hub.counts.add(4, -2);
        let r = current(hub, GAMEPAD_KIND, FALLBACK).unwrap();
        let p = pad(r);
        assert!(stamp(r).0 > first, "a new reading at the read");
        assert!(p.right_x > 0.0 && p.right_y > 0.0, "{p:?}");
        assert!((p.right_x - 2.0 * p.right_y).abs() < 1e-4, "{p:?}");
        release(r);
        std::thread::sleep(Duration::from_millis(5));
        let r = current(hub, GAMEPAD_KIND, FALLBACK).unwrap();
        assert_eq!(pad(r).right_x, 0.0, "no motion, no turn");
        release(r);
    }

    #[test]
    fn async_enumeration_waits_for_the_poll_and_unregistering_stops_callbacks() {
        let (hub, host) = fake();
        let log = Log::default();
        let token = register(hub, GAMEPAD_KIND, ASYNC_ENUMERATION, &log);
        assert!(log.lock().unwrap().is_empty());
        hub.poll();
        assert_eq!(*log.lock().unwrap(), [(FALLBACK, STATUS_CONNECTED, 0)]);

        let unregister = slot::<extern "system" fn(This, u64, u64) -> bool, _>(&hub.object, 0x68);
        assert!(unregister(&hub.object, token, 5000));
        host.lock().unwrap().pads[0] = Some(PadState::default());
        hub.poll();
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn unregistering_waits_for_a_running_callback() {
        use std::sync::atomic::AtomicBool;
        static INSIDE: AtomicBool = AtomicBool::new(false);
        static LEAVE: AtomicBool = AtomicBool::new(false);
        static FROM_INSIDE: AtomicU32 = AtomicU32::new(0);
        extern "system" fn slow(
            token: u64,
            context: *mut c_void,
            _: *const Device,
            _: u64,
            _: u32,
            _: u32,
        ) {
            // From inside its own callback, unregistering fails at once.
            let unregister = slot::<extern "system" fn(This, u64, u64) -> bool, _>(context, 0x68);
            if !unregister(context.cast(), token, 0) {
                FROM_INSIDE.fetch_add(1, Ordering::SeqCst);
            }
            INSIDE.store(true, Ordering::SeqCst);
            while !LEAVE.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let (hub, host) = fake();
        type Register = extern "system" fn(
            This,
            *const Device,
            u32,
            u32,
            u32,
            *mut c_void,
            Option<DeviceCallback>,
            *mut u64,
        ) -> HRESULT;
        let register = slot::<Register, _>(&hub.object, 0x48);
        let mut token = 0;
        let context = (&raw const hub.object).cast_mut().cast();
        assert_eq!(
            register(&hub.object, std::ptr::null(), GAMEPAD_KIND, 1, 0, context, Some(slow), &mut token),
            S_OK
        );
        host.lock().unwrap().pads[1] = Some(PadState::default());
        let poller = std::thread::spawn(|| hub.poll());
        while !INSIDE.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let unregister = slot::<extern "system" fn(This, u64, u64) -> bool, _>(&hub.object, 0x68);
        assert!(!unregister(&hub.object, token, 5000), "times out while it runs");
        let leave = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            LEAVE.store(true, Ordering::SeqCst);
        });
        let start = Instant::now();
        assert!(unregister(&hub.object, token, u64::MAX), "waits for it");
        assert!(start.elapsed() >= Duration::from_millis(40));
        assert_eq!(FROM_INSIDE.load(Ordering::SeqCst), 1);
        poller.join().unwrap();
        leave.join().unwrap();
    }

    #[test]
    fn devices_are_found_by_id_and_rumble_reaches_the_host() {
        let (hub, host) = fake();
        host.lock().unwrap().pads[2] = Some(PadState::default());
        hub.poll();
        let device = &hub.devices[2] as *const Device;
        let info = slot::<extern "system" fn(*const Device) -> *const u8, _>(device, 0x18);
        // SAFETY: the info has the id at +0x20.
        let id = unsafe { *(info(device).add(0x20) as *const [u8; 32]) };
        assert_ne!(id, [0; 32]);

        type Find = extern "system" fn(This, *const [u8; 32], Out) -> HRESULT;
        let find = slot::<Find, _>(&hub.object, 0x80);
        let mut out = std::ptr::null();
        assert_eq!(find(&hub.object, &id, &mut out), S_OK);
        assert_eq!(out, device.cast());
        assert_eq!(find(&hub.object, &[1; 32], &mut out), DEVICE_NOT_FOUND);
        assert!(out.is_null());

        type Rumble = extern "system" fn(*const Device, *const [f32; 4]);
        let rumble = slot::<Rumble, _>(device, 0x50);
        rumble(device, &[0.5, 1.0, 1.0, 1.0]);
        rumble(&hub.devices[FALLBACK], &[1.0; 4]);
        rumble(device, std::ptr::null());
        assert_eq!(host.lock().unwrap().rumble, [(2, 32767, 65535), (2, 0, 0)]);
    }
}
