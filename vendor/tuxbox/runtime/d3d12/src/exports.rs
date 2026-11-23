use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, S_OK};
use windows::Win32::Graphics::Direct3D12::ID3D12Debug;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_UNSUPPORTED;
use windows::core::{GUID, HRESULT, Interface};

// The Xbox debug interface has one flag-setting method. Its object stores a
// second table for QueryInterface, AddRef and Release at byte offset eight.
#[repr(C)]
struct DebugInterface {
    methods: &'static DebugMethods,
    unknown: &'static DebugUnknown,
}

#[repr(C)]
struct DebugMethods {
    set_flags: extern "system" fn(*const DebugInterface, u32) -> HRESULT,
}

#[repr(C)]
struct DebugUnknown {
    query:
        unsafe extern "system" fn(*const DebugInterface, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: extern "system" fn(*const DebugInterface) -> u32,
    release: extern "system" fn(*const DebugInterface) -> u32,
}

static DEBUG: DebugInterface = DebugInterface {
    methods: &DebugMethods { set_flags },
    unknown: &DebugUnknown {
        query: debug_query,
        add_ref,
        release,
    },
};

extern "system" fn set_flags(_: *const DebugInterface, flags: u32) -> HRESULT {
    crate::set_process_debug_flags(flags)
}

extern "system" fn add_ref(_: *const DebugInterface) -> u32 {
    1
}

extern "system" fn release(_: *const DebugInterface) -> u32 {
    0
}

unsafe extern "system" fn debug_query(
    _: *const DebugInterface,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: Both interfaces take the same IID and output pointer contract.
    unsafe { get_debug_interface(iid, out) }
}

/// Returns the process-wide Xbox debug object.
///
/// # Safety
/// The caller supplies a readable IID and a writable output pointer on success.
#[unsafe(export_name = "D3D12GetDebugInterface")]
pub unsafe extern "system" fn get_debug_interface(
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: The caller supplies a readable IID. The console leaves the
    // caller's output unchanged for every other interface, including IUnknown.
    if unsafe { *iid } != ID3D12Debug::IID {
        return E_NOINTERFACE;
    }
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: The output is writable. DEBUG and both tables live for the process.
    unsafe { out.write((&raw const DEBUG).cast_mut().cast()) };
    S_OK
}

// All four legacy DXGIX exports alias this result in both console DLLs.
#[unsafe(no_mangle)]
pub extern "system" fn tuxbox_dxgix_unsupported() -> HRESULT {
    DXGI_ERROR_UNSUPPORTED
}

static FRAMEBUFFER_PRESERVE_BYTES: AtomicU32 = AtomicU32::new(0);

// 0 = unused, 1 = implicit configuration created by the Xbox device factory.
// An explicit configuration stores the exclusive upper guest address.
static VM_LIMIT: AtomicU64 = AtomicU64::new(0);
const VM_START: u64 = 1 << 31;

fn address_limit(entries: u32) -> u64 {
    // One directory entry (8 bytes) describes a 2 KiB leaf table covering
    // 16 MiB of guest VA. The kernel rounds down to eight directories.
    let pages = if entries == 0 { 5120 } else { u64::from(entries) * 1024 };
    let directories = (pages * 4096 / 2056) & !7;
    (directories << 24).min(1 << 40)
}

pub(crate) fn device_vm_created() {
    let _ = VM_LIMIT.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
}

pub(crate) fn placement_fits(address: u64, size: u64) -> bool {
    let limit = VM_LIMIT.load(Ordering::Acquire);
    // The console window starts at VM_START, but TuxBox's XMem backs graphics
    // heaps (e.g. GTA's startup heap) at host-chosen addresses below it.
    address.checked_add(size).is_some_and(|end| limit <= 1 || end <= limit)
}

/// The requested preservation size is recorded before the driver call, even
/// when that call fails.
#[unsafe(export_name = "GetFrameBufferPreserveMemorySizeBytes")]
pub extern "system" fn framebuffer_preserve_memory_size_bytes() -> u32 {
    FRAMEBUFFER_PRESERVE_BYTES.load(Ordering::Relaxed)
}

#[path = "pix_driver.rs"]
mod pix_driver;

#[path = "microcode.rs"]
mod microcode;

#[path = "profiling.rs"]
pub(crate) mod profiling;

/// Publish CPU writes to live guest-backed textures on their device copy queues.
#[unsafe(export_name = "D3DFlushEntireCpuCache")]
pub extern "system" fn flush_entire_cpu_cache() {
    crate::device::placement::flush_cpu_writes();
}

/// Configure the process guest GPU address range before the first device.
///
/// # Safety
/// A non-null record must be readable for its 32-byte header and writable at
/// its output fields. Invalid flags and counts are rejected before those fields
/// are read or modified.
#[unsafe(export_name = "D3D12ConfigureVirtualMemoryX")]
pub unsafe extern "system" fn configure_virtual_memory(record: *mut u32) -> HRESULT {
    if record.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: The caller supplies a readable two-DWORD header. The original
    // function rejects unknown flags and more than 33 entries before reading
    // the rest of the record.
    let flags = unsafe { record.read_unaligned() };
    if flags & !1 != 0 {
        return E_INVALIDARG;
    }
    let entries = unsafe { record.add(1).read_unaligned() };
    if entries > 33 {
        return E_INVALIDARG;
    }
    let preserve = unsafe { record.add(2).read_unaligned() };
    FRAMEBUFFER_PRESERVE_BYTES.store(preserve, Ordering::Relaxed);
    // A nonzero first request also needs a dedicated framebuffer allocation.
    let (status, limit) = if preserve != 0 && VM_LIMIT.load(Ordering::Acquire) == 0 {
        (E_FAIL, 0)
    } else {
        let requested = address_limit(entries);
        match VM_LIMIT.compare_exchange(0, requested, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => (HRESULT(0x1000_0000), requested),
            Err(1) => (E_FAIL, address_limit(0)),
            Err(existing) => (E_FAIL, existing),
        }
    };
    let bytes = record.cast::<u8>();
    unsafe {
        bytes.add(16).cast::<u64>().write_unaligned(if limit == 0 { 0 } else { VM_START });
        bytes.add(24).cast::<u64>().write_unaligned(limit);
    }
    status
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryRange {
    address: *mut u8,
    bytes: usize,
}

#[repr(C)]
struct ClearWorkspace {
    commands: *mut c_void,
    command_bytes: u32,
    scratch: *mut c_void,
    scratch_bytes: u32,
}

// In the Series DLL bit 0 selects a synchronous CPU clear. The default mode
// sets up a GPU operation, even when some ranges are cleared on the CPU.
#[unsafe(export_name = "D3D12ZeroMemoryX")]
unsafe extern "system" fn zero_memory(
    count: u32,
    ranges: *const MemoryRange,
    workspace: *const ClearWorkspace,
    flags: u32,
) -> HRESULT {
    if count == 0 {
        return S_OK;
    }
    if ranges.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the guest supplies count readable records, separate from the
    // memory being cleared. Validate the entire batch before changing bytes.
    let ranges = unsafe { core::slice::from_raw_parts(ranges, count as usize) };
    for &MemoryRange { address, bytes } in ranges {
        if !address.addr().is_multiple_of(16)
            || !bytes.is_multiple_of(16)
            || (bytes != 0 && address.is_null())
            || bytes > isize::MAX as usize
            || address.addr().checked_add(bytes).is_none()
        {
            return E_INVALIDARG;
        }
    }
    // SAFETY: a non-null workspace points to the complete 32-byte Xbox record.
    if let Some(work) = unsafe { workspace.as_ref() }
        && (work.commands.is_null()
            || work.command_bytes < 0x10000
            || work.scratch.is_null()
            || work.scratch_bytes < 0x100)
    {
        return E_INVALIDARG;
    }
    if flags & 1 == 0 {
        return E_NOTIMPL;
    }
    for &MemoryRange { address, bytes } in ranges {
        if bytes != 0 {
            // SAFETY: CPU-mode ranges refer to writable guest memory of the
            // given length. Overlapping ranges are cleared in guest order.
            unsafe { address.write_bytes(0, bytes) };
        }
    }
    S_OK
}
