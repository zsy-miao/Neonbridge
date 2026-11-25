//! DualSense (PlayStation 5) pads over HID.
//!
//! The console reports gamepads through GameInput; on a PC a DualSense is not
//! an XInput device, so the title never sees it. This reads the pad's HID
//! reports and exposes the same state shape the XInput source does.
//!
//! The reports differ between USB (64 bytes) and Bluetooth (78 bytes, and a
//! different offset for the buttons and triggers), so fields are read through
//! Windows' HID parser (`HidP_*`) by their declared usages rather than by
//! fixed offsets. Both transports and the Edge's report then read the same.
//!
//! shortcut: touchpad, gyro and the lightbar are not reported (the title's
//! pad surface has no axes for them); rumble is not sent back yet.

#![allow(non_snake_case)]

use std::sync::{Mutex, Once};
use std::time::Duration;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    GUID_DEVINTERFACE_HID, HID_USAGE_GENERIC_HATSWITCH, HID_USAGE_GENERIC_RX, HID_USAGE_GENERIC_RY,
    HID_USAGE_GENERIC_RZ, HID_USAGE_GENERIC_X, HID_USAGE_GENERIC_Y, HID_USAGE_GENERIC_Z,
    HID_USAGE_PAGE_BUTTON, HID_USAGE_PAGE_GENERIC, HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS,
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetPreparsedData, HidP_GetCaps,
    HidP_GetUsageValue, HidP_GetUsages, HidP_Input,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
};

/// Gamepad buttons, the bits the title maps (same values as `gameinput`).
const MENU: u32 = 0x1;
const VIEW: u32 = 0x2;
const A: u32 = 0x4;
const B: u32 = 0x8;
const X: u32 = 0x10;
const Y: u32 = 0x20;
const UP: u32 = 0x40;
const DOWN: u32 = 0x80;
const LEFT: u32 = 0x100;
const RIGHT: u32 = 0x200;
const LEFT_SHOULDER: u32 = 0x400;
const RIGHT_SHOULDER: u32 = 0x800;
const LEFT_STICK: u32 = 0x1000;
const RIGHT_STICK: u32 = 0x2000;

/// Sony's vendor id and the DualSense / DualSense Edge product ids.
const SONY: u16 = 0x054c;
const DUALSENSE: u16 = 0x0ce6;
const DUALSENSE_EDGE: u16 = 0x0df2;

const GENERIC_READ: u32 = 0x8000_0000;

/// A pad reading: triggers 0 to 1, sticks -1 to 1 (up positive).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
}

static READING: Mutex<Option<Reading>> = Mutex::new(None);
static START: Once = Once::new();

/// The pad's newest state, starting the reader on first use.
pub fn latest() -> Option<Reading> {
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("dualsense".into())
            .spawn(reader);
    });
    *READING.lock().unwrap_or_else(|e| e.into_inner())
}

/// An open pad: its input handle, the HID parser's description of it (freed
/// on drop) and the input report length.
struct Pad {
    handle: HANDLE,
    preparsed: isize,
    input_len: usize,
}

impl Drop for Pad {
    fn drop(&mut self) {
        // SAFETY: both came from the HID API for this handle.
        unsafe {
            HidD_FreePreparsedData(self.preparsed);
            CloseHandle(self.handle);
        }
    }
}

fn reader() {
    loop {
        if let Some(pad) = open_pad() {
            read_reports(&pad);
        }
        *READING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // A pad that is off or out of range is looked for again.
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Reads until the pad goes away. A report that does not parse is ignored.
fn read_reports(pad: &Pad) {
    let mut buf = vec![0u8; pad.input_len.max(78)];
    let log = std::env::var_os("VICTORY_PAD_LOG").is_some();
    let mut last: Option<Reading> = None;
    loop {
        let mut read = 0u32;
        // SAFETY: a live handle and a buffer of its report length.
        let ok = unsafe {
            ReadFile(
                pad.handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            return;
        }
        let Some(reading) = parse(pad.preparsed, &buf[..read as usize]) else {
            continue;
        };
        if last == Some(reading) {
            continue;
        }
        if log && last.is_none_or(|before| before.buttons != reading.buttons) {
            tracing::info!(
                buttons = format_args!("{:#x}", reading.buttons),
                left = format_args!("{:.2},{:.2}", reading.left_x, reading.left_y),
                right = format_args!("{:.2},{:.2}", reading.right_x, reading.right_y),
                triggers = format_args!("{:.2},{:.2}", reading.left_trigger, reading.right_trigger),
                "DualSense report"
            );
        }
        last = Some(reading);
        *READING.lock().unwrap_or_else(|e| e.into_inner()) = Some(reading);
    }
}

/// Opens the first DualSense and keeps its HID description.
fn open_pad() -> Option<Pad> {
    for path in device_paths() {
        // SAFETY: a path from SetupAPI, opened for reading only.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }
        // SAFETY: a plain Windows struct for which all zeroes is valid.
        let mut attributes: HIDD_ATTRIBUTES = unsafe { std::mem::zeroed() };
        attributes.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
        // SAFETY: a live HID handle and room for the attributes.
        let known = unsafe { HidD_GetAttributes(handle, &mut attributes) };
        if known
            && attributes.VendorID == SONY
            && (attributes.ProductID == DUALSENSE || attributes.ProductID == DUALSENSE_EDGE)
        {
            let mut preparsed: isize = 0;
            // SAFETY: a live HID handle and room for the pointer.
            if !unsafe { HidD_GetPreparsedData(handle, &mut preparsed) } {
                // SAFETY: the handle came from CreateFileW above.
                unsafe { CloseHandle(handle) };
                continue;
            }
            // SAFETY: a plain Windows struct for which all zeroes is valid.
            let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
            // SAFETY: the preparations are ours until freed.
            let status = unsafe { HidP_GetCaps(preparsed, &mut caps) };
            let input_len = if status == HIDP_STATUS_SUCCESS {
                usize::from(caps.InputReportByteLength).max(1)
            } else {
                78
            };
            tracing::info!(
                product = format_args!("{:#06x}", attributes.ProductID),
                report = input_len,
                "DualSense connected over HID"
            );
            return Some(Pad {
                handle,
                preparsed,
                input_len,
            });
        }
        // SAFETY: the handle came from CreateFileW above and was not kept.
        unsafe { CloseHandle(handle) };
    }
    None
}

/// Every present HID interface's device path.
fn device_paths() -> Vec<Vec<u16>> {
    let mut paths = Vec::new();
    // SAFETY: the HID interface class, no enumerator or parent window.
    let set: HDEVINFO = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_HID,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == -1 {
        return paths;
    }
    let mut index = 0u32;
    loop {
        // SAFETY: a plain Windows struct for which all zeroes is valid.
        let mut interface: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
        interface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        // SAFETY: the set from above, and room for one interface.
        if unsafe {
            SetupDiEnumDeviceInterfaces(
                set,
                std::ptr::null(),
                &GUID_DEVINTERFACE_HID,
                index,
                &mut interface,
            )
        } == 0
        {
            break;
        }
        index += 1;
        let mut required = 0u32;
        // SAFETY: asks for the room the detail needs.
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                std::ptr::null_mut(),
                0,
                &mut required,
                std::ptr::null_mut(),
            )
        };
        if required == 0 {
            continue;
        }
        let mut buffer = vec![0u8; required as usize];
        let detail = buffer
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY: the buffer is the size the API asked for; cbSize is its header.
        unsafe {
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                detail,
                required,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                continue;
            }
        }
        // The path starts after the u32 cbSize, as UTF-16.
        // SAFETY: the API wrote the path at this offset in our buffer.
        let path = unsafe { buffer.as_ptr().add(4).cast::<u16>() };
        let mut len = 0usize;
        // SAFETY: the API wrote a NUL-terminated path into the buffer.
        while unsafe { *path.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `path` is valid for `len` u16s in the live buffer.
        paths.push(unsafe { std::slice::from_raw_parts(path, len) }.to_vec());
    }
    // SAFETY: the set is ours.
    unsafe { SetupDiDestroyDeviceInfoList(set) };
    paths
}

/// A declared usage's value in `report`, if the descriptor has it.
fn usage_value(preparsed: isize, report: &[u8], usage: u16) -> Option<u32> {
    let mut value = 0u32;
    // SAFETY: the report and the parser's description are live.
    let status = unsafe {
        HidP_GetUsageValue(
            HidP_Input,
            HID_USAGE_PAGE_GENERIC,
            0,
            usage,
            &mut value,
            preparsed,
            report.as_ptr(),
            report.len() as u32,
        )
    };
    (status == HIDP_STATUS_SUCCESS).then_some(value)
}

/// The buttons held in `report`, as HID usage numbers 1 to 14.
fn usage_buttons(preparsed: isize, report: &[u8]) -> Vec<u16> {
    let mut usages = [0u16; 32];
    let mut len = usages.len() as u32;
    // SAFETY: the report and the parser's description are live; the API only
    // reads the report.
    let status = unsafe {
        HidP_GetUsages(
            HidP_Input,
            HID_USAGE_PAGE_BUTTON,
            0,
            usages.as_mut_ptr(),
            &mut len,
            preparsed,
            report.as_ptr() as *mut u8,
            report.len() as u32,
        )
    };
    if status == HIDP_STATUS_SUCCESS {
        usages[..len as usize].to_vec()
    } else {
        Vec::new()
    }
}

/// Whether `report` is an input report this module parses: USB's 0x01 or
/// Bluetooth's 0x31 (the HID parser wants the report ID in the buffer).
fn input_report(report: &[u8]) -> bool {
    matches!(report.first(), Some(0x01 | 0x31))
}

/// The pad state from the report's declared fields.
///
/// Button usages, in Sony's order: 1 square, 2 cross, 3 circle, 4 triangle,
/// 5 L1, 6 R1, 7 L2, 8 R2, 9 create, 10 options, 11 L3, 12 R3, 13 PS,
/// 14 touchpad. Square, cross, circle and triangle become the title's X, A,
/// B and Y, so circle is back as on the console.
fn parse(preparsed: isize, report: &[u8]) -> Option<Reading> {
    if report.len() < 2 || !input_report(report) {
        return None;
    }
    let axis = |usage| usage_value(preparsed, report, usage).unwrap_or(128);
    let trigger = |usage| usage_value(preparsed, report, usage).unwrap_or(0);
    Some(state(
        [
            axis(HID_USAGE_GENERIC_X),
            axis(HID_USAGE_GENERIC_Y),
            axis(HID_USAGE_GENERIC_Z),
            axis(HID_USAGE_GENERIC_RZ),
        ],
        [trigger(HID_USAGE_GENERIC_RX), trigger(HID_USAGE_GENERIC_RY)],
        usage_value(preparsed, report, HID_USAGE_GENERIC_HATSWITCH).unwrap_or(8),
        &usage_buttons(preparsed, report),
    ))
}

/// Turns the declared fields into the title's pad state.
fn state(sticks: [u32; 4], triggers: [u32; 2], hat: u32, usages: &[u16]) -> Reading {
    let centered = |v: u32| ((f32::from(v as u8) - 127.5) / 127.5).clamp(-1.0, 1.0);
    let mut buttons = 0u32;
    for usage in usages {
        buttons |= match usage {
            1 => X,              // square
            2 => A,              // cross
            3 => B,              // circle
            4 => Y,              // triangle
            5 => LEFT_SHOULDER,  // L1
            6 => RIGHT_SHOULDER, // R1
            9 => VIEW,           // create
            10 => MENU,          // options
            11 => LEFT_STICK,
            12 => RIGHT_STICK,
            13 => MENU, // PS
            _ => 0,
        };
    }
    // 0 is up and the directions run clockwise; 8 is released.
    buttons |= match hat {
        0 => UP,
        1 => UP | RIGHT,
        2 => RIGHT,
        3 => DOWN | RIGHT,
        4 => DOWN,
        5 => DOWN | LEFT,
        6 => LEFT,
        7 => UP | LEFT,
        _ => 0,
    };
    Reading {
        buttons,
        left_trigger: f32::from(triggers[0] as u8) / 255.0,
        right_trigger: f32::from(triggers[1] as u8) / 255.0,
        left_x: centered(sticks[0]),
        // The pad's Y axes grow downwards; the title's sticks grow upwards.
        left_y: -centered(sticks[1]),
        right_x: centered(sticks[2]),
        right_y: -centered(sticks[3]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER: [u32; 4] = [128, 128, 128, 128];
    const UP_NEUTRAL: u32 = 8;

    fn pad(usages: &[u16], hat: u32) -> Reading {
        state(CENTER, [0, 0], hat, usages)
    }

    #[test]
    fn cross_circle_square_and_triangle_map() {
        assert_eq!(pad(&[2], UP_NEUTRAL).buttons, A);
        assert_eq!(pad(&[3], UP_NEUTRAL).buttons, B);
        assert_eq!(pad(&[1], UP_NEUTRAL).buttons, X);
        assert_eq!(pad(&[4], UP_NEUTRAL).buttons, Y);
    }

    #[test]
    fn shoulders_system_and_stick_clicks_map() {
        assert_eq!(
            pad(&[5, 6, 9, 10, 11, 12, 13], UP_NEUTRAL).buttons,
            LEFT_SHOULDER | RIGHT_SHOULDER | VIEW | MENU | LEFT_STICK | RIGHT_STICK
        );
    }

    #[test]
    fn hat_directions_run_clockwise_from_up() {
        assert_eq!(pad(&[], 0).buttons, UP);
        assert_eq!(pad(&[], 1).buttons, UP | RIGHT);
        assert_eq!(pad(&[], 2).buttons, RIGHT);
        assert_eq!(pad(&[], 3).buttons, DOWN | RIGHT);
        assert_eq!(pad(&[], 4).buttons, DOWN);
        assert_eq!(pad(&[], 5).buttons, DOWN | LEFT);
        assert_eq!(pad(&[], 6).buttons, LEFT);
        assert_eq!(pad(&[], 7).buttons, UP | LEFT);
        assert_eq!(pad(&[], 8).buttons, 0);
    }

    #[test]
    fn sticks_are_centered_and_signed() {
        let r = state(CENTER, [0, 0], UP_NEUTRAL, &[]);
        assert!(r.left_x.abs() < 0.01 && r.left_y.abs() < 0.01);
        assert!(r.right_x.abs() < 0.01 && r.right_y.abs() < 0.01);
        let r = state([255, 0, 0, 255], [0, 0], UP_NEUTRAL, &[]);
        assert_eq!((r.left_x, r.left_y), (1.0, 1.0));
        assert_eq!((r.right_x, r.right_y), (-1.0, -1.0));
    }

    #[test]
    fn triggers_run_zero_to_one() {
        let r = state(CENTER, [255, 0], UP_NEUTRAL, &[]);
        assert_eq!((r.left_trigger, r.right_trigger), (1.0, 0.0));
    }

    #[test]
    fn usb_and_bluetooth_input_reports_are_accepted() {
        assert!(input_report(&[0x01, 0]));
        assert!(input_report(&[0x31, 0]));
        assert!(!input_report(&[0x02, 0]));
        assert!(!input_report(&[]));
    }

    /// Not part of the suite: prints what the HID scan sees on this machine.
    /// `cargo test -p victory-hle --lib dualsense -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lists_and_opens_hid_devices() {
        let paths = device_paths();
        println!("hid interfaces: {}", paths.len());
        for path in &paths {
            println!("  {}", String::from_utf16_lossy(path));
        }
        match open_pad() {
            Some(pad) => println!("opened a DualSense: report length {}", pad.input_len),
            None => println!("no DualSense opened"),
        }
    }
}
