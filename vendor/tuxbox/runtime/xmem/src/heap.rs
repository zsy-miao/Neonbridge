//! Win32 heap notifications and committed-byte counters.
use core::{ffi::c_void, mem::size_of, ptr};
use std::sync::RwLock;
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NO_MORE_ITEMS, GetLastError, HANDLE, SetLastError,
};
use windows_sys::Win32::System::Memory::{
    HeapAlloc, HeapFree, HeapLock, HeapUnlock, HeapWalk, MEMORY_BASIC_INFORMATION,
    PROCESS_HEAP_ENTRY, VirtualQuery,
};
use windows_sys::Win32::System::SystemServices::{PROCESS_HEAP_ENTRY_BUSY, PROCESS_HEAP_REGION};

type Allocated = unsafe extern "system" fn(HANDLE, u32, usize, *mut c_void);
type Reallocated = unsafe extern "system" fn(HANDLE, u32, *mut c_void, *mut c_void, usize);
type Freed = unsafe extern "system" fn(HANDLE, u32, *mut c_void);

#[derive(Clone, Copy)]
struct Hooks {
    allocated: Option<Allocated>,
    _reallocated: Option<Reallocated>,
    freed: Option<Freed>,
}

static HOOKS: RwLock<Hooks> = RwLock::new(Hooks {
    allocated: None,
    _reallocated: None,
    freed: None,
});

#[unsafe(export_name = "XMemSetWin32HeapTrackingHooks")]
extern "system" fn set_hooks(
    allocated: Option<Allocated>,
    reallocated: Option<Reallocated>,
    freed: Option<Freed>,
) {
    *HOOKS.write().unwrap_or_else(|e| e.into_inner()) = Hooks {
        allocated,
        _reallocated: reallocated,
        freed,
    };
}

pub(super) fn allocate(heap: HANDLE, bytes: usize) -> *mut c_void {
    let p = unsafe { HeapAlloc(heap, 0, bytes) };
    let callback = HOOKS.read().unwrap_or_else(|e| e.into_inner()).allocated;
    if let Some(callback) = callback {
        unsafe { callback(heap, 0, bytes, p) };
    }
    p
}

pub(super) unsafe fn free(heap: HANDLE, p: *mut c_void) {
    unsafe { HeapFree(heap, 0, p) };
    let callback = HOOKS.read().unwrap_or_else(|e| e.into_inner()).freed;
    if let Some(callback) = callback {
        unsafe { callback(heap, 0, p) };
    }
}

#[repr(C)]
struct ExtendedQuery {
    process: HANDLE,
    heap: HANDLE,
    level: u32,
    callback: unsafe extern "system" fn(*const u8, *mut NativeStats) -> i32,
    context: *mut NativeStats,
    information: [usize; 6],
}

#[derive(Default)]
struct NativeStats {
    bytes: [usize; 2],
    received: bool,
}

unsafe extern "system" fn segment_counters(item: *const u8, out: *mut NativeStats) -> i32 {
    // The callback item starts with Level and Size. SegmentCommitSize and
    // LargeAllocCommitSize are in its segment-heap statistics extension.
    if unsafe { item.cast::<u32>().read_unaligned() } == 0x8000_0000
        && unsafe { item.add(8).cast::<usize>().read_unaligned() } >= 0xa8
    {
        unsafe {
            (*out).bytes = [
                item.add(0x80).cast::<usize>().read_unaligned(),
                item.add(0xa0).cast::<usize>().read_unaligned(),
            ];
            (*out).received = true;
        }
    }
    0
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlQueryHeapInformation(
        heap: HANDLE,
        class: u32,
        buffer: *mut ExtendedQuery,
        size: usize,
        needed: *mut usize,
    ) -> i32;
}

#[unsafe(export_name = "XMemGetWin32HeapStatistics")]
unsafe extern "system" fn statistics(heap: HANDLE, out: *mut [usize; 2]) -> i32 {
    if heap.is_null() || out.is_null() {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return 0;
    }
    let previous_error = unsafe { GetLastError() };
    let mut native = NativeStats::default();
    let mut query = ExtendedQuery {
        process: unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() },
        heap,
        level: 0x8000_0000,
        callback: segment_counters,
        context: &raw mut native,
        information: [0; 6],
    };
    // On Windows this is the same query and callback shape as the Xbox DLL.
    // Wine can leave the extended query unimplemented; its regular heap walk
    // provides region commits and separate large virtual blocks.
    let status = unsafe {
        RtlQueryHeapInformation(
            ptr::null_mut(),
            2,
            &mut query,
            size_of::<ExtendedQuery>(),
            ptr::null_mut(),
        )
    };
    if status >= 0 && native.received {
        unsafe { out.write_unaligned(native.bytes) };
        return 1;
    }
    if unsafe { HeapLock(heap) } == 0 {
        return 0;
    }
    let mut entry = PROCESS_HEAP_ENTRY::default();
    let mut totals = [0; 2];
    let error = loop {
        if unsafe { HeapWalk(heap, &mut entry) } == 0 {
            let error = unsafe { GetLastError() };
            break if error == ERROR_NO_MORE_ITEMS {
                0
            } else {
                error
            };
        }
        if u32::from(entry.wFlags) & PROCESS_HEAP_REGION != 0 {
            totals[0] += unsafe { entry.Anonymous.Region.dwCommittedSize } as usize;
        } else if u32::from(entry.wFlags) & PROCESS_HEAP_ENTRY_BUSY != 0 && entry.iRegionIndex >= 64
        {
            // Large heap blocks have a separate virtual allocation. Include the
            // committed prefix and allocator headers in its backing size.
            let mut info = MEMORY_BASIC_INFORMATION::default();
            if unsafe {
                VirtualQuery(
                    entry.lpData,
                    &mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            } == 0
            {
                break unsafe { GetLastError() };
            }
            totals[1] += info.RegionSize;
        }
    };
    unsafe { HeapUnlock(heap) };
    if error != 0 {
        unsafe { SetLastError(error) };
        return 0;
    }
    unsafe {
        ptr::write_unaligned(out, totals);
        SetLastError(previous_error);
    }
    1
}
