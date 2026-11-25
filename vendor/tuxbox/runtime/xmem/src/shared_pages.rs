//! Section-backed CPU views for the Xbox 64 KiB user-physical aperture.
use core::{ffi::c_void, ptr};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{ERROR_INVALID_ADDRESS, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEM_COMMIT, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE,
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

// One pagefile section per batch of XMem physical tokens.
//
// The section is committed whole when it is created. Committing through each
// view instead costs one wineserver request per view under Wine, and each
// request scans every range committed so far, so loading stalls on wineserver.
// If the whole commit fails (for example at the commit limit), views fall back
// to committing their own range.
pub(super) struct Section {
    bytes: u64,
    handle: OnceLock<OwnedHandle>,
    committed: AtomicBool,
}

impl Section {
    pub(super) fn new(count: usize) -> Self {
        Self {
            bytes: count as u64 * PAGE as u64,
            handle: OnceLock::new(),
            committed: AtomicBool::new(false),
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
        if self.handle.set(unsafe { OwnedHandle::from_raw_handle(h) }).is_ok() {
            let raw = self.handle.get().unwrap().as_raw_handle();
            self.committed.store(commit_whole(raw, self.bytes as usize), Ordering::Release);
        }
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
        if !self.committed.load(Ordering::Acquire)
            && unsafe { VirtualAlloc(view.Value, bytes, MEM_COMMIT, PAGE_READWRITE) }.is_null()
        {
            let err = unsafe { GetLastError() };
            unmap(address)?;
            return Err(err);
        }
        Ok(())
    }

    /// Copy `len` bytes to `offset` in this section through a temporary view.
    /// Every mapping of the section, including GPU imports, sees the write.
    pub(super) fn write_at(&self, offset: u64, src: *const u8, len: usize) -> Result<(), u32> {
        let base = offset & !(PAGE as u64 - 1);
        let within = (offset - base) as usize;
        let span = within + len;
        let view = unsafe {
            MapViewOfFile(self.handle()?, FILE_MAP_WRITE, (base >> 32) as u32, base as u32, span)
        };
        if view.Value.is_null() {
            return Err(unsafe { GetLastError() });
        }
        if !self.committed.load(Ordering::Acquire)
            && unsafe { VirtualAlloc(view.Value, span, MEM_COMMIT, PAGE_READWRITE) }.is_null()
        {
            let err = unsafe { GetLastError() };
            unsafe { UnmapViewOfFile(view) };
            return Err(err);
        }
        unsafe {
            ptr::copy_nonoverlapping(src, view.Value.cast::<u8>().add(within), len);
            UnmapViewOfFile(view);
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

// Commit every page of a SEC_RESERVE section through one temporary view.
// The commit belongs to the section, so it outlives the view.
fn commit_whole(handle: *mut c_void, bytes: usize) -> bool {
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, bytes) };
    if view.Value.is_null() {
        return false;
    }
    let ok = !unsafe { VirtualAlloc(view.Value, bytes, MEM_COMMIT, PAGE_READWRITE) }.is_null();
    unsafe { UnmapViewOfFile(view) };
    ok
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
