//! Section-backed CPU views for the Xbox 64 KiB user-physical aperture.
use core::{ffi::c_void, ptr};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{ERROR_INVALID_ADDRESS, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, MEM_COMMIT, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE,
    MEM_REPLACE_PLACEHOLDER, MEM_RESERVE, MEM_RESERVE_PLACEHOLDER, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, MapViewOfFile3, PAGE_NOACCESS, PAGE_READWRITE,
    SEC_RESERVE, UnmapViewOfFile, UnmapViewOfFile2, VirtualAlloc, VirtualAlloc2, VirtualFree,
    VirtualQuery,
};
use windows_sys::Win32::System::SystemServices::MEM_COALESCE_PLACEHOLDERS;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub(super) const PAGE: usize = 65536;

pub(super) fn reserve(address: *const c_void, bytes: usize) -> *mut c_void {
    unsafe {
        VirtualAlloc2(
            GetCurrentProcess(),
            address,
            bytes,
            MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
            PAGE_NOACCESS,
            ptr::null_mut(),
            0,
        )
    }
}

// Check that the entire range is an isolated placeholder. Keep a placeholder
// as its portable AWE backing so individual pages can later be section views.
pub(super) fn replace_aperture(address: *mut c_void, bytes: usize) -> *mut c_void {
    let base = unsafe {
        VirtualAlloc2(GetCurrentProcess(), address, bytes, MEM_RESERVE | MEM_REPLACE_PLACEHOLDER,
            PAGE_NOACCESS, ptr::null_mut(), 0)
    };
    if !base.is_null() && unsafe { VirtualFree(base, bytes, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) } == 0 {
        return ptr::null_mut();
    }
    base
}

// One demand-committed pagefile section per batch of XMem physical tokens.
pub(super) struct Section {
    bytes: u64,
    handle: OnceLock<OwnedHandle>,
}

impl Section {
    pub(super) fn new(count: usize) -> Self {
        Self {
            bytes: count as u64 * PAGE as u64,
            handle: OnceLock::new(),
        }
    }

    fn handle(&self) -> Result<*mut c_void, u32> {
        if let Some(h) = self.handle.get() {
            return Ok(h.as_raw_handle());
        }
        let h = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE | SEC_RESERVE,
                (self.bytes >> 32) as u32,
                self.bytes as u32,
                ptr::null(),
            )
        };
        if h.is_null() {
            return Err(unsafe { GetLastError() });
        }
        self.handle
            .set(unsafe { OwnedHandle::from_raw_handle(h) })
            .ok();
        Ok(self.handle.get().unwrap().as_raw_handle())
    }

    pub(super) fn map(&self, offset: u64, address: usize) -> Result<(), u32> {
        self.map_range(offset, address, PAGE)
    }

    pub(super) fn map_range(&self, offset: u64, address: usize, bytes: usize) -> Result<(), u32> {
        let view = unsafe {
            MapViewOfFile3(
                self.handle()?,
                GetCurrentProcess(),
                address as _,
                offset,
                bytes,
                MEM_REPLACE_PLACEHOLDER,
                PAGE_READWRITE,
                ptr::null_mut(),
                0,
            )
        };
        if view.Value.is_null() {
            return Err(unsafe { GetLastError() });
        }
        if unsafe { VirtualAlloc(view.Value, bytes, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
            let err = unsafe { GetLastError() };
            unmap(address)?;
            return Err(err);
        }
        Ok(())
    }

    pub(super) fn read_into(&self, offset: u64, dest: *mut u8) -> Result<(), u32> {
        let view = unsafe {
            MapViewOfFile(
                self.handle()?,
                FILE_MAP_READ,
                (offset >> 32) as u32,
                offset as u32,
                PAGE,
            )
        };
        if view.Value.is_null() {
            return Err(unsafe { GetLastError() });
        }
        unsafe {
            ptr::copy_nonoverlapping(view.Value.cast(), dest, PAGE);
            UnmapViewOfFile(view);
        }
        Ok(())
    }
}

pub(super) fn unmap(address: usize) -> Result<(), u32> {
    let view = MEMORY_MAPPED_VIEW_ADDRESS {
        Value: address as _,
    };
    if unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, MEM_PRESERVE_PLACEHOLDER) } == 0 {
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}

pub(super) fn region(address: usize) -> Result<MEMORY_BASIC_INFORMATION, u32> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    if unsafe {
        VirtualQuery(
            address as _,
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    } == 0
    {
        return Err(unsafe { GetLastError() });
    }
    Ok(info)
}

pub(super) fn isolate_page(address: usize, info: &MEMORY_BASIC_INFORMATION) -> Result<(), u32> {
    isolate_range(address, PAGE, info)
}

pub(super) fn isolate_range(address: usize, bytes: usize, info: &MEMORY_BASIC_INFORMATION) -> Result<(), u32> {
    if info.State != MEM_RESERVE
        || info.AllocationProtect != PAGE_NOACCESS
        || address < info.BaseAddress.addr()
        || address + bytes > info.BaseAddress.addr() + info.RegionSize
    {
        return Err(ERROR_INVALID_ADDRESS);
    }
    if (info.AllocationBase.addr() != address || info.RegionSize != bytes)
        && unsafe { VirtualFree(address as _, bytes, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) } == 0
    {
        return Err(unsafe { GetLastError() });
    }
    Ok(())
}

pub(super) fn coalesce(base: usize, bytes: usize) -> Result<(), u32> {
    if region(base)?.RegionSize == bytes {
        return Ok(());
    }
    if unsafe { VirtualFree(base as _, bytes, MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS) } == 0 {
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}
