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
const E_FAIL: i32 = 0x8000_4005u32 as i32;
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
    /// Reserved only; blocks are committed as they are handed out.
    lazy: bool,
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

fn switch_on(name: &str) -> bool {
    std::env::var_os(name).is_none_or(|v| v != "0")
}

/// Copied from ztrx victory-hle audio.rs:66,151 (GPL-3.0, credit to its
/// authors): device addresses start at APU_BASE, as 0 means "none" to
/// titles. Difference: ours is a bias on the heap offset (one committed
/// region) where the leader's free list starts at it.
/// TUXBOX_APU_BASE=0: an APU address is the bare offset, the first block 0.
const APU_BASE: usize = 0x0001_0000;
fn apu_base() -> usize {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| switch_on("TUXBOX_APU_BASE")) { APU_BASE } else { 0 }
}

/// Host address of `bytes` at APU address `apu`, when all of it is in the heap.
pub(crate) fn heap_host(apu: u32, bytes: usize) -> Option<usize> {
    let heap = APU_HEAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let heap = heap.as_ref()?;
    // TUXBOX_APU_BASE: unbias (see apu_base).
    let apu = (apu as usize).checked_sub(apu_base())?;
    let end = apu.checked_add(bytes)?;
    (end <= heap.size).then(|| heap.base + apu)
}

fn apu_allocate(size: usize, alignment: usize) -> Option<(usize, u32)> {
    if size == 0 || !alignment.is_power_of_two() {
        return None;
    }

    let mut heap = APU_HEAP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let heap = heap.as_mut()?;
    let end = heap.offset.checked_add(alignment)?.checked_add(size)?.checked_add(apu_base())?;
    u32::try_from(end).ok()?;
    let at = heap.take(size, alignment)?;
    if heap.lazy {
        // TUXBOX_APU_CREATE_STRICT: commit on demand in the reserved range.
        // SAFETY: [base+at, +size) lies inside our reservation.
        if unsafe { VirtualAlloc((heap.base + at) as *const c_void, size, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
            heap.give(at);
            return None;
        }
    }
    Some((heap.base + at, (at + apu_base()) as u32))
}

/// Create the host-backed memory region used for APU allocations.
#[unsafe(export_name = "ApuCreateHeap")]
pub extern "system" fn apu_create_heap(size_bytes: u32, _flags: u32) -> i32 {
    let mut heap = APU_HEAP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Copied from ztrx victory-hle audio.rs:142-155 (GPL-3.0, credit to its
    // authors): cached 0 is E_INVALIDARG, a second call ALREADY_ALLOCATED,
    // and the sizes are no cap. Difference: ours is one region, so the whole
    // 32-bit device space is reserved and blocks are committed as they are
    // handed out. TUXBOX_APU_CREATE_STRICT=0: a repeat is S_OK and the
    // region is the size asked for, committed up front, as before.
    const ALREADY_ALLOCATED: i32 = 0x8ACA_0001_u32 as i32;
    let strict = create_strict();
    if strict && size_bytes == 0 {
        return E_INVALIDARG;
    }
    if heap.is_some() {
        return if strict { ALREADY_ALLOCATED } else { S_OK };
    }

    let size = if strict { u32::MAX as usize - APU_BASE } else { (size_bytes as usize).max(MIN_HEAP_SIZE) };
    let flags = if strict { MEM_RESERVE } else { MEM_RESERVE | MEM_COMMIT };
    // SAFETY: A null base requests a fresh region; size and protection are valid Win32 values.
    let base = unsafe { VirtualAlloc(ptr::null(), size, flags, PAGE_READWRITE) }
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
        lazy: strict,
    });
    S_OK
}

fn create_strict() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| switch_on("TUXBOX_APU_CREATE_STRICT"))
}

fn align_strict() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| switch_on("TUXBOX_APU_ALIGN_STRICT"))
}

fn valid_live() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| switch_on("TUXBOX_APU_VALID_LIVE"))
}

fn shape_arrays() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| switch_on("TUXBOX_SHAPE_ARRAYS"))
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
    // Copied from ztrx victory-hle audio.rs:194-212 (GPL-3.0, credit to its
    // authors): the alignment must be a power of two of at least 4.
    // Difference: blocks still align to at least 16 inside our region.
    // TUXBOX_APU_ALIGN_STRICT=0: any alignment below 16 is taken as 16.
    if align_strict() && (alignment < 4 || !alignment.is_power_of_two()) {
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
    u32::try_from(offset + apu_base()).ok()
}

#[unsafe(export_name = "ApuIsVirtualAddressValid")]
pub extern "system" fn apu_is_virtual_address_valid(block: *const c_void, size: u32) -> bool {
    // Copied from ztrx victory-hle audio.rs:162-171,225-228 (GPL-3.0, credit
    // to its authors): arg 2 is an alignment, and only a byte inside a live
    // block is valid (its device address a multiple of it). Difference: the
    // live blocks are our heap's `used` map. TUXBOX_APU_VALID_LIVE=0: arg 2
    // is a size and anything below the high-water mark is valid, as before.
    if valid_live() {
        let align = size;
        let heap = APU_HEAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(heap) = heap.as_ref() else { return false };
        let Some(offset) = (block as usize).checked_sub(heap.base) else { return false };
        let live = matches!(heap.used.range(..=offset).next_back(),
            Some((&start, &len)) if offset < start + len.max(1));
        let apu = offset + apu_base();
        return live && apu != 0 && (align == 0 || apu.is_multiple_of(align as usize));
    }
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
    // TUXBOX_APU_BASE: unbias (see apu_base).
    let Some(address) = (address as usize).checked_sub(apu_base()) else { return ptr::null_mut() };
    if address >= heap.offset {
        return ptr::null_mut();
    }
    (heap.base + address) as *mut c_void
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
    if shape_arrays() {
        return unsafe { allocate_shape_arrays(contexts) };
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

/// Copied from ztrx victory-hle audio.rs:234-289 (GPL-3.0, credit to its
/// authors): six counts, then six array pointers @0x18, then six device
/// addresses @0x48; each array is count * CONTEXT_BYTES, 256-aligned. A
/// second allocate is E_FAIL. Difference: arrays come from our APU heap
/// (zeroed) and the XMA array goes to our probe and decoder.
/// TUXBOX_SHAPE_ARRAYS=0: XMA only, other counts E_NOTIMPL, as before.
const CONTEXT_BYTES: [usize; 6] = [32, 256, 256, 16, 64, 256];
static SHAPE: Mutex<Option<[usize; 6]>> = Mutex::new(None);

fn free_arrays(arrays: &[usize; 6]) {
    let mut heap = APU_HEAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(heap) = heap.as_mut() else { return };
    for &p in arrays.iter().filter(|&&p| p != 0) {
        let at = p - heap.base;
        heap.give(at);
    }
}

/// # Safety
/// `contexts` must point to at least `0x60` readable and writable bytes.
unsafe fn allocate_shape_arrays(contexts: *mut u8) -> i32 {
    let mut allocated = SHAPE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if allocated.is_some() {
        return E_FAIL;
    }
    // SAFETY: the title passes its 0x60-byte request.
    let counts: [u32; 6] = core::array::from_fn(|i| unsafe { ptr::read_unaligned(contexts.add(i * 4).cast::<u32>()) });
    let mut arrays = [0usize; 6];
    let mut addresses = [0u32; 6];
    for i in 0..6 {
        if counts[i] == 0 {
            continue;
        }
        let Some((ptr, apu)) = (counts[i] as usize).checked_mul(CONTEXT_BYTES[i]).and_then(|n| apu_allocate(n, 256)) else {
            free_arrays(&arrays);
            return E_OUTOFMEMORY;
        };
        (arrays[i], addresses[i]) = (ptr, apu);
    }
    // SAFETY: as above.
    unsafe {
        for i in 0..6 {
            ptr::write_unaligned(contexts.add(0x18 + i * 8).cast::<usize>(), arrays[i]);
            ptr::write_unaligned(contexts.add(0x48 + i * 4).cast::<u32>(), addresses[i]);
        }
    }
    *allocated = Some(arrays);
    if counts[4] != 0 {
        probe::contexts_allocated(arrays[4], counts[4] as usize);
        xma::contexts_allocated(arrays[4], counts[4] as usize);
    }
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
    messages::object_created();
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
    // TUXBOX_SHAPE_ARRAYS: the arrays are freed; nothing allocated is E_FAIL
    // (ztrx victory-hle audio.rs:280-289, GPL-3.0, credit to its authors).
    if shape_arrays() {
        return match SHAPE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            Some(arrays) => {
                free_arrays(&arrays);
                S_OK
            }
            None => E_FAIL,
        };
    }
    S_OK
}

const _: () = assert!(SHAPE_XMA_ARRAY_OFFSET + size_of::<*mut c_void>() == 0x40);
