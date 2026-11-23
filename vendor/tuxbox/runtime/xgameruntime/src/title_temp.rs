//! Attach the launcher's scratch directory after Wine initializes its drive mappings.
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

fn mount(path: &str) -> io::Result<()> {
    let mut name = [0u16; 256];
    // Even an inaccessible assigned device owns the letter. Only attach on a
    // confirmed missing device and leave existing drive-directory aliases alone.
    unsafe {
        if QueryDosDeviceW(w!("T:"), name.as_mut_ptr(), name.len() as u32) != 0
            || GetLastError() != ERROR_FILE_NOT_FOUND
            || std::path::Path::new(r"T:\").is_dir()
        {
            return Ok(());
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
        letter: u16::from(b'T'),
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
    let Ok(path) = std::env::var("TUXBOX_WINE_TITLE_TEMP") else {
        return;
    };
    let ntdll = unsafe { GetModuleHandleW(w!("ntdll.dll")) };
    if ntdll.is_null()
        || unsafe { GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()) }.is_none()
    {
        return;
    }
    if let Err(error) = mount(&path) {
        let message = format!("TuxBox xgameruntime: could not attach T: ({error})\n\0");
        unsafe { OutputDebugStringA(message.as_ptr()) };
    }
}
