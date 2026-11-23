//! GameInput v0 polling and ordered snapshots. The SDK supplies the typed COM tables.
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
const INPUT_IID: GUID = GUID::from_u128(0x11be2a7e_4254_445a_9c09_ffc40f006918);
const DEVICE_IID: GUID = GUID::from_u128(0x31dd86fb_4c1b_408a_868f_439b3cd47125);
const READING_IID: GUID = GUID::from_u128(0x2156947a_e1fa_4de0_a30b_d812931dbd8d);

// These immutable objects are the vtables defined by the matching C SDK.
unsafe extern "C" {
    static gi_input_table: u8;
    static gi_device_table: u8;
    static gi_reading_table: u8;
}

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
    number: u64,
    mouse: MouseMotion,
    scan_at: u64,
    pads: [Option<Pad>; PORTS],
    desktop: Option<DesktopPad>,
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
                    table: unsafe { &gi_device_table },
                    value: Value::Device {
                        slot,
                        status: AtomicU32::new(0),
                    },
                })
            }),
            latest: Default::default(),
            history: VecDeque::with_capacity(HISTORY),
            number: 0,
            mouse: MouseMotion::default(),
            scan_at: 0,
            pads: [None; PORTS],
            desktop: (std::env::var_os("TUXBOX_KEYBOARD_GAMEPAD").is_some_and(|v| v == "1"))
                .then(DesktopPad::default),
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
            status.store(
                if connected {
                    1 | (u32::from(active) << 1)
                } else {
                    0
                },
                Ordering::Relaxed,
            );
            if !connected || !active {
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
        if !active {
            self.mouse.previous = None;
            return;
        }
        for slot in 0..DEVICES {
            let kind = kind(slot);
            if kind & mask == 0 {
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
                    Sample::Pad(self.desktop.as_mut().unwrap().merge(pad))
                }
                VIRTUAL => continue,
                _ => match self.pads[slot - 2] {
                    Some(pad) => Sample::Pad(pad),
                    None => continue,
                },
            };
            if self.latest[slot]
                .as_ref()
                .is_some_and(|s| s.reading().sample == data)
            {
                continue;
            }
            self.number += 1;
            let reading = Arc::new(Object {
                table: unsafe { &gi_reading_table },
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
        table: unsafe { &gi_input_table },
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
    state.refresh(mask);
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
    if kind == 0 || kind & r.kind != 0 {
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
        *out = *data;
        true
    } else {
        false
    }
}
