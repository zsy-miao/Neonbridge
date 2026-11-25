//! CPU virtual memory requests observed in Frosty's binned allocator.
use core::{ffi::c_void, ptr};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, HANDLE, SetLastError};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA;
use windows_sys::Win32::System::Memory::{
    GetWriteWatch, MEM_COMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, PAGE_READWRITE,
    PAGE_READONLY, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_GUARD, VirtualAlloc,
};

use windows_sys::Win32::System::SystemServices::{MEM_TOP_DOWN, MEM_WRITE_WATCH};

// Guest graphics placements have the same host protection as ordinary heaps.
static PLACEMENTS: LazyLock<Mutex<HashSet<usize>>> = LazyLock::new(Default::default);

pub(super) fn allocate_placement(bytes: usize, alignment: usize) -> *mut c_void {
    let mut bases = PLACEMENTS.lock().unwrap_or_else(|e| e.into_inner());
    if bases.try_reserve(1).is_err() {
        unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
        return ptr::null_mut();
    }
    use windows_sys::Win32::System::Memory::{
        MEM_ADDRESS_REQUIREMENTS, MEM_EXTENDED_PARAMETER, MEM_EXTENDED_PARAMETER_0,
        MEM_EXTENDED_PARAMETER_1, MemExtendedParameterAddressRequirements, VirtualAlloc2,
    };
    let Some(extent) = bytes.checked_next_multiple_of(alignment) else {
        unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
        return ptr::null_mut();
    };
    let mut requirements = MEM_ADDRESS_REQUIREMENTS { Alignment: alignment, ..Default::default() };
    let mut param = MEM_EXTENDED_PARAMETER {
        Anonymous1: MEM_EXTENDED_PARAMETER_0 { _bitfield: MemExtendedParameterAddressRequirements as u64 },
        Anonymous2: MEM_EXTENDED_PARAMETER_1 { Pointer: (&raw mut requirements).cast() },
    };
    let base = unsafe { VirtualAlloc2(windows_sys::Win32::System::Threading::GetCurrentProcess(),
        ptr::null(), extent, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE, &raw mut param, 1) };
    if !base.is_null() { bases.insert(base.addr()); }
    base
}

pub(super) unsafe fn free_placement(base: *mut c_void) {
    use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};
    let mut bases = PLACEMENTS.lock().unwrap_or_else(|e| e.into_inner());
    if unsafe { VirtualFree(base, 0, MEM_RELEASE) } != 0 { bases.remove(&base.addr()); }
}

// Direct CPU allocations own their tag until XMemFreeDefault releases them.
#[derive(Clone, Copy)]
struct DirectAllocation { extent: usize, tag: u64 }
static DIRECT: LazyLock<Mutex<HashMap<usize, DirectAllocation>>> = LazyLock::new(Default::default);

fn direct_tag() -> u64 {
    if !super::options::TAG_DIRECT.load(std::sync::atomic::Ordering::Acquire) {
        return 0;
    }
    // The query record contains the packed six-bit character values
    // followed by the decoded four bytes in display order.
    let fourcc = 0x586d5641;
    (super::make_tag(fourcc) >> 40) | (u64::from(fourcc.swap_bytes()) << 32)
}

fn remember_direct(allocations: &mut HashMap<usize, DirectAllocation>, base: *mut c_void, extent: usize) {
    if !base.is_null() {
        allocations.insert(base.addr(), DirectAllocation { extent, tag: direct_tag() });
    }
}

pub(super) fn allocate_direct(bytes: usize) -> *mut c_void {
    let Some(extent) = bytes.checked_next_multiple_of(4096) else {
        unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
        return ptr::null_mut();
    };
    let mut allocations = DIRECT.lock().unwrap_or_else(|e| e.into_inner());
    if allocations.try_reserve(1).is_err() {
        unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
        return ptr::null_mut();
    }
    let base = unsafe { VirtualAlloc(ptr::null(), extent, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
    remember_direct(&mut allocations, base, extent);
    base
}

/// Commit ordinary host pages for a graphics or physical `XMemAllocDefault`.
/// Graphics requests use at least 64 KiB pages. A 128 KiB alignment request
/// uses a 2 MiB alignment. `below_4gb` keeps the block under the 4 GiB line.
pub(super) fn allocate_device_pages(bytes: usize, attributes: u64) -> *mut c_void {
    let kind = ((attributes >> 29) & 15) as u32;
    let page_code = ((attributes >> 14) & 3) as u32;
    let align_code = ((attributes >> 24) & 31) as u32;
    let below_4gb = attributes & (1 << 33) != 0;
    let graphics = matches!(kind, 1..=6 | 10..=12);
    let physical = matches!(kind, 7..=9);
    if (!graphics && !physical) || page_code > 2 {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return ptr::null_mut();
    }
    let mut page = match page_code {
        2 => 2 << 20,
        1 => 64 << 10,
        _ => 4 << 10,
    };
    if graphics && page < (64 << 10) || physical && page_code != 2 {
        page = 64 << 10;
    }
    let mut alignment = if align_code == 0 { page } else { 1usize << align_code.min(31) };
    if align_code == 0x11 {
        alignment = 2 << 20;
    }
    alignment = alignment.max(page).max(0x10000);
    let Some(extent) = bytes.checked_next_multiple_of(alignment) else {
        unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
        return ptr::null_mut();
    };
    if !below_4gb && alignment <= 0x10000 {
        return allocate_direct(extent);
    }
    use windows_sys::Win32::System::Memory::{
        MEM_ADDRESS_REQUIREMENTS, MEM_EXTENDED_PARAMETER, MEM_EXTENDED_PARAMETER_0,
        MEM_EXTENDED_PARAMETER_1, MemExtendedParameterAddressRequirements, VirtualAlloc2,
    };
    let mut allocations = DIRECT.lock().unwrap_or_else(|e| e.into_inner());
    if allocations.try_reserve(1).is_err() {
        unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
        return ptr::null_mut();
    }
    let mut requirements = MEM_ADDRESS_REQUIREMENTS {
        Alignment: alignment,
        HighestEndingAddress: if below_4gb { 0xFFFF_FFFF as *mut c_void } else { ptr::null_mut() },
        ..Default::default()
    };
    let mut param = MEM_EXTENDED_PARAMETER {
        Anonymous1: MEM_EXTENDED_PARAMETER_0 { _bitfield: MemExtendedParameterAddressRequirements as u64 },
        Anonymous2: MEM_EXTENDED_PARAMETER_1 { Pointer: (&raw mut requirements).cast() },
    };
    let base = unsafe {
        VirtualAlloc2(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            ptr::null(),
            extent,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
            &raw mut param,
            1,
        )
    };
    remember_direct(&mut allocations, base, extent);
    base
}

pub(super) fn release_direct(base: *mut c_void) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};
    let mut allocations = DIRECT.lock().unwrap_or_else(|e| e.into_inner());
    if !allocations.contains_key(&base.addr()) { return false; }
    if unsafe { VirtualFree(base, 0, MEM_RELEASE) } != 0 { allocations.remove(&base.addr()); }
    true
}

/// Scarlett returns the desktop address fields followed by partition flags and an allocation tag.
#[repr(C)]
pub struct VirtualInfo {
    pub(super) memory: MEMORY_BASIC_INFORMATION,
    console_flags: u64,
    console_tag: u64,
}

#[repr(C)]
pub struct XmemOption {
    id: u32,
    reserved: u32,
    value: u64,
}

#[derive(Default)]
struct WatchState {
    suspension_requested: bool,
    suspended: bool,
}

static WATCH_STATE: Mutex<WatchState> = Mutex::new(WatchState {
    suspension_requested: false,
    suspended: false,
});

/// Set the process-wide allocation policy selected by the low DWORD payload.
///
/// # Safety
/// A recognized option must point to 12 readable bytes.
#[unsafe(export_name = "XMemSetOption")]
pub unsafe extern "system" fn set_option(option: *const XmemOption) {
    use std::sync::atomic::Ordering::Release;
    if option.is_null() {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return;
    }
    let id = unsafe { option.cast::<u32>().read_unaligned() };
    if !matches!(id, 1..=4 | 0x10000 | 0x10001) { return; }
    let value = unsafe { option.cast::<u8>().add(8).cast::<u32>().read_unaligned() };
    match id {
        1 => super::options::VERIFY.store(value != 0, Release),
        2 => super::options::TAG_DIRECT.store(value != 0, Release),
        3 => super::options::realloc_notifications(value),
        4 => super::options::set_page_policy(value != 0),
        0x10000 => {
            let mut state = WATCH_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.suspension_requested = value != 0;
            if state.suspension_requested { state.suspended = true; }
        }
        0x10001 => super::options::AUTO_WATCH.store(value != 0, Release),
        _ => unreachable!(),
    }
}

/// Resume write-watch queries after the suspension option is cleared.
#[unsafe(export_name = "XMemResumeWriteWatch")]
pub extern "system" fn resume_write_watch() -> i32 {
    let mut state = WATCH_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.suspension_requested {
        return 0;
    }
    state.suspended = false;
    1
}

/// Query dirty pages for a desktop write-watch allocation.
///
/// # Safety
/// The base must identify a write-watch reservation in this process. Output
/// pointers and their capacity must satisfy the Windows GetWriteWatch ABI.
#[unsafe(export_name = "XMemGetWriteWatch")]
pub unsafe extern "system" fn get_write_watch(
    flags: u32,
    base: *const c_void,
    bytes: usize,
    addresses: *mut *mut c_void,
    count: *mut usize,
    granularity: *mut u32,
) -> u32 {
    if WATCH_STATE.lock().unwrap_or_else(|e| e.into_inner()).suspended {
        // The console returns ERROR_CANTOPEN (1003) without modifying outputs.
        return 1003;
    }
    // The console forwards all six arguments to the host when not suspended.
    unsafe { GetWriteWatch(flags, base, bytes, addresses, count, granularity) }
}

/// Clear write-watch records for a region. A suspended watch returns
/// `ERROR_CANTOPEN` (1003) and does not call the host.
///
/// # Safety
/// `base` and `bytes` must describe a write-watch allocation the caller owns,
/// the same contract as the host reset.
#[unsafe(export_name = "XMemResetWriteWatch")]
pub unsafe extern "system" fn reset_write_watch(base: *const c_void, bytes: usize) -> u32 {
    if WATCH_STATE.lock().unwrap_or_else(|e| e.into_inner()).suspended {
        return 1003;
    }
    unsafe { windows_sys::Win32::System::Memory::ResetWriteWatch(base, bytes) }
}

/// Query ordinary CPU image and private mappings whose backing is distinguishable
/// from a translated graphics reservation.
///
/// # Safety
/// `information` must be null or point to 64 writable bytes.
#[unsafe(export_name = "XMemVirtualQuery")]
pub unsafe extern "system" fn virtual_query(
    process: HANDLE,
    address: *const c_void,
    information: *mut VirtualInfo,
) -> i32 {
    use windows_sys::Win32::System::Memory::{MEM_FREE, MEM_IMAGE, MEM_PRIVATE, VirtualQueryEx};
    if information.is_null() {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return 0;
    }
    let mut area = MEMORY_BASIC_INFORMATION::default();
    if unsafe { VirtualQueryEx(process, address, &mut area, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
        return 0;
    }
    // Both graphics placement and physical-page apertures are represented as
    // private allocations on this host. Their Xbox flags cannot be inferred
    // from the CPU protection of the individual queried page.
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetProcessId};
    let current = unsafe { GetProcessId(process) == GetCurrentProcessId() };
    let ordinary_protect = matches!(area.AllocationProtect & !PAGE_GUARD,
        PAGE_READONLY | PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE);
    let writable_private = current && area.Type == MEM_PRIVATE && area.AllocationProtect == PAGE_READWRITE
        && !PLACEMENTS.lock().unwrap_or_else(|e| e.into_inner()).contains(&area.AllocationBase.addr())
        && !super::pools::is_arena(area.AllocationBase.addr());
    let cpu_private = area.Type == MEM_PRIVATE && (ordinary_protect || writable_private);
    let free = area.State == MEM_FREE && area.Protect == PAGE_NOACCESS;
    if !(free || area.Type == MEM_IMAGE || cpu_private) {
        unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
        return 0;
    }
    let Some(title_id) = super::partitions::title_partition_id() else { return 0 };
    let direct = if current && area.Type == MEM_PRIVATE {
        DIRECT.lock().unwrap_or_else(|e| e.into_inner()).get(&area.AllocationBase.addr()).copied()
    } else { None };
    let mut state = area.State;
    if writable_private {
        // A read-only query of the host watch list distinguishes MEM_WRITE_WATCH
        // from an ordinary PAGE_READWRITE allocation. Do not reset the list.
        let mut address = ptr::null_mut();
        let mut count = 1;
        let mut granularity = 0;
        let previous = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if unsafe { GetWriteWatch(0, area.BaseAddress, 4096, &mut address, &mut count, &mut granularity) } == 0 {
            state |= 1 << 21;
        }
        unsafe { SetLastError(previous) };
    }
    // The guest ABI clears the partition-id slot and all padding. Build the
    // record in place so a Rust struct copy cannot leave padding unspecified.
    unsafe {
        information.cast::<u8>().write_bytes(0, size_of::<VirtualInfo>());
        let out = &mut (*information).memory;
        out.BaseAddress = area.BaseAddress;
        out.AllocationBase = area.AllocationBase;
        out.AllocationProtect = area.AllocationProtect;
        out.RegionSize = direct.filter(|a| a.tag != 0).map_or(area.RegionSize, |a| a.extent);
        out.State = state;
        out.Protect = area.Protect;
        out.Type = area.Type;
        (*information).console_flags = u64::from(area.PartitionId == title_id);
        (*information).console_tag = direct.map_or(0, |a| a.tag);
    }
    1
}

/// Reserve or commit CPU memory for the binned allocator and physical-page apertures.
///
/// # Safety
/// A nonnull address must identify a range the caller is entitled to reserve or
/// commit in this process. The caller owns successful reservations and releases
/// them through VirtualFree.
#[unsafe(export_name = "XMemVirtualAlloc")]
pub unsafe extern "system" fn virtual_alloc(
    address: *mut c_void,
    bytes: usize,
    allocation_type: u32,
    profile: u64,
    protect: u32,
) -> *mut c_void {
    let result = unsafe { virtual_alloc_inner(address, bytes, allocation_type, profile, protect) };
    super::diagnostics::watch("XMemVirtualAlloc", result as usize, bytes, || format!(
        "requested={address:p} type={allocation_type:#x} profile={profile:#x} protect={protect:#x}"));
    result
}

unsafe fn virtual_alloc_inner(
    address: *mut c_void,
    bytes: usize,
    allocation_type: u32,
    profile: u64,
    protect: u32,
) -> *mut c_void {
    // GTA's startup heap requests a single committed read/write range with
    // console page and cache attributes. Back its CPU accesses with ordinary
    // host pages so the original base can also be released with VirtualFree.
    if matches!((profile, allocation_type, protect),
        (0x183 | 0x383, 0x2000_3000, 0x22004)
            | (0x283, 0x2000_3000, 0x2004 | 0x2404)
            | (0x181, 0x2040_3000, PAGE_READWRITE)
            | (7, 0x2040_3000, 0x30004))
    {
        return unsafe { VirtualAlloc(address, bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
    }
    if (profile, allocation_type, protect) == (0x283, 0x2040_1000, 0x42404) {
        return unsafe { VirtualAlloc(address, bytes, MEM_COMMIT, PAGE_READWRITE) };
    }
    // Frosty's BinnedAllocFromOS passes protection in its fifth argument.
    // Large allocations in that routine also request top-down placement.
    // Frosty's physical-page pool reserves an aperture, then backs individual
    // 64 KiB pages as needed. The D3D12 placement uploader reads these CPU pages.
    if allocation_type == 0x2040_2000
        && matches!((profile, protect), (3, 0x801) | (0x21, PAGE_READWRITE) | (0x23, 0x2004 | 0x2404 | 0x10404 | 0x22004))
    {
        // CPU physical pages and the GPU-only NOACCESS aperture both
        // acquire backing later. A Windows placeholder supports both.
        let Some(extent) = bytes.checked_next_multiple_of(super::shared_pages::PAGE) else {
            unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
            return ptr::null_mut();
        };
        return super::shared_pages::reserve(address, extent);
    }
    if profile == 3 {
        let host = match (allocation_type, protect) {
            (0x2040_2000, 0x2404 | 0x10404) => Some((MEM_RESERVE, PAGE_NOACCESS)),
            (0x2040_3000, 0x2404 | 0x10404) => Some((MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)),
            (0x2040_1000, 0x2404 | 0x10404) if !address.is_null() => {
                Some((MEM_COMMIT, PAGE_READWRITE))
            }
            _ => None,
        };
        if let Some((kind, protection)) = host {
            let watch = kind & MEM_RESERVE != 0 && super::options::auto_watch_available();
            return unsafe { VirtualAlloc(address, bytes, if watch { kind | MEM_WRITE_WATCH } else { kind }, protection) };
        }
    }
    let operation = allocation_type & !MEM_TOP_DOWN;
    let base_protect = protect & !PAGE_GUARD;
    let guard_allowed = protect & PAGE_GUARD == 0 || base_protect != PAGE_NOACCESS;
    let plain = guard_allowed && matches!(base_protect, PAGE_NOACCESS | PAGE_READONLY | PAGE_READWRITE |
        PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE)
        && (operation == MEM_RESERVE || operation == MEM_COMMIT || operation == MEM_RESERVE | MEM_COMMIT);
    let watched = (operation == MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH && protect == PAGE_READWRITE)
        || (operation == MEM_RESERVE | MEM_WRITE_WATCH && protect == PAGE_NOACCESS);
    // Reserving the aperture does not map a CPU page. GPU/cache attributes
    // take effect on the eventual backing, while its host VA stays inaccessible.
    if profile == 0x27 && allocation_type == 0x2040_6000 && protect & 0x3ff == PAGE_READWRITE {
        return super::shared_pages::replace_aperture(address, bytes);
    }
    if profile == 7 && allocation_type == (MEM_RESERVE | windows_sys::Win32::System::Memory::MEM_RESERVE_PLACEHOLDER) && protect == PAGE_NOACCESS {
        return super::shared_pages::reserve(address, bytes);
    }
    if profile != 1 || !(plain || watched) {
        let description = format!(
            "TuxBox: unsupported XMemVirtualAlloc address={address:p} bytes={bytes} type={allocation_type:#x} profile={profile:#x} protect={protect:#x}\0"
        );
        unsafe { OutputDebugStringA(description.as_ptr()) };
        // SAFETY: The error code is written to the current thread.
        unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
        return ptr::null_mut();
    }

    // SAFETY: The caller supplies the address range. Windows validates the
    // reservation, rounds to host page boundaries, and sets the last error on failure.
    unsafe { VirtualAlloc(address, bytes, allocation_type, protect) }
}
