//! Flamingo: a DualSense (the PlayStation 5 controller, and the DualSense
//! Edge) as the title's first gamepad when no XInput pad is connected. It is
//! no XInput device, so without Steam Input or a similar tool the title saw
//! nothing. Read over HID, by USB or Bluetooth, on a thread of its own; the
//! last report is what [`latest`] gives. Buttons go to the Xbox pad's by
//! position (cross A, circle B, square X, triangle Y; options menu; create
//! and the touch pad's click view). `FLAMINGO_DUALSENSE=0` leaves it out.
//!
//! Reports (the controller's own layout): USB sends report 0x01, 64 bytes;
//! Bluetooth sends a short 0x01 until feature report 0x05 is read, then
//! report 0x31 with the USB fields one byte later. In the full ones, from
//! the field start: left stick x, y, right stick x, y, left trigger, right
//! trigger, a counter, then three button bytes (hat in the low nibble,
//! square cross circle triangle; L1 R1 L2 R2 create options L3 R3; PS,
//! touch pad, mute). The short one: sticks at 1-4, buttons at 5-7,
//! triggers at 8 and 9.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, RIGHT, RIGHT_SHOULDER, RIGHT_STICK, UP, VIEW, X, Y};

/// One report as the title's pad sees it: buttons in the title's bits,
/// triggers 0..1, sticks -1..1 (up and right positive).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Report {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
}

/// Sony's vendor id, and the DualSense and the DualSense Edge.
const VENDOR: u16 = 0x054C;
const PRODUCTS: [u16; 2] = [0x0CE6, 0x0DF2];
/// A report older than this means the controller went away.
const STALE: Duration = Duration::from_secs(2);

static LAST: Mutex<Option<(Report, Instant)>> = Mutex::new(None);

/// The last report, if one came lately. Starts the reader on first use.
pub(super) fn latest() -> Option<Report> {
    static START: std::sync::Once = std::sync::Once::new();
    if std::env::var_os("FLAMINGO_DUALSENSE").is_some_and(|v| v == "0") {
        return None;
    }
    START.call_once(|| {
        let _ = std::thread::Builder::new().name("flamingo-dualsense".into()).spawn(read_forever);
    });
    let last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    last.and_then(|(r, at)| (at.elapsed() < STALE).then_some(r))
}

/// A report's fields: `None` for a report that is not an input report of
/// the controller's (`usb`: it came over USB, whose 0x01 is the full one).
pub(super) fn parse(b: &[u8], usb: bool) -> Option<Report> {
    let (sticks, triggers, buttons) = match *b.first()? {
        0x01 if usb && b.len() >= 11 => (1, 5, 8),
        0x31 if b.len() >= 12 => (2, 6, 9),
        // Bluetooth before the switch to full reports.
        0x01 if b.len() >= 10 => (1, 8, 5),
        _ => return None,
    };
    let axis = |v: u8| ((f32::from(v) - 128.0) / 127.0).clamp(-1.0, 1.0);
    let (b0, b1, b2) = (b[buttons], b[buttons + 1], b[buttons + 2]);
    let mut bits = 0;
    // The hat: 0 up, then clockwise in eighths; 8 none.
    let hat = b0 & 0x0F;
    if matches!(hat, 7 | 0 | 1) {
        bits |= UP;
    }
    if matches!(hat, 1..=3) {
        bits |= RIGHT;
    }
    if matches!(hat, 3..=5) {
        bits |= DOWN;
    }
    if matches!(hat, 5..=7) {
        bits |= LEFT;
    }
    for (on, bit) in [
        (b0 & 0x10, X),
        (b0 & 0x20, A),
        (b0 & 0x40, B),
        (b0 & 0x80, Y),
        (b1 & 0x01, LEFT_SHOULDER),
        (b1 & 0x02, RIGHT_SHOULDER),
        (b1 & 0x10, VIEW),
        (b1 & 0x20, MENU),
        (b1 & 0x40, LEFT_STICK),
        (b1 & 0x80, RIGHT_STICK),
        (b2 & 0x02, VIEW),
    ] {
        if on != 0 {
            bits |= bit;
        }
    }
    Some(Report {
        buttons: bits,
        left_trigger: f32::from(b[triggers]) / 255.0,
        right_trigger: f32::from(b[triggers + 1]) / 255.0,
        left_x: axis(b[sticks]),
        left_y: -axis(b[sticks + 1]),
        right_x: axis(b[sticks + 2]),
        right_y: -axis(b[sticks + 3]),
    })
}

/// Finds a controller, reads it until it goes, and looks again.
fn read_forever() {
    loop {
        match open() {
            Some(handle) => {
                read(handle);
                // SAFETY: our handle.
                unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
                *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            None => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// The first DualSense among the HID devices, opened; switched to full
/// reports (reading feature report 0x05 does that over Bluetooth).
fn open() -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid};
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    // SAFETY: plain device queries with buffers of the sizes passed; every
    // handle and list is closed or returned.
    unsafe {
        let mut guid = std::mem::zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(&guid, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if set == -1 {
            return None;
        }
        let mut found = None;
        for index in 0.. {
            let mut data: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &guid, index, &mut data) == 0 {
                break;
            }
            // The detail: a size, then the path; in u32s for its alignment.
            let mut detail = [0u32; 256];
            let d = detail.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*d).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            let size = size_of_val(&detail) as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &data, d, size, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                continue;
            }
            let path = (&raw const (*d).DevicePath).cast::<u16>();
            let handle = CreateFileW(
                path,
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut attributes: HIDD_ATTRIBUTES = std::mem::zeroed();
            attributes.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
            if HidD_GetAttributes(handle, &mut attributes)
                && attributes.VendorID == VENDOR
                && PRODUCTS.contains(&attributes.ProductID)
            {
                let mut feature = [0u8; 64];
                feature[0] = 0x05;
                let _ = HidD_GetFeature(handle, feature.as_mut_ptr().cast(), feature.len() as u32);
                tracing::info!(product = format_args!("{:#06x}", attributes.ProductID), "DualSense found: it is the title's first pad while no XInput pad is connected");
                found = Some(handle);
                break;
            }
            CloseHandle(handle);
        }
        SetupDiDestroyDeviceInfoList(set);
        found
    }
}

/// Reads reports until the controller goes away.
fn read(handle: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    let mut buffer = [0u8; 128];
    loop {
        let mut n = 0u32;
        // SAFETY: our handle and a buffer of the length passed.
        let ok = unsafe { ReadFile(handle, buffer.as_mut_ptr(), buffer.len() as u32, &mut n, std::ptr::null_mut()) };
        if ok == 0 || n == 0 {
            return;
        }
        // USB reports are 64 bytes; Bluetooth ones longer.
        if let Some(report) = parse(&buffer[..n as usize], n <= 64) {
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((report, Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A USB report with the left stick up and right, the right stick at
    /// rest, the left trigger full, cross and the hat's up-right held,
    /// options and L1 held, the touch pad clicked.
    #[test]
    fn a_usb_report() {
        let mut b = [0u8; 64];
        b[0] = 0x01;
        b[1..5].copy_from_slice(&[255, 0, 128, 128]);
        b[5] = 255;
        b[8] = 0x20 | 1;
        b[9] = 0x20 | 0x01;
        b[10] = 0x02;
        let r = parse(&b, true).unwrap();
        assert_eq!(r.buttons, A | UP | RIGHT | MENU | LEFT_SHOULDER | VIEW);
        assert_eq!((r.left_x, r.left_y), (1.0, 1.0));
        assert!(r.right_x.abs() < 0.01 && r.right_y.abs() < 0.01);
        assert_eq!((r.left_trigger, r.right_trigger), (1.0, 0.0));
    }

    /// Bluetooth: the full report one byte later, and the short one.
    #[test]
    fn bluetooth_reports() {
        let mut b = [0u8; 78];
        b[0] = 0x31;
        b[2..6].copy_from_slice(&[0, 255, 128, 128]);
        b[7] = 128;
        b[9] = 0x80 | 8;
        let r = parse(&b, false).unwrap();
        assert_eq!(r.buttons, Y);
        // x at 0 is full left; y at 255 is full down.
        assert_eq!((r.left_x, r.left_y), (-1.0, -1.0));
        let mut s = [0u8; 78];
        s[0] = 0x01;
        s[1..5].copy_from_slice(&[128, 128, 128, 128]);
        s[5] = 0x40 | 8;
        s[9] = 255;
        let r = parse(&s, false).unwrap();
        assert_eq!(r.buttons, B);
        assert_eq!(r.right_trigger, 1.0);
        assert_eq!(parse(&[0x02, 0, 0], true), None);
    }
}
