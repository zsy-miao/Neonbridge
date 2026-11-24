//! Section-backed CPU views for the Xbox 64 KiB user-physical aperture.
use core::{ffi::c_void, ptr};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
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

thread_local! {
    /// The last OS call that failed on this thread (function:line, error),
    /// for the page-mapping failure log (test80: 256 error-87 failures with no
    /// step named).
    static LAST_OS_ERROR: std::cell::Cell<(&'static str, u32)> = const { std::cell::Cell::new(("", 0)) };
}

/// GetLastError, noted with the step that failed.
pub(super) fn os_error(step: &'static str) -> u32 {
    let error = unsafe { GetLastError() };
    LAST_OS_ERROR.with(|l| l.set((step, error)));
    error
}

/// Takes the last failed OS step on this thread.
pub(super) fn take_os_error() -> (&'static str, u32) {
    LAST_OS_ERROR.with(|l| l.replace(("", 0)))
}

/// Physical-page windows the title reserved with PAGE_GUARD: pages mapped
/// there keep the guard; only cache modifiers are dropped from a view's
/// protection (leonida 8d4dd7a8 / 80ed5dd3, re-derived). Base -> end.
static GUARDED: std::sync::Mutex<std::collections::BTreeMap<usize, usize>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

fn guarded() -> std::sync::MutexGuard<'static, std::collections::BTreeMap<usize, usize>> {
    GUARDED.lock().unwrap_or_else(|e| e.into_inner())
}

/// TUXBOX_XMEM_GUARD=0 refuses guarded windows, as before.
pub(super) fn guard_enabled() -> bool {
    std::env::var("TUXBOX_XMEM_GUARD").map_or(true, |v| v.trim() != "0")
}

/// Reserve a window whose title protection carries PAGE_GUARD.
pub(super) fn reserve_guarded(address: *const c_void, bytes: usize) -> *mut c_void {
    let base = reserve(address, bytes);
    if !base.is_null() {
        guarded().insert(base.addr(), base.addr().saturating_add(bytes));
    }
    base
}

fn is_guarded(address: usize) -> bool {
    guarded().range(..=address).next_back().is_some_and(|(_, &end)| address < end)
}

pub(super) fn reserve(address: *const c_void, bytes: usize) -> *mut c_void {
    // A new window at a released one's addresses starts unguarded.
    if !address.is_null() {
        let (start, end) = (address.addr(), address.addr().saturating_add(bytes));
        guarded().retain(|&b, &mut e| e <= start || end <= b);
    }
    if address.is_null() {
        // Neonbridge: keep placeholders inside the console GPU window.
        return unsafe { super::virtual_memory::host_alloc(address, bytes, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, 0) };
    }
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
            return Err(os_error("handle:111"));
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
            return Err(os_error("map_range:138"));
        }
        if unsafe { VirtualAlloc(view.Value, bytes, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
            let err = os_error("map_range:141");
            unmap(address)?;
            return Err(err);
        }
        if is_guarded(address) {
            let mut old = 0;
            let guard = windows_sys::Win32::System::Memory::PAGE_GUARD;
            if unsafe { windows_sys::Win32::System::Memory::VirtualProtect(view.Value, bytes, PAGE_READWRITE | guard, &mut old) } == 0 {
                let err = os_error("map_range:149");
                unmap(address)?;
                return Err(err);
            }
        }
        Ok(())
    }

    /// Copy `bytes` from `src` into the section at `offset` (within one page)
    /// through a temporary view, committing the page first.
    pub(super) fn write_from(&self, offset: u64, src: *const u8, bytes: usize) -> Result<(), u32> {
        let page = offset & !(PAGE as u64 - 1);
        let within = (offset - page) as usize;
        if within + bytes > PAGE {
            return Err(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER);
        }
        let view = unsafe { MapViewOfFile(self.handle()?, FILE_MAP_WRITE, (page >> 32) as u32, page as u32, PAGE) };
        if view.Value.is_null() {
            return Err(os_error("write_from:167"));
        }
        let result = if unsafe { VirtualAlloc(view.Value, PAGE, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
            Err(os_error("write_from:170"))
        } else {
            unsafe { ptr::copy_nonoverlapping(src, view.Value.cast::<u8>().add(within), bytes) };
            Ok(())
        };
        unsafe { UnmapViewOfFile(view) };
        result
    }

    /// Copy `bytes` at `offset` (within one page) out of the section.
    pub(super) fn read_part(&self, offset: u64, dest: *mut u8, bytes: usize) -> Result<(), u32> {
        let page = offset & !(PAGE as u64 - 1);
        let within = (offset - page) as usize;
        if within + bytes > PAGE {
            return Err(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER);
        }
        let view = unsafe { MapViewOfFile(self.handle()?, FILE_MAP_READ, (page >> 32) as u32, page as u32, PAGE) };
        if view.Value.is_null() {
            return Err(os_error("read_part:188"));
        }
        // Only pages with section data are read here (uncommitted ones fault).
        unsafe {
            ptr::copy_nonoverlapping(view.Value.cast::<u8>().add(within), dest, bytes);
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
            return Err(os_error("read_into:209"));
        }
        unsafe {
            ptr::copy_nonoverlapping(view.Value.cast(), dest, PAGE);
            UnmapViewOfFile(view);
        }
        Ok(())
    }
}

/// test77: unmapping pool pages back to placeholders failed with error 87
/// (ERROR_INVALID_PARAMETER) on views of one to four pages, so the title's
/// CopyPageMappingsBatchX batches that unmapped them were refused. When the
/// unmap step or the placeholder coalesce returns 87, these fall back
/// (TUXBOX_XMEM_UNMAP_FALLBACK=0 keeps the error), logged.
fn fallback_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_XMEM_UNMAP_FALLBACK").map_or(true, |v| v.trim() != "0"))
}

fn fallback_note(what: std::fmt::Arguments) {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::loader::log(&format!("{what} ({n} so far; TUXBOX_XMEM_UNMAP_FALLBACK=0 fails)"));
    }
}

/// test82: 256 unmaps failed with error 87 at the base of a lone or
/// recorded view, both keeping and not keeping its placeholder, with no live
/// GPU import over the page. A released imported heap's kernel allocation can
/// hold the view until the GPU passes it, and test82's queues were stalled.
/// An unmap failing with 87 is retried after 1, 2, 4, 8 and 16 ms (31 ms at
/// most), logged; TUXBOX_XMEM_UNMAP_RETRY=0 fails at once.
pub(super) fn unmap(address: usize) -> Result<(), u32> {
    let first = unmap_once(address);
    if first != Err(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER)
        || NO_RETRY.with(|n| n.get())
        || std::env::var("TUXBOX_XMEM_UNMAP_RETRY").is_ok_and(|v| v.trim() == "0")
    {
        return first;
    }
    let step = LAST_OS_ERROR.with(|e| e.get());
    let started = std::time::Instant::now();
    let mut result = first;
    for ms in [1u64, 2, 4, 8, 16] {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        result = unmap_once(address);
        if result != Err(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER) { break; }
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 16 || n.is_power_of_two() {
        crate::loader::log(&format!(
            "xmem: unmapping view {address:#x} failed with error 87 at {step:?}; retried for {} ms: {} ({n} so far; TUXBOX_XMEM_UNMAP_RETRY=0 off)",
            started.elapsed().as_millis(), match result { Ok(()) => "unmapped".to_string(), Err(e) => format!("still error {e}") }));
    }
    result
}

thread_local! {
    static NO_RETRY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with unmaps failing at once (no sleeping retry).
pub(super) fn without_retry<T>(f: impl FnOnce() -> T) -> T {
    let before = NO_RETRY.with(|n| n.replace(true));
    let result = f();
    NO_RETRY.with(|n| n.set(before));
    result
}

fn unmap_once(address: usize) -> Result<(), u32> {
    let view = MEMORY_MAPPED_VIEW_ADDRESS {
        Value: address as _,
    };
    if unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, MEM_PRESERVE_PLACEHOLDER) } != 0 {
        return Ok(());
    }
    let error = os_error("unmap:244");
    if error != windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER || !fallback_on() {
        return Err(error);
    }
    // A view not created over a placeholder cannot keep one: unmap it outright
    // and reserve a placeholder of its size at the same address.
    let info = region(address)?;
    if info.AllocationBase.addr() != address || info.Type != windows_sys::Win32::System::Memory::MEM_MAPPED {
        return Err(error);
    }
    let bytes = info.RegionSize;
    if unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, 0) } == 0 {
        return Err(os_error("unmap:256"));
    }
    let placeholder = unsafe {
        VirtualAlloc2(GetCurrentProcess(), address as _, bytes, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, ptr::null_mut(), 0)
    };
    if placeholder.is_null() {
        let e = os_error("unmap:262");
        fallback_note(format_args!("xmem: view {address:#x} (+{bytes:#x}) unmapped without its placeholder, which could not be reserved again: error {e}"));
        return Err(e);
    }
    fallback_note(format_args!("xmem: view {address:#x} (+{bytes:#x}) kept no placeholder on unmap (error 87): unmapped and reserved a placeholder there"));
    Ok(())
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
        return Err(os_error("region:280"));
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
        return Err(os_error("isolate_range:300"));
    }
    Ok(())
}

pub(super) fn coalesce(base: usize, bytes: usize) -> Result<(), u32> {
    if region(base)?.RegionSize == bytes {
        return Ok(());
    }
    if unsafe { VirtualFree(base as _, bytes, MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS) } == 0 {
        let error = os_error("coalesce:310");
        // Placeholders from two reservations (a re-reserved join) or around a
        // still-mapped view cannot coalesce; split placeholders stay reserved
        // and map as before, so this is not a failure.
        if error == windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER && fallback_on() {
            fallback_note(format_args!("xmem: placeholders at {base:#x} (+{bytes:#x}) left split: coalesce gave error 87"));
            return Ok(());
        }
        Err(error)
    } else {
        Ok(())
    }
}

// Neonbridge: join adjacent placeholders exactly covering [base, base+bytes).
pub(super) fn coalesce_range(base: usize, bytes: usize) -> Result<(), u32> {
    if unsafe { VirtualFree(base as _, bytes, MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS) } == 0 {
        Err(os_error("coalesce_range:327"))
    } else {
        Ok(())
    }
}

// Neonbridge: split the leading `bytes` off a placeholder.
pub(super) fn split(base: usize, bytes: usize) -> Result<(), u32> {
    if unsafe { VirtualFree(base as _, bytes, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) } == 0 {
        Err(os_error("split:336"))
    } else {
        Ok(())
    }
}

/// Neonbridge: placeholders coalesce only inside one OS reservation, so a
/// range whose unmapped placeholders (`pieces`, each [a, b)) come from two
/// reservations (a joined range and the next page, Err(87)) is re-reserved:
/// every placeholder in it is released and one new placeholder reserved over
/// [lo, hi). On failure the original placeholders are reserved again.
/// TUXBOX_JOIN_RERESERVE=0 leaves such ranges unjoined (shadow copies).
pub(super) fn re_reserve(lo: usize, hi: usize, pieces: &[(usize, usize)]) -> Result<(), u32> {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("TUXBOX_JOIN_RERESERVE").is_none_or(|v| v != "0")) {
        return Err(87);
    }
    let restore = |freed: &[(usize, usize)]| {
        for &(a, b) in freed {
            let _ = reserve(a as _, b - a);
        }
    };
    for (i, &(a, _)) in pieces.iter().enumerate() {
        if unsafe { VirtualFree(a as _, 0, MEM_RELEASE) } == 0 {
            let e = os_error("re_reserve:360");
            restore(&pieces[..i]);
            return Err(e);
        }
    }
    if reserve(lo as _, hi - lo).is_null() {
        let e = os_error("re_reserve:366");
        restore(pieces);
        return Err(e);
    }
    Ok(())
}
