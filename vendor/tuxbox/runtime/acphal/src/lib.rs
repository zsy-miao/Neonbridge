//! Rust provider for the Xbox audio processor heap and AcpHal entry points Frosty imports.

use core::{ffi::c_void, mem::size_of, ptr};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000Eu32 as i32;
const MIN_HEAP_SIZE: usize = 0x1000;
const DEFAULT_ALIGNMENT: usize = 16;
const SHAPE_XMA_COUNT_OFFSET: usize = 0x10;
const SHAPE_XMA_ARRAY_OFFSET: usize = 0x38;
const SHAPE_XMA_CONTEXT_SIZE: usize = 64;
const HAL_VTABLE_SLOTS: usize = 32;

struct ApuHeap {
    base: usize,
    size: usize,
    offset: usize,
}

static APU_HEAP: Mutex<Option<ApuHeap>> = Mutex::new(None);

fn apu_allocate(size: usize, alignment: usize) -> Option<(usize, u32)> {
    if size == 0 || !alignment.is_power_of_two() {
        return None;
    }

    let mut heap = APU_HEAP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let heap = heap.as_mut()?;
    let current = heap.base.checked_add(heap.offset)?;
    let aligned = current.checked_add(alignment.checked_sub(1)?)? & !(alignment - 1);
    let end = aligned.checked_add(size)?;
    let heap_end = heap.base.checked_add(heap.size)?;
    let apu_address = u32::try_from(aligned.checked_sub(heap.base)?).ok()?;
    if end > heap_end {
        return None;
    }
    heap.offset = end.checked_sub(heap.base)?;
    Some((aligned, apu_address))
}

/// Create the host-backed memory region used for APU allocations.
#[unsafe(export_name = "ApuCreateHeap")]
pub extern "system" fn apu_create_heap(size_bytes: u32, _flags: u32) -> i32 {
    let mut heap = APU_HEAP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if heap.is_some() {
        return S_OK;
    }

    let size = (size_bytes as usize).max(MIN_HEAP_SIZE);
    // SAFETY: A null base requests a fresh region; size and protection are valid Win32 values.
    let base = unsafe { VirtualAlloc(ptr::null(), size, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) }
        as usize;
    if base == 0 {
        return E_OUTOFMEMORY;
    }
    *heap = Some(ApuHeap {
        base,
        size,
        offset: 0,
    });
    S_OK
}

/// Allocate a block from the APU heap and return its 32-bit heap-relative address.
///
/// # Safety
/// Each non-null output must point to a writable pointer-sized or 32-bit value, respectively.
#[unsafe(export_name = "ApuAlloc")]
pub unsafe extern "system" fn apu_alloc(
    block_out: *mut *mut c_void,
    address_out: *mut u32,
    size: u32,
    alignment: u32,
) -> i32 {
    if block_out.is_null() {
        return E_INVALIDARG;
    }
    let alignment = (alignment as usize).max(DEFAULT_ALIGNMENT);
    if size == 0 || !alignment.is_power_of_two() {
        return E_INVALIDARG;
    }
    let Some((block, address)) = apu_allocate(size as usize, alignment) else {
        return E_OUTOFMEMORY;
    };
    // SAFETY: The caller's output pointers are required writable by this function's contract.
    unsafe {
        ptr::write_unaligned(block_out, block as *mut c_void);
        if !address_out.is_null() {
            ptr::write_unaligned(address_out, address);
        }
    }
    S_OK
}

fn apu_offset(block: *const c_void, size: usize) -> Option<u32> {
    let heap = APU_HEAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let heap = heap.as_ref()?;
    let offset = (block as usize).checked_sub(heap.base)?;
    if offset >= heap.offset || size > heap.offset - offset {
        return None;
    }
    u32::try_from(offset).ok()
}

#[unsafe(export_name = "ApuIsVirtualAddressValid")]
pub extern "system" fn apu_is_virtual_address_valid(block: *const c_void, size: u32) -> bool {
    apu_offset(block, size as usize).is_some()
}

#[unsafe(export_name = "ApuMapVirtualAddress")]
pub extern "system" fn apu_map_virtual_address(block: *const c_void) -> u32 {
    apu_offset(block, 1).unwrap_or(0)
}

#[unsafe(export_name = "ApuMapApuAddress")]
pub extern "system" fn apu_map_apu_address(address: u32) -> *mut c_void {
    let heap = APU_HEAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(heap) = heap.as_ref() else { return ptr::null_mut() };
    if address as usize >= heap.offset {
        return ptr::null_mut();
    }
    (heap.base + address as usize) as *mut c_void
}

/// Read the free and used byte counts for the single host-backed APU heap.
///
/// # Safety
/// `state_out`, when non-null, must point to at least two writable `u32` values.
#[unsafe(export_name = "ApuHeapGetState")]
pub unsafe extern "system" fn apu_heap_get_state(state_out: *mut u32, _index: u32) -> i32 {
    if state_out.is_null() {
        return E_INVALIDARG;
    }
    let heap = APU_HEAP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (free, used) = match heap.as_ref() {
        Some(heap) => (
            heap.size.saturating_sub(heap.offset).min(u32::MAX as usize) as u32,
            heap.offset.min(u32::MAX as usize) as u32,
        ),
        None => (0, 0),
    };
    // SAFETY: The caller's contract provides two writable `u32` slots.
    unsafe {
        ptr::write_unaligned(state_out, free);
        ptr::write_unaligned(state_out.add(1), used);
    }
    S_OK
}

/// Individual heap blocks are reclaimed when the process tears down its APU heap.
#[unsafe(export_name = "ApuFree")]
pub extern "system" fn apu_free(_block: *mut c_void) -> i32 {
    S_OK
}

/// Allocate the XMA context array Frosty requests in its SHAPE context record.
///
/// # Safety
/// `contexts` must point to at least `0x48` readable and writable bytes.
#[unsafe(export_name = "AcpHalAllocateShapeContexts")]
pub unsafe extern "system" fn acp_hal_allocate_shape_contexts(contexts: *mut u8) -> i32 {
    if contexts.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: The caller promises a readable 0x48-byte SHAPE context record.
    let counts = unsafe {
        [0x00, 0x04, 0x08, 0x0c, SHAPE_XMA_COUNT_OFFSET, 0x14]
            .map(|offset| ptr::read_unaligned(contexts.add(offset).cast::<u32>()))
    };
    let [source, equalizer, filter_volume, dma, xma, pcm] = counts;
    if source != 0 || equalizer != 0 || filter_volume != 0 || dma != 0 || pcm != 0 {
        return E_NOTIMPL;
    }
    if xma == 0 {
        // SAFETY: The caller promises a writable 0x48-byte SHAPE context record.
        unsafe {
            ptr::write_unaligned(
                contexts.add(SHAPE_XMA_ARRAY_OFFSET).cast::<*mut c_void>(),
                ptr::null_mut(),
            );
        }
        return S_OK;
    }

    let Some(bytes) = (xma as usize).checked_mul(SHAPE_XMA_CONTEXT_SIZE) else {
        return E_OUTOFMEMORY;
    };
    let Some((array, _apu_address)) = apu_allocate(bytes, DEFAULT_ALIGNMENT) else {
        return E_OUTOFMEMORY;
    };
    // SAFETY: The caller promises the context record is writable; `array` is in the live APU heap.
    unsafe {
        ptr::write_unaligned(
            contexts.add(SHAPE_XMA_ARRAY_OFFSET).cast::<*mut c_void>(),
            array as *mut c_void,
        );
    }
    S_OK
}

type HalMethod = unsafe extern "system" fn(*mut c_void, usize, usize, usize) -> i32;

unsafe extern "system" fn inert_hal_method(
    _this: *mut c_void,
    _first: usize,
    _second: usize,
    _third: usize,
) -> i32 {
    S_OK
}

#[repr(C)]
struct HalObject {
    vtable: *const HalMethod,
}

static HAL_VTABLE: [HalMethod; HAL_VTABLE_SLOTS] = [inert_hal_method; HAL_VTABLE_SLOTS];
static HAL_OBJECT: OnceLock<usize> = OnceLock::new();

/// Return a process-lifetime inert IAcpHal object; SHAPE DSP is not implemented.
///
/// # Safety
/// `object_out` must point to a writable pointer-sized output.
#[unsafe(export_name = "AcpHalCreate")]
pub unsafe extern "system" fn acp_hal_create(object_out: *mut *mut c_void) -> i32 {
    if object_out.is_null() {
        return E_INVALIDARG;
    }
    let object = *HAL_OBJECT.get_or_init(|| {
        let object = Box::new(HalObject {
            vtable: HAL_VTABLE.as_ptr(),
        });
        Box::leak(object) as *mut HalObject as usize
    });
    // SAFETY: The caller's contract provides a writable pointer-sized output.
    unsafe { ptr::write_unaligned(object_out, object as *mut c_void) };
    S_OK
}

/// Release the inert SHAPE provider state.
///
/// The reference export takes no arguments and returns an HRESULT. This host provider has
/// no DSP device state to release, so successful cleanup is a no-op.
#[unsafe(export_name = "AcpHalReleaseShapeContexts")]
pub extern "system" fn acp_hal_release_shape_contexts() -> i32 {
    S_OK
}

const _: () = assert!(SHAPE_XMA_ARRAY_OFFSET + size_of::<*mut c_void>() == 0x40);
