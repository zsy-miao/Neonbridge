//! CPU-backed Xbox default allocations. Callers retain ownership until the matching free.
mod diagnostics;
mod heap;
mod history;
mod options;
mod partitions;
mod physical_pages;
mod pools;
mod shared_pages;
mod virtual_memory;

use core::{ffi::c_void, mem::size_of, ptr};
use std::sync::{LazyLock, RwLock};

use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, SetLastError};
use windows_sys::Win32::System::Memory::{
    GetProcessHeap, HeapCreate, HEAP_NO_SERIALIZE,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

// Series audio working memory uses 64 KiB aligned, CPU-writable pages.
const AUDIO_PLACEMENT: u64 = 0xf081_4000;
const DISPLAY_PLACEMENT: u64 = 0x3080_8000;
const PLACEMENT: u64 = 0x3080_4000;

type AllocateHook = unsafe extern "system" fn(usize, u64) -> *mut c_void;
type FreeHook = unsafe extern "system" fn(*mut c_void, u64);

#[derive(Clone, Copy)]
struct AllocationHooks {
    allocate: AllocateHook,
    release: Option<FreeHook>,
}

// Publish the pair together and release the read lock before invoking a guest
// callback, so a callback can replace hooks without deadlocking.
static ALLOCATION_HOOKS: LazyLock<RwLock<Option<AllocationHooks>>> = LazyLock::new(|| {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    let exported = unsafe { GetProcAddress(module, c"__XMemAllocationHooks__".as_ptr().cast()) };
    // The optional PE data export is a pair of function pointers, valid for
    // the process lifetime. A title can replace or clear it with SetAllocationHooks.
    let hooks = exported.and_then(|symbol| {
        #[repr(C)]
        struct TitleHooks { allocate: Option<AllocateHook>, release: Option<FreeHook> }
        let pair = unsafe { (symbol as *const TitleHooks).read_unaligned() };
        Some(AllocationHooks { allocate: pair.allocate?, release: Some(pair.release?) })
    });
    RwLock::new(hooks)
});

/// Return this provider's host-backed title heap handle.
#[unsafe(export_name = "XMemGetTitleHeap")]
pub extern "system" fn get_title_heap() -> *mut c_void {
    // SAFETY: GetProcessHeap takes no arguments and returns this process's heap.
    unsafe { GetProcessHeap() }
}

/// Create an ordinary CPU heap with the observed title-partition flags.
/// Partition-specific and caller-supplied backing modes remain unsupported.
#[unsafe(export_name = "XMemCreateWin32Heap")]
pub extern "system" fn create_win32_heap(flags: u64, base: *mut c_void) -> *mut c_void {
    let mode = flags & !0x8000;
    if mode > 7 {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return ptr::null_mut();
    }
    if mode & 2 != 0 && (mode != 2 || base.is_null() || (base as usize) & 0xfff != 0) {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return ptr::null_mut();
    }
    let host_flags = match mode {
        0 => HEAP_NO_SERIALIZE,
        4 => 0,
        _ => {
            unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
            return ptr::null_mut();
        }
    };
    // SAFETY: A zero initial and maximum size requests a private growable heap.
    unsafe { HeapCreate(host_flags, 0, 0) }
}

/// Install callbacks as a pair. A null allocator restores both defaults.
///
/// # Safety
/// Nonnull callbacks must obey the Xbox allocation ABI and remain valid until
/// no in-flight allocation or free call can invoke them.
#[unsafe(export_name = "XMemSetAllocationHooks")]
pub unsafe extern "system" fn set_allocation_hooks(
    allocate: Option<AllocateHook>,
    release: Option<FreeHook>,
) {
    let mut hooks = ALLOCATION_HOOKS.write().unwrap_or_else(|e| e.into_inner());
    *hooks = allocate.map(|allocate| AllocationHooks { allocate, release });
}

/// Allocate through the installed callback or the default provider.
#[unsafe(export_name = "XMemAlloc")]
pub extern "system" fn alloc(bytes: usize, attributes: u64) -> *mut c_void {
    let hooks = *ALLOCATION_HOOKS.read().unwrap_or_else(|e| e.into_inner());
    let result = match hooks {
        // SAFETY: Installing the callback requires the caller to keep it valid.
        Some(hooks) => unsafe { (hooks.allocate)(bytes, attributes) },
        None => alloc_default(bytes, attributes),
    };
    if options::VERIFY.load(std::sync::atomic::Ordering::Acquire) { options::validate(result, bytes, attributes); }
    result
}

/// Release through the installed callback or the default provider.
///
/// # Safety
/// The pointer and attributes must satisfy the currently installed free callback.
#[unsafe(export_name = "XMemFree")]
pub unsafe extern "system" fn free(memory: *mut c_void, attributes: u64) {
    let hooks = *ALLOCATION_HOOKS.read().unwrap_or_else(|e| e.into_inner());
    match hooks {
        Some(AllocationHooks { release: Some(release), .. }) => unsafe { release(memory, attributes) },
        Some(_) => unsafe { SetLastError(ERROR_INVALID_PARAMETER) },
        None => unsafe { free_default(memory, attributes) },
    }
}

/// Encode the four-byte tag in the console allocation tag's high word.
#[unsafe(export_name = "XMemMakeTag")]
pub extern "system" fn make_tag(fourcc: u32) -> u64 {
    if fourcc == 0 {
        return 0;
    }
    let mut word = 0u32;
    for (index, byte) in fourcc.to_le_bytes().into_iter().enumerate() {
        let value = match byte {
            b'a'..=b'z' => byte - b'a',
            b'A'..=b'Z' => byte - b'A' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'$' => 62,
            _ => 63,
        };
        word |= u32::from(value) << (8 + 6 * index);
    }
    u64::from(word) << 32
}

fn process_heap_attributes(attributes: u64) -> bool {
    // The supplied Xbox DLL routes selector 0 with ordinary CPU allocation
    // types 0..4 and no special alignment bits through its partition heap.
    // The remaining low attribute bits are metadata and do not change this path.
    (attributes >> 29) & 0xf == 0
        && attributes & 0x1f00_0000 <= 0x0400_0000
        && attributes & 0xc000 == 0
}

fn device_memory(attributes: u64) -> bool {
    // Graphics types 1..=6 and 10..=12, and physical types 7..=9.
    // Page-size codes above 2 are not a host page class.
    matches!((attributes >> 29) & 15, 1..=12) && (attributes >> 14) & 3 <= 2
}

/// Allocate guest memory with the selected backing. A null return denotes failure.
#[unsafe(export_name = "XMemAllocDefault")]
pub extern "system" fn alloc_default(bytes: usize, attributes: u64) -> *mut c_void {
    let result = alloc_default_inner(bytes, attributes);
    diagnostics::watch("XMemAllocDefault", result as usize, bytes, || format!("attributes={attributes:#x}"));
    result
}

fn alloc_default_inner(bytes: usize, attributes: u64) -> *mut c_void {
    if bytes == 0 {
        return ptr::null_mut();
    }
    match attributes {
        // SAFETY: The process heap is valid for this process; HeapAlloc checks the requested size.
        attr if process_heap_attributes(attr) => heap::allocate(unsafe { GetProcessHeap() }, bytes),
        PLACEMENT | AUDIO_PLACEMENT => virtual_memory::allocate_placement(bytes, 0x10000),
        DISPLAY_PLACEMENT => virtual_memory::allocate_placement(bytes, 0x200000),
        attr if pools::cpu_attributes(attr) && bytes <= 0x20000 => pools::allocate(bytes, attr),
        attr if pools::cpu_attributes(attr) && attr & 0xc000 == 0x4000 => {
            // Page-size selector 1 uses 64 KiB extents. A fresh host virtual
            // reservation already satisfies the requested (at most 64 KiB) alignment.
            if !options::can_demote_pages() {
                unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
                return ptr::null_mut();
            }
            let Some(extent) = bytes.checked_next_multiple_of(0x10000) else {
                unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY) };
                return ptr::null_mut();
            };
            virtual_memory::allocate_direct(extent)
        }
        attr if pools::cpu_attributes(attr) && attr & 0xc000 == 0 => virtual_memory::allocate_direct(bytes),
        attr if device_memory(attr) => {
            let result = virtual_memory::allocate_device_pages(bytes, attr);
            static NOTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if NOTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                let line = format!(
                    "TuxBox xmem: device allocation attributes={attr:#x} bytes={bytes} address={result:p}\0"
                );
                unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(line.as_ptr()) };
            }
            result
        }
        _ => {
            static NOTED: std::sync::Once = std::sync::Once::new();
            NOTED.call_once(|| {
                let line = format!("TuxBox xmem: unsupported XMemAllocDefault attributes={attributes:#x} bytes={bytes}\0");
                unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(line.as_ptr()) };
            });
            // SAFETY: SetLastError accepts this Windows error code and only updates thread-local state.
            unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
            ptr::null_mut()
        }
    }
}

/// Release a guest allocation using its original backing.
///
/// # Safety
/// For a supported attribute, a nonnull pointer must be the base of a live allocation
/// returned by `alloc_default` with that attribute. The caller must have exclusive
/// permission to free it, with no further use after release.
#[unsafe(export_name = "XMemFreeDefault")]
pub unsafe extern "system" fn free_default(memory: *mut c_void, attributes: u64) {
    if memory.is_null() && !process_heap_attributes(attributes) {
        return;
    }
    match attributes {
        attr if process_heap_attributes(attr) => {
            // SAFETY: The caller supplies a live allocation from this process heap.
            unsafe { heap::free(GetProcessHeap(), memory) };
        }
        PLACEMENT | DISPLAY_PLACEMENT | AUDIO_PLACEMENT => {
            // SAFETY: The caller supplies the reservation base; MEM_RELEASE requires size zero.
            unsafe { virtual_memory::free_placement(memory) };
        }
        attr if pools::cpu_attributes(attr) && (virtual_memory::release_direct(memory) || pools::release(memory.addr())) => {}
        attr if device_memory(attr) && virtual_memory::release_direct(memory) => {}
        // SAFETY: Recording an unsupported attribute does not access the caller's allocation.
        _ => unsafe { SetLastError(ERROR_NOT_SUPPORTED) },
    }
}

/// Return the host-backed title memory profile requested by Frosty.
///
/// # Safety
/// With selector 0x20000001, a nonnull output must point to 120 writable bytes.
#[unsafe(export_name = "XMemGetWorkingSetStatistics")]
pub unsafe extern "system" fn working_set_statistics(selector: u32, out: *mut [u64; 15]) -> i32 {
    if selector != 0x2000_0001 || out.is_null() {
        let error = if out.is_null() {
            ERROR_INVALID_PARAMETER
        } else {
            ERROR_NOT_SUPPORTED
        };
        // SAFETY: This updates only the calling thread's error.
        unsafe { SetLastError(error) };
        return 0;
    }
    // SAFETY: The caller provides the complete v2 result. The console zeroes it before querying.
    unsafe { out.write_unaligned([0; 15]) };
    let mut memory = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: The initialized length describes the writable local buffer.
    if unsafe { GlobalMemoryStatusEx(&mut memory) } == 0 {
        return 0;
    }
    let cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let mut process = PROCESS_MEMORY_COUNTERS_EX {
        cb,
        ..Default::default()
    };
    // SAFETY: This pseudo-handle identifies the calling process. The EX record has the
    // base record as its prefix; cb selects the extended size and private-commit counter.
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut process).cast(), cb) } == 0 {
        return 0;
    }

    // Desktop has no dedicated Xbox partition. Limit additional headroom to both
    // host-available physical memory and commit available to this process.
    // Proton may only populate PagefileUsage. Windows defines both fields as the
    // process commit charge, including committed pages that are resident in RAM.
    let committed = if process.PrivateUsage != 0 {
        process.PrivateUsage
    } else {
        process.PagefileUsage
    };
    let used = (committed as u64).min(memory.ullTotalPhys);
    let headroom = memory.ullAvailPhys.min(memory.ullAvailPageFile);
    let budget = used.saturating_add(headroom.saturating_sub(physical_pages::relinquished())).min(memory.ullTotalPhys);
    let mut result = [0; 15];
    result[0] = budget;
    result[1] = used;
    // The console repeats these fields for a single pool. Other console-only
    // counters stay zero on a host with no corresponding reported hardware.
    result[5] = budget;
    result[6] = used;
    result[11] = (process.PeakPagefileUsage as u64).max(used);
    // SAFETY: Same caller-provided result checked above. Publish the populated snapshot.
    unsafe { out.write_unaligned(result) };
    publish_graphics_budget(out);
    1
}

struct GraphicsBudget {
    registered: bool,
    bytes: [u8; 24],
}

static GRAPHICS_BUDGET: std::sync::Mutex<GraphicsBudget> = std::sync::Mutex::new(GraphicsBudget {
    registered: false,
    bytes: [0; 24],
});

/// Record the one-shot 24-byte graphics budget. Later calls are ignored.
/// The first dword is the GPU page-table size, and zero means 20 MiB.
/// The second dword is the frame-preserve size.
///
/// # Safety
/// `config` must be null or point at 24 readable bytes.
#[unsafe(export_name = "XMemRegisterGraphicsMemoryConfiguration")]
pub unsafe extern "system" fn register_graphics_memory(config: *const u8) {
    if config.is_null() {
        return;
    }
    let mut slot = GRAPHICS_BUDGET.lock().unwrap_or_else(|error| error.into_inner());
    if slot.registered {
        return;
    }
    // SAFETY: the caller provided 24 readable bytes and the slot owns the destination.
    unsafe { config.copy_to_nonoverlapping(slot.bytes.as_mut_ptr(), 24) };
    slot.registered = true;
}

fn publish_graphics_budget(out: *mut [u64; 15]) {
    let slot = GRAPHICS_BUDGET.lock().unwrap_or_else(|error| error.into_inner());
    if !slot.registered {
        return;
    }
    let page_table = u32::from_le_bytes([slot.bytes[0], slot.bytes[1], slot.bytes[2], slot.bytes[3]]);
    let frame = u32::from_le_bytes([slot.bytes[4], slot.bytes[5], slot.bytes[6], slot.bytes[7]]);
    let page_table = if page_table == 0 { 0x140_0000 } else { page_table };
    // The title record keeps the frame-preserve size at byte 84 and the GPU
    // page-table size at byte 96. Both sit inside the 120-byte result.
    unsafe {
        let base = out.cast::<u8>();
        base.add(84).cast::<u32>().write_unaligned(frame);
        base.add(96).cast::<u32>().write_unaligned(page_table);
    }
}
