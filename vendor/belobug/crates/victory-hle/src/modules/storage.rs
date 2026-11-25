//! Persistent local storage: a host folder of the title's own
//! (`runtime::Title::storage`, under the data directory, never the cache)
//! with the quota its manifest declares, seen by the title as a drive
//! (`files`).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::path::Path;

use super::xasync::{self, AsyncBlock, E_ABORT};
use super::xgameruntime::{
    Guid, HRESULT, Interface, Methods, NOT_SUPPORTED, Object, S_OK, add_ref, missing, put,
    put_string,
};
use crate::runtime;

pub const PROVIDER: Guid = Guid::new(
    0xf4fa_f4d4,
    0x2d04,
    0x4fce,
    [0xb3, 0xe0, 0x47, 0x4a, 0x71, 0x3a, 0x3e, 0x84],
);
pub const IID: Guid = Guid::new(
    0xd294_11df,
    0x0794,
    0x4553,
    [0x8b, 0x27, 0x95, 0xfc, 0x02, 0xd0, 0xf7, 0x5d],
);

/// As Grand Theft Auto VI (2026) asks for it; it calls 0x20 (path) and 0x28
/// (space) with the same arguments.
pub const IID_2026: Guid = Guid::new(
    0x41a4_e10c,
    0x5a7e,
    0x41d9,
    [0x8c, 0x37, 0x37, 0xbd, 0xe6, 0x2a, 0x07, 0xd6],
);

pub static OBJECT: Object = Object::new("persistent local storage", &VTABLE);
pub const INTERFACES: &[Interface] = &[(PROVIDER, IID, &OBJECT), (PROVIDER, IID_2026, &OBJECT)];

const E_FAIL: HRESULT = 0x8000_4005_u32 as i32;

type This = *const Object;

/// The path titles get: the root of a virtual drive over the folder. On the
/// console storage is a drive of its own, and titles rely on it: Minecraft
/// keeps only what precedes the path's last `:\` and mounts that as a
/// drive, so a host folder such as `C:\Users\..` would make it write to the
/// root of `C:`. None (the path calls fail) before the title is set.
fn folder() -> Option<String> {
    let storage = runtime::title().storage;
    if storage.as_os_str().is_empty() {
        return None;
    }
    #[cfg(windows)]
    return super::files::mount(&storage).map(|letter| format!("{letter}:\\"));
    #[cfg(not(windows))]
    None
}

extern "system" fn path_size(_: This, out: *mut usize) -> HRESULT {
    match folder() {
        Some(path) => put(out, path.len() + 1),
        None => NOT_SUPPORTED,
    }
}

/// A buffer too small for the path fails with `E_FAIL`, the documented
/// Xbox result (a PC gets `ERROR_INSUFFICIENT_BUFFER`).
extern "system" fn path(_: This, size: usize, buffer: *mut u8, used: *mut usize) -> HRESULT {
    match folder() {
        Some(path) if size <= path.len() => E_FAIL,
        Some(path) => put_string(&path, size, buffer, used),
        None => NOT_SUPPORTED,
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceInfo {
    /// Writable now.
    pub available_free: u64,
    /// Left in the quota.
    pub total_free: u64,
    pub used: u64,
    /// The quota.
    pub total: u64,
}

fn used_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => used_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// What the host disk under `dir` has free (unlimited if unknown).
pub(crate) fn disk_free(dir: &Path) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        let mut free = 0;
        let null = std::ptr::null_mut();
        // SAFETY: a NUL-terminated path and a valid output.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                null,
                null,
            )
        };
        if ok != 0 {
            return free;
        }
    }
    let _ = dir;
    u64::MAX
}

fn space() -> SpaceInfo {
    let title = runtime::title();
    let used = used_bytes(&title.storage);
    let free = title.storage_bytes.saturating_sub(used);
    SpaceInfo {
        // Writable now: the quota's rest, as far as the host disk allows.
        available_free: free.min(disk_free(&title.storage)),
        total_free: free,
        used,
        total: title.storage_bytes,
    }
}

extern "system" fn space_info(_: This, out: *mut SpaceInfo) -> HRESULT {
    put(out, space())
}

/// Tells our prompts apart from other calls on the same block.
static PROMPT: u8 = 0;

/// There is nobody to ask: granted when the quota has room, else declined.
extern "system" fn prompt_for_space(_: This, bytes: u64, block: *mut AsyncBlock) -> HRESULT {
    let hr = if bytes <= space().total_free {
        S_OK
    } else {
        E_ABORT
    };
    xasync::finished(block, (&raw const PROMPT).cast(), hr)
}

extern "system" fn prompt_result(_: This, block: *mut AsyncBlock) -> HRESULT {
    let none = std::ptr::null_mut();
    xasync::XAsyncGetResult(block, (&raw const PROMPT).cast(), 0, none, none.cast())
}

static VTABLE: Methods<8> = Methods([
    missing::<0> as *const (),
    add_ref as *const (),
    add_ref as *const (),
    path_size as *const (),        // 0x18
    path as *const (),             // 0x20
    space_info as *const (),       // 0x28
    prompt_for_space as *const (), // 0x30
    prompt_result as *const (),    // 0x38
]);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::xgameruntime::method;

    #[test]
    fn serves_a_drive_over_the_title_folder_and_its_quota() {
        type Size = extern "system" fn(This, *mut usize) -> HRESULT;
        type Get = extern "system" fn(This, usize, *mut u8, *mut usize) -> HRESULT;
        type Space = extern "system" fn(This, *mut SpaceInfo) -> HRESULT;
        type Prompt = extern "system" fn(This, u64, *mut AsyncBlock) -> HRESULT;
        type Result = extern "system" fn(This, *mut AsyncBlock) -> HRESULT;
        let this: This = &OBJECT;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("save.dat"), [0u8; 100]).unwrap();
        runtime::set_title(runtime::Title {
            id: 1,
            storage: dir.path().to_owned(),
            storage_bytes: 1000,
            ..Default::default()
        });

        // The path is a drive root, never the host folder.
        let mut size = 0;
        assert_eq!(method::<Size>(&OBJECT, 0x18)(this, &mut size), S_OK);
        assert_eq!(size, 4, "X:\\ and its NUL");
        let (mut buffer, mut used) = ([0u8; 4], 0);
        let get = method::<Get>(&OBJECT, 0x20);
        assert_eq!(get(this, 4, buffer.as_mut_ptr(), &mut used), S_OK);
        assert_eq!(get(this, 3, buffer.as_mut_ptr(), &mut used), E_FAIL);
        let drive = std::str::from_utf8(&buffer[..3]).unwrap();
        assert!(drive.ends_with(":\\"), "{drive}");
        // Title file calls on it reach the folder.
        let path = format!("{drive}save.dat\0");
        let mut attributes = [0u8; 36];
        let get_attributes = super::super::files::GetFileAttributesExW;
        let wide: Vec<u16> = path.encode_utf16().collect();
        assert_ne!(
            get_attributes(wide.as_ptr(), 0, attributes.as_mut_ptr().cast()),
            0
        );

        let mut info = SpaceInfo {
            available_free: 0,
            total_free: 0,
            used: 0,
            total: 0,
        };
        assert_eq!(method::<Space>(&OBJECT, 0x28)(this, &mut info), S_OK);
        assert_eq!((info.used, info.total_free, info.total), (100, 900, 1000));

        let prompt = method::<Prompt>(&OBJECT, 0x30);
        let result = method::<Result>(&OBJECT, 0x38);
        for (bytes, expect) in [(900, S_OK), (901, E_ABORT)] {
            let mut block = AsyncBlock {
                queue: std::ptr::null(),
                context: std::ptr::null_mut(),
                callback: None,
                internal: [0; 4],
            };
            assert_eq!(prompt(this, bytes, &mut block), S_OK);
            assert_eq!(xasync::XAsyncGetStatus(&mut block, true), expect);
            assert_eq!(result(this, &mut block), expect, "{bytes} bytes");
        }
    }
}
