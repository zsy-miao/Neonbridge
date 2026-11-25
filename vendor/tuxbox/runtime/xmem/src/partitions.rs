//! Desktop title memory shares the process partition. CPU physical-page
//! allocations and working-set queries observe memory relinquished to the OS.
use core::{mem::size_of, ptr};
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_OBJECT_NOT_FOUND, HANDLE,
    OBJ_CASE_INSENSITIVE, OBJ_INHERIT, RtlNtStatusToDosError, SetLastError, UNICODE_STRING,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};

const PAGE_SHIFT: u32 = 12;
pub(super) const TITLE_PAGE_STRIDE: u64 = 65536 >> PAGE_SHIFT;

fn failure(e: u32) -> i32 {
    unsafe { SetLastError(e) };
    0
}

/// The Xbox driver returns the first title PFN and a topology boundary in
/// 4 KiB units. They coincide in a single memory pool. TuxBox reserves the
/// null 64 KiB region and starts its first CPU physical token at that boundary.
#[unsafe(export_name = "XMemGetPhysicalMemoryLayout")]
unsafe extern "system" fn physical_layout(out: *mut [u64; 2]) -> i32 {
    if out.is_null() {
        return failure(ERROR_INVALID_PARAMETER);
    }
    unsafe { out.write_unaligned([TITLE_PAGE_STRIDE; 2]) };
    1
}

#[unsafe(export_name = "XMemTransferMemory")]
extern "system" fn transfer(direction: u32, size: usize) -> i32 {
    if matches!(direction, 1 | 2) {
        return failure(ERROR_NOT_SUPPORTED);
    }
    if size == 0 {
        return 1;
    }
    if direction != 0 {
        return failure(ERROR_INVALID_PARAMETER);
    }
    let Some(bytes) = size.checked_next_multiple_of(2 << 20) else {
        return failure(windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY);
    };
    match super::physical_pages::relinquish(bytes as u64) {
        Ok(()) => 1,
        Err(e) => failure(e),
    }
}

pub(super) fn title_partition_id() -> Option<u16> {
    let mut region = MEMORY_BASIC_INFORMATION::default();
    let address = ptr::addr_of!(region).cast();
    let bytes = unsafe { VirtualQuery(address, &mut region, size_of::<MEMORY_BASIC_INFORMATION>()) };
    (bytes != 0).then_some(region.PartitionId)
}

/// A requested handle must be an actual memory partition handle. Wine hosts
/// without this object type can still answer ID and presence queries.
#[unsafe(export_name = "XMemGetPartitionHandleAndId")]
unsafe extern "system" fn partition_info(kind: u32, handle: *mut HANDLE, id: *mut u32) -> u8 {
    if kind > 2 {
        return failure(ERROR_INVALID_PARAMETER) as u8;
    }
    if kind == 2 {
        return failure(ERROR_OBJECT_NOT_FOUND) as u8;
    }
    let partition_id = if kind == 0 {
        0
    } else if let Some(id) = title_partition_id() {
        u32::from(id)
    } else {
        return 0;
    };
    if !handle.is_null() {
        // Resolve at runtime because stock Proton does not export NtOpenPartition.
        type OpenPartition =
            unsafe extern "system" fn(*mut HANDLE, u32, *const OBJECT_ATTRIBUTES) -> i32;
        let ntdll = unsafe { GetModuleHandleW(windows_sys::w!("ntdll.dll")) };
        let Some(open) = (unsafe { GetProcAddress(ntdll, c"NtOpenPartition".as_ptr().cast()) })
        else {
            return failure(ERROR_OBJECT_NOT_FOUND) as u8;
        };
        let mut name: Vec<u16> = format!("\\KernelObjects\\MemoryPartition{partition_id}")
            .encode_utf16()
            .collect();
        let len = (name.len() * 2) as u16;
        let path = UNICODE_STRING {
            Length: len,
            MaximumLength: len,
            Buffer: name.as_mut_ptr(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            ObjectName: &path,
            Attributes: OBJ_CASE_INSENSITIVE | OBJ_INHERIT,
            ..Default::default()
        };
        let mut result = ptr::null_mut();
        let open: OpenPartition = unsafe { core::mem::transmute(open) };
        // Query access only. The host owns the underlying memory partition.
        let status = unsafe { open(&mut result, 1, &attributes) };
        if status < 0 {
            return failure(unsafe { RtlNtStatusToDosError(status) }) as u8;
        }
        unsafe { handle.write_unaligned(result) };
    }
    if !id.is_null() {
        unsafe { id.write_unaligned(partition_id) };
    }
    1
}
