//! Xbox DirectStorage interfaces and x64 records used by the file-to-memory path.
use core::ffi::c_void;
use windows_core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, interface};

#[interface("fb3b54ac-bf82-463a-b777-73195837864d")]
pub unsafe trait IFactory: IUnknown {
    fn CreateQueue(
        &self,
        desc: *const QueueDesc,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT;
    fn OpenFile(&self, path: *const u16, iid: *const GUID, out: *mut *mut c_void) -> HRESULT;
    fn CreateStatusArray(
        &self,
        capacity: u32,
        name: *const i8,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT;
    fn SetCpuAffinity(&self, mask: u64) -> HRESULT;
    fn SetDebugFlags(&self, flags: u32);
    fn SetStagingBufferSize(&self, size: u32) -> HRESULT;
}

#[interface("43704f9f-27c6-4a9f-8815-d16527894df4")]
pub unsafe trait IFile: IUnknown {
    fn Close(&self);
    fn GetHandle(&self) -> *mut c_void;
}

#[interface("cd66d752-2786-44f7-9070-e2a6fdad3f1b")]
pub unsafe trait IStatus: IUnknown {
    fn IsComplete(&self, index: u32) -> bool;
    fn GetHResult(&self, index: u32) -> HRESULT;
}

#[interface("a3f6649a-8c2c-4b3a-8b6e-10186a04b9bc")]
pub unsafe trait IQueue: IUnknown {
    fn EnqueueRequest(&self, req: *const Request);
    fn EnqueueStatus(&self, array: *mut c_void, index: u32);
    fn EnqueueSignal(&self, fence: *mut c_void, value: u64);
    fn Submit(&self);
    fn CancelRequestsWithTag(&self, mask: u64, value: u64);
    fn Close(&self);
    fn GetErrorEvent(&self) -> *mut c_void;
    fn RetrieveErrorRecord(&self, record: *mut ErrorRecord);
    fn Query(&self, info: *mut QueueInfo);
}

#[repr(C)]
pub struct QueueDesc {
    pub source_type: u64,
    pub capacity: u16,
    pub priority: i8,
    pub name: *const i8,
}

#[repr(C)]
pub struct QueueInfo {
    pub desc: QueueDesc,
    pub empty: u16,
    pub until_submit: u16,
}

// Only the file source and linear destination fields are interpreted. The
// second destination word is reserved for a physical-page offset.
#[repr(C)]
pub struct Request {
    pub options: u64,
    pub destination: *mut u8,
    pub page_offset: u16,
    pub reserved: [u8; 6],
    pub destination_size: u32,
    pub source: *mut c_void,
    pub offset: u64,
    pub size: u32,
    pub intermediate_size: u32,
    pub tag: u64,
}

// The request variant is the largest member of the Xbox error record union.
#[repr(C)]
#[derive(Clone)]
pub struct ErrorRecord {
    pub count: u32,
    pub padding: u32, // FirstFailure is aligned to eight bytes.
    pub hr: i32,
    pub kind: u32,
    pub options: u64,
    pub destination: usize,
    pub destination_size: u32,
    pub file_name: [u16; 260],
    pub offset: u64,
    pub source: usize,
    pub size: u32,
    pub tag: u64,
    pub name: [u8; 64],
}

const _: () = {
    assert!(size_of::<QueueDesc>() == 24);
    assert!(size_of::<ErrorRecord>() == 0x290);
    assert!(core::mem::offset_of!(Request, destination_size) == 24);
    assert!(core::mem::offset_of!(Request, source) == 32);
    assert!(core::mem::offset_of!(Request, tag) == 56);
};

#[interface("62959d11-d45f-4884-a0a3-f7862ea243ae")]
pub unsafe trait IFactory2: IFactory {
    fn GetStats(&self, stats: *mut StorageStats);
    fn CreateQueue1(
        &self,
        desc: *const QueueDesc1,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT;
}

#[interface("3c02cb0c-c050-4195-99a1-a1ce94ba33c5")]
pub unsafe trait IQueue2: IQueue {
    fn EnqueueSetEvent(&self, event: *mut c_void);
    fn Query1(&self, info: *mut QueueInfo1);
}

#[repr(C)]
pub struct QueueDesc1 {
    pub desc: QueueDesc,
    pub options: u64,
}

#[repr(C)]
pub struct QueueInfo1 {
    pub desc: QueueDesc1,
    pub empty: u16,
    pub until_submit: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PriorityStats {
    pub queues: u32,
    pub peak_queues: u32,
    pub total_queues: u32,
    pub statuses: u32,
    pub fences: u32,
    pub pending: u32,
    pub requests: u32,
    pub source_bytes: u64,
    pub destination_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct StorageStats {
    pub priorities: [PriorityStats; 4],
    pub disks: u8,
    pub peak_disks: u8,
    pub total_disks: u32,
    pub pending_reads: u32,
    pub total_reads: u32,
    pub retries: u32,
    pub read_errors: u32,
    pub pending_decodes: u32,
    pub total_decodes: u32,
    pub decode_errors: u32,
    pub stalls: u32,
    pub files: u32,
    pub peak_files: u32,
    pub total_files: u32,
    pub title_bytes: u32,
    pub peak_title_bytes: u32,
    pub errors: u32,
}
