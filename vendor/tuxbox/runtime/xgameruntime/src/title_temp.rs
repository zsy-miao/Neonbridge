//! Attach the launcher's scratch directory (T:) and package root (G:) after Wine
//! initializes its drive mappings.
use std::{fs::OpenOptions, io, os::windows::io::AsRawHandle, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, GetLastError},
    Storage::FileSystem::QueryDosDeviceW,
    System::{
        Diagnostics::Debug::OutputDebugStringA,
        IO::DeviceIoControl,
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        WindowsProgramming::DRIVE_FIXED,
    },
};
use windows_sys::core::w;

// Wine's mountmgr_unix_drive from ddk/mountmgr.h, followed by the Unix mount path.
#[repr(C)]
struct Drive {
    size: u32,
    drive_type: u32,
    fs_type: u32,
    serial: u32,
    unix_dev: u64,
    letter: u16,
    mount_point_offset: u16,
    device_offset: u16,
    label_offset: u16,
    path: [u8; 4096],
}

fn mount(letter: u8, path: &str, replace: bool) -> io::Result<()> {
    let mut name = [0u16; 256];
    let device = [u16::from(letter), u16::from(b':'), 0];
    // Even an inaccessible assigned device owns the letter. Unless asked to
    // replace it, only attach on a confirmed missing device and leave existing
    // drive-directory aliases alone.
    if !replace {
        unsafe {
            if QueryDosDeviceW(device.as_ptr(), name.as_mut_ptr(), name.len() as u32) != 0
                || GetLastError() != ERROR_FILE_NOT_FOUND
                || std::path::Path::new(&format!("{}:\\", char::from(letter))).is_dir()
            {
                return Ok(());
            }
        }
    }
    if !path.starts_with('/') || path.len() >= 4096 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let offset = std::mem::offset_of!(Drive, path);
    let mut drive = Drive {
        size: (offset + path.len() + 1) as u32,
        drive_type: DRIVE_FIXED,
        fs_type: 0,
        serial: 0,
        unix_dev: 0,
        letter: u16::from(letter),
        mount_point_offset: offset as u16,
        device_offset: 0,
        label_offset: 0,
        path: [0; 4096],
    };
    drive.path[..path.len()].copy_from_slice(path.as_bytes());
    let manager = OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\MountPointManager")?;
    const IOCTL_MOUNTMGR_DEFINE_UNIX_DRIVE: u32 = 0x006d_c080;
    // The input contains its terminating NUL and every offset is relative to its header.
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            manager.as_raw_handle(),
            IOCTL_MOUNTMGR_DEFINE_UNIX_DRIVE,
            ptr::from_ref(&drive).cast(),
            drive.size,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn attach() {
    let ntdll = unsafe { GetModuleHandleW(w!("ntdll.dll")) };
    if ntdll.is_null()
        || unsafe { GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()) }.is_none()
    {
        return;
    }
    // Titles read their package through G:, as on the console. The launcher records
    // the prefix's G: before the launch and restores it after the title exits, so
    // the launched package replaces any G: here.
    for (variable, letter, replace) in [
        ("TUXBOX_WINE_TITLE_TEMP", b'T', false),
        ("TUXBOX_WINE_PACKAGE_DRIVE", b'G', true),
    ] {
        let Ok(path) = std::env::var(variable) else {
            continue;
        };
        if let Err(error) = mount(letter, &path, replace) {
            let message = format!("TuxBox xgameruntime: could not attach {}: ({error})\n\0", char::from(letter));
            unsafe { OutputDebugStringA(message.as_ptr()) };
        }
    }
}
