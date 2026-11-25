//! XSystem: the console's identity and sandbox, and how analytics describe
//! the device. Fixed values: one offline retail console, the model the user
//! chose ([`victory_gpu::console`]).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::xgameruntime::{
    Guid, HRESULT, Interface, Methods, Object, add_ref, missing, put_string,
};

pub const PROVIDER: Guid = Guid::new(
    0xe349_bd1a,
    0xfc20,
    0x4e40,
    [0xb9, 0x9c, 0x41, 0x78, 0xcc, 0x6b, 0x40, 0x9f],
);

/// The interface as Minecraft (2022) asks for it; an older table
/// ended before the device id.
pub const IID: Guid = Guid::new(
    0x6fd7_1f09,
    0x7513,
    0x49f0,
    [0x89, 0xbc, 0xbf, 0xaf, 0x5d, 0xf6, 0xf8, 0x52],
);

/// Analytics information; its interface id is the provider's.
pub const ANALYTICS: Guid = Guid::new(
    0xb884_675d,
    0xb738,
    0x4a9c,
    [0x81, 0x5d, 0x9a, 0x9a, 0x1e, 0x0c, 0x6c, 0x9b],
);

/// 38 characters (39 bytes with the NUL).
const CONSOLE_ID: &str = "00000000000000000000000000000000000001";
/// The retail sandbox.
const SANDBOX: &str = "RETAIL";
/// Base64 of 32 bytes: 44 characters (45 bytes with the NUL).
const DEVICE_ID: &str = "VkljdG9yeSBvZmZsaW5lIGRldmljZSBpZGVudGl0eSE=";

pub static OBJECT: Object = Object::new("system", &VTABLE);
pub static ANALYTICS_OBJECT: Object = Object::new("analytics", &ANALYTICS_VTABLE);

/// The interface as the title (2026) asks for it; it calls the
/// same three slots (0x18, 0x20, 0x28) with the same arguments.
pub const IID_2026: Guid = Guid::new(
    0x1861_cf2e,
    0xe18b,
    0x4834,
    [0xa9, 0xf5, 0xb4, 0xa4, 0xe6, 0xef, 0xb4, 0xcf],
);

pub const INTERFACES: &[Interface] = &[
    (PROVIDER, IID, &OBJECT),
    (PROVIDER, IID_2026, &OBJECT),
    (ANALYTICS, ANALYTICS, &ANALYTICS_OBJECT),
];

type This = *const Object;

extern "system" fn console_id(_: This, size: usize, buffer: *mut u8, used: *mut usize) -> HRESULT {
    put_string(CONSOLE_ID, size, buffer, used)
}
extern "system" fn sandbox_id(_: This, size: usize, buffer: *mut u8, used: *mut usize) -> HRESULT {
    put_string(SANDBOX, size, buffer, used)
}
/// The device id this title sees (titles see different ones).
extern "system" fn device_id(_: This, size: usize, buffer: *mut u8, used: *mut usize) -> HRESULT {
    put_string(DEVICE_ID, size, buffer, used)
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
}

#[repr(C)]
pub struct AnalyticsInfo {
    pub os: Version,
    pub hosting_os: Version,
    /// NUL-terminated.
    pub family: [u8; 64],
    pub form: [u8; 64],
}

const OS: Version = Version {
    major: 10,
    minor: 0,
    build: 22000,
    revision: 0,
};

fn text(s: &str) -> [u8; 64] {
    let mut out = [0; 64];
    out[..s.len()].copy_from_slice(s.as_bytes());
    out
}

/// A struct this large comes back through the caller's buffer, whose
/// address is also the return value.
extern "system" fn analytics_info(_: This, out: *mut AnalyticsInfo) -> *mut AnalyticsInfo {
    let info = AnalyticsInfo {
        os: OS,
        hosting_os: OS,
        family: text("Xbox"),
        form: text(victory_gpu::console::current().name()),
    };
    // SAFETY: the caller passes room for the struct.
    unsafe { out.write(info) };
    out
}

static VTABLE: Methods<6> = Methods([
    missing::<0> as *const (),
    add_ref as *const (),
    add_ref as *const (),
    console_id as *const (), // 0x18, by the older table; titles seen do not call it
    sandbox_id as *const (), // 0x20
    device_id as *const (),  // 0x28
]);

static ANALYTICS_VTABLE: Methods<4> = Methods([
    missing::<0> as *const (),
    add_ref as *const (),
    add_ref as *const (),
    analytics_info as *const (), // 0x18
]);

/// The device type is not a runtime call: code linked into the title reads
/// `ConsoleMode` from this key (`RegOpenKeyW`, `RegQueryValueExW`) and
/// takes a missing value for a PC.
#[cfg(windows)]
mod device_type {
    #![allow(non_snake_case)]

    use std::ffi::c_void;

    use victory_gpu::console::{self, Console};

    use crate::modules::files::host;
    use crate::{HleModule, hle_export};

    type Key = *mut c_void;
    const LOCAL_MACHINE: Key = 0xffff_ffff_8000_0002_u64 as Key;
    const KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\OEM";
    const VALUE: &str = "ConsoleMode";
    const SUCCESS: u32 = 0;
    const FILE_NOT_FOUND: u32 = 2;
    const INVALID_PARAMETER: u32 = 87;
    const MORE_DATA: u32 = 234;
    const DWORD: u32 = 4;

    /// Stands for the key; never a host handle value.
    static OEM: u8 = 0;
    fn oem() -> Key {
        (&raw const OEM).cast_mut().cast()
    }

    /// What the title's code maps to each model's device type.
    pub(super) fn console_mode(console: Console) -> u32 {
        match console {
            Console::SeriesX => 0x64,
            Console::SeriesS => 0x60,
        }
    }

    /// The value the title reads. `VICTORY_RENDER_S=1`: a Series S's on any
    /// console. The title picks its render path by this value alone (a
    /// 1920x1080 game plane under a 3840x2160 UI plane) and its content and
    /// memory by the rest of the profile, so a Series X install renders a
    /// quarter of the pixels (measured on Neptune's other runtime: the
    /// registry value and the analytics form are all the title asks).
    fn reported_mode() -> u32 {
        static RENDER_S: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| std::env::var_os("VICTORY_RENDER_S").is_some_and(|v| v != "0"));
        if *RENDER_S {
            console_mode(Console::SeriesS)
        } else {
            console_mode(console::current())
        }
    }

    fn is(text: *const u16, expected: &str) -> bool {
        if text.is_null() {
            return false;
        }
        // SAFETY: the title passes a NUL-terminated string.
        let text = unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
        };
        text.eq_ignore_ascii_case(expected)
    }

    /// Opens our key for its path; the key has no subkeys. `None`: not
    /// ours, ask the host.
    fn open(root: Key, sub: *const u16, out: *mut Key) -> Option<u32> {
        if root == oem() {
            return Some(FILE_NOT_FOUND);
        }
        if root == LOCAL_MACHINE && is(sub, KEY) && !out.is_null() {
            // SAFETY: checked non-null; the title passes room for a key.
            unsafe { out.write(oem()) };
            return Some(SUCCESS);
        }
        None
    }

    // Our titles import only these registry functions (and `RegOpenKeyExW`
    // below), so no other call can reach the host with our key.
    pub extern "system" fn RegOpenKeyW(root: Key, sub: *const u16, out: *mut Key) -> u32 {
        open(root, sub, out).unwrap_or_else(|| {
            host!("advapi32.dll", "RegOpenKeyW": extern "system" fn(Key, *const u16, *mut Key) -> u32)(
                root, sub, out,
            )
        })
    }

    pub extern "system" fn RegOpenKeyExW(
        root: Key,
        sub: *const u16,
        options: u32,
        access: u32,
        out: *mut Key,
    ) -> u32 {
        open(root, sub, out).unwrap_or_else(|| {
            host!("advapi32.dll", "RegOpenKeyExW": extern "system" fn(
                Key, *const u16, u32, u32, *mut Key,
            ) -> u32)(root, sub, options, access, out)
        })
    }

    pub extern "system" fn RegQueryValueExW(
        key: Key,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> u32 {
        if key != oem() {
            return host!("advapi32.dll", "RegQueryValueExW": extern "system" fn(
                Key, *const u16, *mut u32, *mut u32, *mut u8, *mut u32,
            ) -> u32)(key, name, reserved, kind, data, size);
        }
        if !is(name, VALUE) {
            return FILE_NOT_FOUND;
        }
        if !data.is_null() && size.is_null() {
            return INVALID_PARAMETER;
        }
        // SAFETY: the title passes valid or null pointers, sized by `size`.
        unsafe {
            if !kind.is_null() {
                kind.write(DWORD);
            }
            if size.is_null() {
                return SUCCESS;
            }
            let room = size.read();
            size.write(4);
            if data.is_null() {
                return SUCCESS;
            }
            if room < 4 {
                return MORE_DATA;
            }
            data.cast::<u32>().write_unaligned(reported_mode());
        }
        tracing::debug!(console = console::current().name(), "device type read");
        SUCCESS
    }

    pub extern "system" fn RegCloseKey(key: Key) -> u32 {
        if key == oem() {
            return SUCCESS;
        }
        host!("advapi32.dll", "RegCloseKey": extern "system" fn(Key) -> u32)(key)
    }

    pub(crate) fn module() -> HleModule {
        HleModule {
            name: "device type",
            dlls: &["advapi32.dll", "api-ms-win-core-registry-*"],
            exports: vec![
                hle_export!(RegOpenKeyW),
                hle_export!(RegOpenKeyExW),
                hle_export!(RegQueryValueExW),
                hle_export!(RegCloseKey),
            ],
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn w(s: &str) -> Vec<u16> {
            s.encode_utf16().chain([0]).collect()
        }

        #[test]
        fn titles_read_the_chosen_model() {
            assert_eq!(console_mode(Console::SeriesX), 0x64);
            assert_eq!(console_mode(Console::SeriesS), 0x60);
            let mut key = std::ptr::null_mut();
            let path = w(&KEY.to_ascii_lowercase());
            assert_eq!(RegOpenKeyW(LOCAL_MACHINE, path.as_ptr(), &mut key), SUCCESS);
            let (mut kind, mut value, mut size) = (0, 0u32, 4);
            let name = w(VALUE);
            let query = |data: *mut u32, size: *mut u32, kind: *mut u32| {
                RegQueryValueExW(
                    key,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    kind,
                    data.cast(),
                    size,
                )
            };
            assert_eq!(query(&mut value, &mut size, &mut kind), SUCCESS);
            assert_eq!((kind, size), (DWORD, 4));
            assert_eq!(value, console_mode(console::current()));
            let mut small = 2;
            assert_eq!(query(&mut value, &mut small, &mut kind), MORE_DATA);
            assert_eq!(small, 4);
            let other = w("Other");
            assert_eq!(
                RegQueryValueExW(
                    key,
                    other.as_ptr(),
                    std::ptr::null_mut(),
                    &mut kind,
                    std::ptr::null_mut(),
                    &mut size
                ),
                FILE_NOT_FOUND
            );
            let mut sub = std::ptr::null_mut();
            assert_eq!(
                RegOpenKeyExW(key, other.as_ptr(), 0, 1, &mut sub),
                FILE_NOT_FOUND
            );
            assert_eq!(RegCloseKey(key), SUCCESS);
            let mut again = std::ptr::null_mut();
            assert_eq!(
                RegOpenKeyExW(LOCAL_MACHINE, path.as_ptr(), 0, 1, &mut again),
                SUCCESS
            );
            assert_eq!(again, oem());
        }

        #[test]
        fn other_keys_reach_the_host() {
            let mut key = std::ptr::null_mut();
            let path = w(r"Software\Microsoft\Windows NT\CurrentVersion");
            assert_eq!(RegOpenKeyW(LOCAL_MACHINE, path.as_ptr(), &mut key), SUCCESS);
            assert_ne!(key, oem());
            let (mut size, name) = (0, w("ProductName"));
            assert_eq!(
                RegQueryValueExW(
                    key,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut size
                ),
                SUCCESS
            );
            assert!(size > 0);
            assert_eq!(RegCloseKey(key), SUCCESS);
        }
    }
}
#[cfg(windows)]
pub(crate) use device_type::module;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::xgameruntime::{S_OK, method};

    #[test]
    fn identity_strings_fit_the_documented_sizes() {
        type Get = extern "system" fn(This, usize, *mut u8, *mut usize) -> HRESULT;
        let this: This = &OBJECT;
        let mut buffer = [0u8; 64];
        let mut used = 0;
        for (slot, size) in [(0x18, 39), (0x20, 16), (0x28, 45)] {
            let get = method::<Get>(&OBJECT, slot);
            assert_eq!(get(this, size, buffer.as_mut_ptr(), &mut used), S_OK);
            assert!(used <= size && buffer[used - 1] == 0, "slot {slot:#x}");
        }
        assert_eq!(&buffer[..7], b"VkljdG9");
    }

    #[test]
    fn analytics_info_comes_back_through_the_buffer() {
        type Get = extern "system" fn(This, *mut AnalyticsInfo) -> *mut AnalyticsInfo;
        let mut info = std::mem::MaybeUninit::<AnalyticsInfo>::uninit();
        let get = method::<Get>(&ANALYTICS_OBJECT, 0x18);
        let back = get(&ANALYTICS_OBJECT, info.as_mut_ptr());
        assert_eq!(back, info.as_mut_ptr());
        // SAFETY: written by the call.
        let info = unsafe { info.assume_init() };
        assert_eq!(size_of::<AnalyticsInfo>(), 0x90);
        assert_eq!(info.os.build, 22000);
        assert_eq!(&info.form[..14], b"Xbox Series X\0");
    }
}
