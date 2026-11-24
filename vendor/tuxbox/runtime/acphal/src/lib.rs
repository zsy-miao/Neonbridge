//! Rust provider for the Xbox audio processor heap and AcpHal entry points Frosty imports.

mod messages;
mod probe;
mod xma;

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

/// One committed region; an APU address is an offset into it. `offset` is
/// the high-water mark. Blocks ApuFree gives back go on a free list of
/// extents below it, merged with free neighbours, and are handed out first
/// fit in address order (Xbox2Windows 64b6d2d3 and f9033a9b, MIT, ported), so
/// while nothing is freed the offsets are those of a bump allocator.
/// TUXBOX_APU_FREE=0 keeps ApuFree a no-op.
struct ApuHeap {
    base: usize,
    size: usize,
    offset: usize,
    /// Free extents below `offset`: start -> length.
    free: std::collections::BTreeMap<usize, usize>,
    /// Live blocks: start -> length.
    used: std::collections::BTreeMap<usize, usize>,
    used_bytes: usize,
}

impl ApuHeap {
    /// First fit in the free list, else past the high-water mark.
    fn take(&mut self, size: usize, alignment: usize) -> Option<usize> {
        let pad_for = |start: usize| (alignment - (self.base + start) % alignment) % alignment;
        let fit = self.free.iter().find_map(|(&start, &len)| {
            let pad = pad_for(start);
            (pad.checked_add(size)? <= len).then_some((start, len, pad))
        });
        let at = if let Some((start, len, pad)) = fit {
            self.free.remove(&start);
            if pad != 0 {
                self.free.insert(start, pad);
            }
            let rest = len - pad - size;
            if rest != 0 {
                self.free.insert(start + pad + size, rest);
            }
            let at = start + pad;
            // Reused memory reads as zero, like fresh pages.
            // SAFETY: [at, at+size) lies inside the committed region and no live block.
            unsafe { ptr::write_bytes((self.base + at) as *mut u8, 0, size) };
            at
        } else {
            let start = self.offset;
            let pad = pad_for(start);
            let end = start.checked_add(pad)?.checked_add(size)?;
            if end > self.size {
                return None;
            }
            if pad != 0 {
                self.give_extent(start, pad);
            }
            self.offset = end;
            start + pad
        };
        self.used.insert(at, size);
        self.used_bytes += size;
        Some(at)
    }

    /// Frees the live block starting at `at`; false for anything else.
    fn give(&mut self, at: usize) -> bool {
        let Some(len) = self.used.remove(&at) else { return false };
        self.used_bytes -= len;
        self.give_extent(at, len);
        true
    }

    fn give_extent(&mut self, mut start: usize, mut len: usize) {
        if let Some((&before, &before_len)) = self.free.range(..start).next_back()
            && before + before_len == start
        {
            self.free.remove(&before);
            start = before;
            len += before_len;
        }
        if let Some(after_len) = self.free.remove(&(start + len)) {
            len += after_len;
        }
        self.free.insert(start, len);
    }
}

fn apu_free_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TUXBOX_APU_FREE").is_none_or(|v| v != "0"))
}

static APU_HEAP: Mutex<Option<ApuHeap>> = Mutex::new(None);

/// Host address of `bytes` at APU address `apu`, when all of it is in the heap.
pub(crate) fn heap_host(apu: u32, bytes: usize) -> Option<usize> {
    let heap = APU_HEAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let heap = heap.as_ref()?;
    let end = (apu as usize).checked_add(bytes)?;
    (end <= heap.size).then(|| heap.base + apu as usize)
}

fn apu_allocate(size: usize, alignment: usize) -> Option<(usize, u32)> {
    if size == 0 || !alignment.is_power_of_two() {
        return None;
    }

    let mut heap = APU_HEAP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let heap = heap.as_mut()?;
    let end = heap.offset.checked_add(alignment)?.checked_add(size)?;
    u32::try_from(end).ok()?;
    let at = heap.take(size, alignment)?;
    Some((heap.base + at, at as u32))
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
        free: Default::default(),
        used: Default::default(),
        used_bytes: 0,
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
            heap.size.saturating_sub(heap.used_bytes).min(u32::MAX as usize) as u32,
            heap.used_bytes.min(u32::MAX as usize) as u32,
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

/// Gives back the live block starting at `block`. Null is a no-op and S_OK;
/// anything else that is not the start of a live block (unknown, inside a
/// block, already freed) is E_INVALIDARG and leaves the heap as it was.
#[unsafe(export_name = "ApuFree")]
pub extern "system" fn apu_free(block: *mut c_void) -> i32 {
    if block.is_null() || !apu_free_on() {
        return S_OK;
    }
    let mut heap = APU_HEAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(heap) = heap.as_mut() else { return E_INVALIDARG };
    match (block as usize).checked_sub(heap.base) {
        Some(at) if heap.give(at) => S_OK,
        _ => E_INVALIDARG,
    }
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
    probe::contexts_allocated(array, xma as usize);
    xma::contexts_allocated(array, xma as usize);
    S_OK
}

type HalMethod = unsafe extern "system" fn(*mut c_void, usize, usize, usize) -> i32;


#[repr(C)]
struct HalObject {
    vtable: *const HalMethod,
}

// Inert methods (S_OK) that count calls per slot for the XMA probe.
macro_rules! hal_methods {
    ($($n:literal),*) => { [$(probe::method::<$n> as HalMethod),*] };
}
type SubmitMethod = unsafe extern "system" fn(*mut c_void, u32, u64, u32, *const c_void, u32) -> i32;
static HAL_VTABLE: [HalMethod; HAL_VTABLE_SLOTS] = {
    let mut v = hal_methods!(0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31);
    // Slot 2 is the coprocessor command submit; it still counts for the probe.
    // SAFETY: the title calls slot 2 with the submit signature; both are system ABI.
    v[2] = unsafe { core::mem::transmute::<SubmitMethod, HalMethod>(xma::submit as SubmitMethod) };
    // Connection and message queue; each still counts for the probe.
    v[0] = messages::connect as HalMethod;
    v[1] = messages::disconnect as HalMethod;
    v[3] = messages::pop_message as HalMethod;
    v[4] = messages::message_count as HalMethod;
    v
};
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
    xma::contexts_released();
    S_OK
}

const _: () = assert!(SHAPE_XMA_ARRAY_OFFSET + size_of::<*mut c_void>() == 0x40);
