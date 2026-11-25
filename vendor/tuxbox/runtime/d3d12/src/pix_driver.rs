//! Prefix of the Series PIX device callback table used for capture setup.
use std::sync::Mutex;
use windows::Win32::Foundation::{
    CloseHandle, E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, ERROR_INVALID_HANDLE, GENERIC_READ,
    GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, S_OK,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::{HRESULT, w};

static PIX_HANDLE: Mutex<Option<usize>> = Mutex::new(None);

fn control(handle: HANDLE, code: u32, words: &[u32]) -> HRESULT {
    let mut returned = 0;
    let input = (!words.is_empty()).then(|| words.as_ptr().cast());
    // SAFETY: The session lock keeps the handle open through this synchronous call.
    match unsafe {
        DeviceIoControl(
            handle,
            code,
            input,
            size_of_val(words) as u32,
            None,
            0,
            Some(&mut returned),
            None,
        )
    } {
        Ok(()) if returned == 0 => S_OK,
        Ok(()) => E_FAIL,
        Err(error) => error.code(),
    }
}

unsafe extern "system" fn open(out: *mut HANDLE) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let mut session = PIX_HANDLE.lock().unwrap_or_else(|e| e.into_inner());
    let handle = if let Some(raw) = *session {
        HANDLE(raw as _)
    } else {
        let handle = match unsafe {
            CreateFileW(
                w!(r"\\.\XgsKmd10"),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        } {
            Ok(handle) => handle,
            Err(error) => return error.code(),
        };
        let status = control(handle, 0x7900_c190, &[]);
        if status.is_err() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return status;
        }
        *session = Some(handle.0 as usize);
        handle
    };
    // SAFETY: The caller supplies writable handle storage.
    unsafe {
        out.write_unaligned(handle);
    }
    S_OK
}

extern "system" fn close(handle: HANDLE) -> HRESULT {
    let mut session = PIX_HANDLE.lock().unwrap_or_else(|e| e.into_inner());
    if *session == Some(handle.0 as usize) {
        let _ = control(handle, 0x7900_c194, &[]);
        unsafe {
            let _ = CloseHandle(handle);
        }
        *session = None;
    }
    S_OK
}

extern "system" fn command(handle: HANDLE, first: u32, second: u32) -> HRESULT {
    if handle == INVALID_HANDLE_VALUE {
        return E_INVALIDARG;
    }
    let session = PIX_HANDLE.lock().unwrap_or_else(|e| e.into_inner());
    if *session != Some(handle.0 as usize) {
        return HRESULT::from_win32(ERROR_INVALID_HANDLE.0);
    }
    control(handle, 0x7900_c19c, &[first, second])
}

/// The caller supplies a record with a readable size DWORD, writable up to
/// the requested byte count. Old callers may request part of a pointer.
#[unsafe(export_name = "PixGetPixieEntryPoints")]
pub unsafe extern "system" fn get_entry_points(table: *mut u8) -> HRESULT {
    if table.is_null() {
        return E_POINTER;
    }
    let size = unsafe { table.cast::<u32>().read_unaligned() };
    let prefix = [
        size as usize,
        open as *const () as usize,
        close as *const () as usize,
        command as *const () as usize,
    ];
    if size as usize > size_of_val(&prefix) {
        return E_NOTIMPL;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(prefix.as_ptr().cast(), table, size as usize);
    }
    S_OK
}
