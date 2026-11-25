//! Xbox file-to-memory DirectStorage using Windows file handles and RFC 1950 streams.
#![allow(non_snake_case)]
mod abi;
mod queue;

use abi::*;
use core::ffi::c_void;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::os::windows::{
    ffi::OsStringExt,
    fs::OpenOptionsExt,
    io::{AsRawHandle, IntoRawHandle},
};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicI32, Ordering},
};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows_core::{GUID, HRESULT, IUnknown, Interface, implement};

const E_PENDING: HRESULT = HRESULT(0x8000000a_u32 as i32);

#[implement(IFactory2)]
struct Factory;
#[implement(IFile)]
struct StorageFile {
    backing: Arc<FileBacking>,
}

struct FileBacking {
    state: Mutex<FileState>,
    idle: Condvar,
}
struct FileState {
    handle: Option<Arc<File>>,
    requests: usize,
}
struct FileRead {
    file: Arc<File>,
    backing: Arc<FileBacking>,
}
impl FileBacking {
    fn capture(self: &Arc<Self>) -> Result<FileRead, HRESULT> {
        let mut state = lock(&self.state);
        let file = state.handle.clone().ok_or(E_HANDLE)?;
        state.requests += 1;
        Ok(FileRead {
            file,
            backing: Arc::clone(self),
        })
    }
}
impl Drop for FileRead {
    fn drop(&mut self) {
        let mut state = lock(&self.backing.state);
        state.requests -= 1;
        if state.requests == 0 {
            self.backing.idle.notify_all();
        }
    }
}
#[implement(IStatus)]
struct Status {
    values: Arc<[AtomicI32]>,
}

// The singleton carries no mutable state. Its COM reference count is atomic.
unsafe impl Send for IFactory2 {}
unsafe impl Sync for IFactory2 {}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe fn output<I: Interface>(iid: *const GUID, out: *mut *mut c_void) -> Result<(), HRESULT> {
    if out.is_null() {
        return Err(E_POINTER);
    }
    unsafe { out.write(core::ptr::null_mut()) };
    match unsafe { iid.as_ref() } {
        None => Err(E_POINTER),
        Some(id) if *id == I::IID || *id == IUnknown::IID => Ok(()),
        _ => Err(E_NOINTERFACE),
    }
}

/// Creates or obtains the process-wide Xbox storage factory.
///
/// # Safety
/// `iid` and `out` must point to a readable GUID and writable COM output.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DStorageGetFactory(
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    static FACTORY: OnceLock<IFactory2> = OnceLock::new();
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    unsafe { FACTORY.get_or_init(|| Factory.into()).query(iid, out) }
}

impl IFactory_Impl for Factory_Impl {
    unsafe fn CreateQueue(
        &self,
        desc: *const QueueDesc,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT {
        if let Err(hr) = unsafe { output::<IQueue>(iid, out) } {
            return hr;
        }
        let Some(desc) = (unsafe { desc.as_ref() }) else {
            return E_POINTER;
        };
        match unsafe { queue::create(desc, false) } {
            Ok(queue) => unsafe { queue.query(iid, out) },
            Err(hr) => hr,
        }
    }
    unsafe fn OpenFile(
        &self,
        path: *const u16,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT {
        if let Err(hr) = unsafe { output::<IFile>(iid, out) } {
            return hr;
        }
        if path.is_null() {
            return E_POINTER;
        }
        // The caller supplies a NUL-terminated Windows path for the duration of OpenFile.
        let wide = windows::core::PCWSTR(path);
        let path = OsString::from_wide(unsafe { wide.as_wide() });
        match OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(path)
        {
            Ok(file) => {
                {
                    let mut s = storage_stats();
                    s.files += 1;
                    s.peak_files = s.peak_files.max(s.files);
                    s.total_files = s.total_files.saturating_add(1);
                }
                let object: IFile = StorageFile {
                    backing: Arc::new(FileBacking {
                        state: Mutex::new(FileState {
                            handle: Some(Arc::new(file)),
                            requests: 0,
                        }),
                        idle: Condvar::new(),
                    }),
                }
                .into();
                unsafe { out.write(object.into_raw()) };
                S_OK
            }
            Err(error) => HRESULT::from_win32(error.raw_os_error().unwrap_or(1) as u32),
        }
    }
    unsafe fn CreateStatusArray(
        &self,
        capacity: u32,
        _: *const i8,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT {
        if let Err(hr) = unsafe { output::<IStatus>(iid, out) } {
            return hr;
        }
        if capacity == 0 {
            return E_INVALIDARG;
        }
        let mut values = Vec::new();
        if values.try_reserve_exact(capacity as usize).is_err() {
            return E_OUTOFMEMORY;
        }
        values.resize_with(capacity as usize, || AtomicI32::new(S_OK.0));
        let obj: IStatus = Status {
            values: values.into(),
        }
        .into();
        unsafe { out.write(obj.into_raw()) };
        S_OK
    }
    unsafe fn SetCpuAffinity(&self, _: u64) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn SetDebugFlags(&self, _: u32) {}
    unsafe fn SetStagingBufferSize(&self, size: u32) -> HRESULT {
        let mut scratch = lock(&STAGING);
        let mut stats = storage_stats();
        if stats.files != 0 || stats.priorities.iter().any(|p| p.queues != 0) {
            return E_INVALIDARG;
        }
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(size as usize).is_err() {
            return E_OUTOFMEMORY;
        }
        *scratch = Some((size as usize, bytes));
        stats.title_bytes = size;
        stats.peak_title_bytes = stats.peak_title_bytes.max(size);
        S_OK
    }
}

impl IFile_Impl for StorageFile_Impl {
    unsafe fn Close(&self) {
        let mut state = lock(&self.backing.state);
        state.handle.take();
        let _idle = self
            .backing
            .idle
            .wait_while(state, |state| state.requests != 0)
            .unwrap_or_else(|e| e.into_inner());
    }
    unsafe fn GetHandle(&self) -> *mut c_void {
        lock(&self.backing.state)
            .handle
            .as_ref()
            .and_then(|file| file.try_clone().ok())
            .map_or(core::ptr::null_mut(), IntoRawHandle::into_raw_handle)
    }
}

impl IStatus_Impl for Status_Impl {
    unsafe fn IsComplete(&self, index: u32) -> bool {
        self.values
            .get(index as usize)
            .is_some_and(|v| v.load(Ordering::Acquire) != E_PENDING.0)
    }
    unsafe fn GetHResult(&self, index: u32) -> HRESULT {
        self.values
            .get(index as usize)
            .map_or(E_INVALIDARG, |v| HRESULT(v.load(Ordering::Acquire)))
    }
}

static STAGING: Mutex<Option<(usize, Vec<u8>)>> = Mutex::new(None);
fn storage_stats() -> std::sync::MutexGuard<'static, StorageStats> {
    static STATS: std::sync::LazyLock<Mutex<StorageStats>> =
        std::sync::LazyLock::new(Default::default);
    lock(&STATS)
}
impl Drop for FileBacking {
    fn drop(&mut self) {
        storage_stats().files -= 1;
    }
}
impl IFactory2_Impl for Factory_Impl {
    unsafe fn GetStats(&self, stats: *mut StorageStats) {
        if !stats.is_null() {
            unsafe {
                stats.write(*storage_stats());
            }
        }
    }
    unsafe fn CreateQueue1(
        &self,
        desc: *const QueueDesc1,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT {
        if out.is_null() {
            return E_POINTER;
        }
        unsafe {
            out.write(core::ptr::null_mut());
        }
        let (Some(id), Some(desc)) = (unsafe { (iid.as_ref(), desc.as_ref()) }) else {
            return E_POINTER;
        };
        if *id != IQueue2::IID && *id != IQueue::IID && *id != IUnknown::IID {
            return E_NOINTERFACE;
        }
        if desc.options > 1 {
            return E_INVALIDARG;
        }
        match unsafe { queue::create(&desc.desc, desc.options == 1) } {
            Ok(queue) => unsafe { queue.query(iid, out) },
            Err(e) => e,
        }
    }
}
