//! The GameInput v0 COM tables, slot for slot as the SDK's GameInput.h
//! declares IGameInput, IGameInputReading and IGameInputDevice. Ported from
//! abi.cpp, which built them from the SDK's C vtables. Slots the runtime
//! doesn't implement return E_NOTIMPL (or false, zero or nothing for their
//! return type) and clear their interface out-parameters, as abi.cpp's
//! Unavailable did.
use crate::{Object, gi_add, gi_query, gi_release};
use crate::callbacks::{gi_register_device, gi_stop, gi_unregister};
use crate::{gi_device, gi_key_count, gi_keys, gi_kind, gi_mouse, gi_pad, gi_read, gi_sequence, gi_slot, gi_status, gi_time, gi_timestamp};

const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;

#[repr(C)]
pub(crate) struct Table<const N: usize>([*const (); N]);
// Immutable function pointers.
unsafe impl<const N: usize> Sync for Table<N> {}

impl<const N: usize> Table<N> {
    pub(crate) fn first(&'static self) -> &'static u8 {
        unsafe { &*self.0.as_ptr().cast::<u8>() }
    }
}

unsafe extern "system" fn not_implemented() -> i32 {
    E_NOTIMPL
}
unsafe extern "system" fn nothing() {}
unsafe extern "system" fn zero() -> u64 {
    0
}
unsafe extern "system" fn clear_1(_: usize, out: *mut usize) -> i32 {
    if !out.is_null() { unsafe { *out = 0 } }
    E_NOTIMPL
}
unsafe extern "system" fn clear_2(_: usize, _: usize, out: *mut usize) -> i32 {
    if !out.is_null() { unsafe { *out = 0 } }
    E_NOTIMPL
}
unsafe extern "system" fn clear_3(_: usize, _: usize, _: usize, out: *mut usize) -> i32 {
    if !out.is_null() { unsafe { *out = 0 } }
    E_NOTIMPL
}
/// GetRawReport: no report.
unsafe extern "system" fn no_report(_: usize, out: *mut usize) -> bool {
    if !out.is_null() { unsafe { *out = 0 } }
    false
}

unsafe extern "system" fn current(this: &Object, kind: u32, device: *const Object, out: *mut *const Object) -> i32 {
    unsafe { gi_read(this, kind, device, core::ptr::null(), 0, out) }
}
unsafe extern "system" fn next(this: &Object, reference: *const Object, kind: u32, device: *const Object, out: *mut *const Object) -> i32 {
    unsafe { gi_read(this, kind, device, reference, 1, out) }
}
unsafe extern "system" fn previous(this: &Object, reference: *const Object, kind: u32, device: *const Object, out: *mut *const Object) -> i32 {
    unsafe { gi_read(this, kind, device, reference, -1, out) }
}
unsafe extern "system" fn timestamp(_: &Object) -> u64 {
    gi_time()
}

macro_rules! f {
    ($f:expr) => { $f as *const () };
}

pub(crate) static INPUT: Table<22> = Table([
    f!(gi_query), f!(gi_add), f!(gi_release),
    f!(timestamp), f!(current), f!(next), f!(previous),
    f!(clear_3), // GetTemporalReading
    f!(accept_reading), // RegisterReadingCallback
    f!(gi_register_device),
    f!(accept_reading), // RegisterSystemButtonCallback (same shape)
    f!(accept_layout), // RegisterKeyboardLayoutCallback
    f!(gi_stop), f!(gi_unregister),
    f!(clear_1), // CreateDispatcher
    f!(clear_2), // CreateAggregateDevice
    f!(clear_2), f!(clear_2), f!(clear_2), f!(clear_2), // FindDeviceFromId/Object/PlatformHandle/PlatformString
    f!(not_implemented), // EnableOemDeviceSupport
    f!(nothing), // SetFocusPolicy
]);

pub(crate) static READING: Table<25> = Table([
    f!(gi_query), f!(gi_add), f!(gi_release),
    f!(gi_kind), f!(gi_sequence), f!(gi_timestamp), f!(gi_device),
    f!(no_report), // GetRawReport
    f!(zero), f!(zero), f!(zero), f!(zero), f!(zero), f!(zero), // controller axes, buttons, switches
    f!(gi_key_count), f!(gi_keys), f!(gi_mouse),
    f!(zero), f!(zero), // touch
    f!(zero), f!(zero), f!(zero), // motion, arcade stick, flight stick
    f!(gi_pad),
    f!(zero), f!(zero), // racing wheel, UI navigation
]);

pub(crate) static DEVICE: Table<22> = Table([
    f!(gi_query), f!(gi_add), f!(gi_release),
    f!(info), f!(gi_status), f!(battery),
    f!(clear_3), // CreateForceFeedbackEffect
    f!(zero), // IsForceFeedbackMotorPoweredOn
    f!(nothing), // SetForceFeedbackMotorGain
    f!(not_implemented), // SetHapticMotorState
    f!(nothing), f!(nothing), f!(nothing), f!(nothing), // SetRumbleState, SetInputSynchronizationState, SendInputSynchronizationHint, PowerOff
    f!(clear_3), // CreateRawDeviceReport
    f!(clear_2), // GetRawDeviceFeature
    f!(not_implemented), f!(not_implemented), // SetRawDeviceFeature, SendRawDeviceOutput
    f!(clear_2), // SendRawDeviceOutputWithResponse
    f!(not_implemented), // ExecuteRawDeviceIoControl
    f!(zero), // AcquireExclusiveRawDeviceAccess
    f!(nothing), // ReleaseExclusiveRawDeviceAccess
]);

/// GameInputBatteryState, zeroed.
unsafe extern "system" fn battery(_: &Object, state: *mut [u32; 5]) {
    if !state.is_null() { unsafe { *state = [0; 5] } }
}

#[repr(C)]
struct KeyboardInfo {
    kind: u32,
    layout: u32,
    key_count: u32,
    function_key_count: u32,
    max_simultaneous_keys: u32,
    platform_type: u32,
    platform_subtype: u32,
    native_language: usize,
}

#[repr(C)]
struct MouseInfo {
    supported_buttons: u32,
    sample_rate: u32,
    sensor_dpi: u32,
    has_wheel_x: bool,
    has_wheel_y: bool,
}

/// GameInputGamepadInfo: fourteen button labels.
#[repr(C)]
struct GamepadInfo([u32; 14]);

#[repr(C)]
#[derive(Clone, Copy)]
struct DeviceInfo {
    info_size: u32,
    vendor_id: u16,
    product_id: u16,
    revision_number: u16,
    interface_number: u8,
    collection_number: u8,
    usage: [u16; 2],
    hardware_version: [u16; 4],
    firmware_version: [u16; 4],
    device_id: [u8; 32],
    device_root_id: [u8; 32],
    device_family: u32,
    capabilities: u32,
    supported_input: u32,
    supported_rumble_motors: u32,
    /// input, output, feature reports; axes, buttons, switches; touch points,
    /// touch sensors; force-feedback, haptic motors; strings, descriptor size
    counts: [u32; 12],
    /// input/output/feature report, axis, button, switch, keyboard, mouse,
    /// touch sensor, motion, arcade stick, flight stick, gamepad, racing
    /// wheel, UI navigation, force feedback, haptic, display name, strings,
    /// descriptor
    info: [usize; 20],
    supported_system_buttons: u32,
}

const _: () = assert!(size_of::<DeviceInfo>() == 328 && core::mem::offset_of!(DeviceInfo, info) == 160);
const _: () = assert!(size_of::<KeyboardInfo>() == 40 && size_of::<MouseInfo>() == 16 && size_of::<GamepadInfo>() == 56);
const KEYBOARD_INFO: usize = 6;
const MOUSE_INFO: usize = 7;
const GAMEPAD_INFO: usize = 12;

static KEYBOARD: KeyboardInfo = KeyboardInfo {
    kind: 0, // GameInputAnsiKeyboard
    layout: 0, key_count: 0, function_key_count: 0, max_simultaneous_keys: 256, platform_type: 0, platform_subtype: 0, native_language: 0,
};
static MOUSE: MouseInfo = MouseInfo {
    supported_buttons: 0x1 | 0x2 | 0x4 | 0x8 | 0x10, // left, right, middle, 4, 5
    sample_rate: 0, sensor_dpi: 0, has_wheel_x: false, has_wheel_y: false,
};
static PAD: GamepadInfo = GamepadInfo([0; 14]);

/// GetDeviceInfo: slot 0 the keyboard, 1 the mouse, the rest gamepads.
unsafe extern "system" fn info(this: &Object) -> *const DeviceInfo {
    static DEVICES: std::sync::LazyLock<[DeviceInfo; crate::DEVICES]> = std::sync::LazyLock::new(|| {
        core::array::from_fn(|i| {
            let mut id = [0u8; 32];
            id[0] = (i + 1) as u8;
            let mut info = [0usize; 20];
            match i {
                0 => info[KEYBOARD_INFO] = &raw const KEYBOARD as usize,
                1 => info[MOUSE_INFO] = &raw const MOUSE as usize,
                _ => info[GAMEPAD_INFO] = &raw const PAD as usize,
            }
            DeviceInfo {
                info_size: size_of::<DeviceInfo>() as u32,
                vendor_id: 0, product_id: 0, revision_number: 0, interface_number: 0, collection_number: 0,
                usage: [0; 2], hardware_version: [0; 4], firmware_version: [0; 4],
                device_id: id, device_root_id: id,
                device_family: if i < 2 { 3 } else { 2 }, // GameInputFamilyHid, GameInputFamilyXbox360
                capabilities: 0,
                supported_input: match i { 0 => 0x10, 1 => 0x20, _ => 0x4_0000 }, // keyboard, mouse, gamepad
                supported_rumble_motors: 0,
                counts: [0; 12],
                info,
                supported_system_buttons: 0,
            }
        })
    });
    &DEVICES[gi_slot(this) as usize]
}

// Reading, system-button and keyboard-layout callbacks are accepted with a
// token and never fire (the title polls readings), as VIctory-fxceless
// gameinput.rs (GPL-3.0, credit) does; E_NOTIMPL could fail the title's input
// setup. TUXBOX_GI_ACCEPT_CALLBACKS=0 refuses them again.
pub(crate) const ACCEPTED: u64 = 1 << 63;
fn accept(token: *mut u64) -> i32 {
    if std::env::var_os("TUXBOX_GI_ACCEPT_CALLBACKS").is_some_and(|v| v == "0") {
        return E_NOTIMPL;
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    if !token.is_null() {
        unsafe { *token = ACCEPTED | NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) };
    }
    0
}
unsafe extern "system" fn accept_reading(_: usize, _: usize, _: u32, _: usize, _: usize, token: *mut u64) -> i32 {
    accept(token)
}
unsafe extern "system" fn accept_layout(_: usize, _: usize, _: usize, _: usize, token: *mut u64) -> i32 {
    accept(token)
}
