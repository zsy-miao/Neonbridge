//! Process-wide allocator policies selected by XMemSetOption.
use core::{
    ffi::{CStr, c_void},
    mem::MaybeUninit,
};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use windows_sys::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, FreeLibrary,
};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleExA, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub(super) static VERIFY: AtomicBool = AtomicBool::new(false);
pub(super) static TAG_DIRECT: AtomicBool = AtomicBool::new(false);
pub(super) static AUTO_WATCH: AtomicBool = AtomicBool::new(true);

/// The Xbox runtime enables automatic watching for a graphics reservation
/// only with driver support or access to a tools partition. The desktop
/// retail profile has no tools partition or corresponding graphics driver.
pub(super) fn auto_watch_available() -> bool {
    if !AUTO_WATCH.load(Ordering::Acquire) {
        return false;
    }
    let mut module = core::ptr::null_mut();
    if unsafe { GetModuleHandleExA(0, c"xbthunk.dll".as_ptr().cast(), &raw mut module) } == 0 {
        return false;
    }
    let available = unsafe {
        GetProcAddress(module, c"XbGetPartitionHandleAndId".as_ptr().cast()).is_some_and(|f| {
            let query: unsafe extern "system" fn(u32, *mut *mut c_void, *mut u32) -> u8 =
                std::mem::transmute(f);
            query(2, core::ptr::null_mut(), core::ptr::null_mut()) != 0
        })
    };
    unsafe {
        FreeLibrary(module);
    }
    available
}
const DISABLE_DEMOTION: u8 = 1;
const POOLS_INITIALIZED: u8 = 2;
static PAGE_POLICY: AtomicU8 = AtomicU8::new(0);

fn report(error: i32, message: &CStr) {
    // The game runtime may be absent for a standalone guest. Preserve its
    // diagnostics in OutputDebugString when no runtime reporter is loaded.
    let runtime = unsafe { LoadLibraryA(c"xgameruntime.dll".as_ptr().cast()) };
    if !runtime.is_null() {
        if let Some(f) = unsafe { GetProcAddress(runtime, c"XErrorReport".as_ptr().cast()) } {
            let f: unsafe extern "system" fn(i32, *const i8) = unsafe { std::mem::transmute(f) };
            unsafe {
                f(error, message.as_ptr());
                FreeLibrary(runtime);
            };
            return;
        }
        unsafe {
            FreeLibrary(runtime);
        }
    }
    unsafe { OutputDebugStringA(message.as_ptr().cast()) };
}

pub(super) fn realloc_notifications(value: u32) {
    // Win32 has no process-wide heap callback switch. If the process provides
    // the Xbox heap service, route this option to its registered notification path.
    let mut module = core::ptr::null_mut();
    if unsafe { GetModuleHandleExA(0, c"xbthunk.dll".as_ptr().cast(), &raw mut module) } != 0 {
        if let Some(f) = unsafe {
            GetProcAddress(
                module,
                c"XbSetCallHeapFreeCallbackForRealloc".as_ptr().cast(),
            )
        } {
            let f: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(f) };
            unsafe {
                f(value);
                FreeLibrary(module);
            };
            return;
        }
        unsafe {
            FreeLibrary(module);
        }
    }
    // Desktop XMem has no heap-reallocation export of its own.
    unsafe {
        windows_sys::Win32::Foundation::SetLastError(
            windows_sys::Win32::Foundation::ERROR_NOT_SUPPORTED,
        )
    };
}

pub(super) fn set_page_policy(disable_demotion: bool) {
    if PAGE_POLICY
        .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
            (state & POOLS_INITIALIZED == 0).then_some(u8::from(disable_demotion))
        })
        .is_err()
    {
        report(E_NOTIMPL, c"XMem: page-size demotion policy is fixed after the first allocation pool is initialized.");
    }
}

pub(super) fn initialize_pool() {
    PAGE_POLICY.fetch_or(POOLS_INITIALIZED, Ordering::AcqRel);
}

pub(super) fn can_demote_pages() -> bool {
    PAGE_POLICY.load(Ordering::Acquire) & DISABLE_DEMOTION == 0
}

pub(super) fn validate(p: *mut c_void, size: usize, attr: u64) {
    if p.is_null() {
        report(E_OUTOFMEMORY, c"XMem: allocator returned NULL.");
        return;
    }
    let page_class = (attr >> 14) & 3;
    let kind = (attr >> 29) & 15;
    let exponent = (attr >> 24) & 31;
    if page_class == 3 {
        report(E_INVALIDARG, c"XMem: invalid backing page size.");
    }
    if kind > 12 {
        report(E_INVALIDARG, c"XMem: invalid allocation memory type.");
    }
    if exponent > 17 {
        report(E_INVALIDARG, c"XMem: invalid allocation alignment.");
    }
    if !p.addr().is_multiple_of(1 << exponent) {
        report(
            E_POINTER,
            c"XMem: allocation does not satisfy the requested alignment.",
        );
    }
    if attr & (1 << 33) != 0
        && (p.addr() as u64).saturating_add(size.saturating_sub(1) as u64) >= 1 << 32
    {
        report(
            E_POINTER,
            c"XMem: allocation exceeds the requested 4 GiB boundary.",
        );
    }
    let mut covered = 0;
    while covered < size {
        let mut info = MaybeUninit::<super::virtual_memory::VirtualInfo>::uninit();
        let address = p.addr().saturating_add(covered);
        if unsafe {
            super::virtual_memory::virtual_query(
                GetCurrentProcess(),
                address as *const _,
                info.as_mut_ptr(),
            )
        } == 0
        {
            report(E_FAIL, c"XMem: unable to query allocation backing.");
            break;
        }
        let info = unsafe { info.assume_init() }.memory;
        let pages = info.State & 0x2040_0000;
        if info.State & !0x2060_0000 != MEM_COMMIT {
            report(E_FAIL, c"XMem: allocation contains uncommitted pages.");
            break;
        }
        if kind == 0 && info.Protect != PAGE_READWRITE {
            report(E_FAIL, c"XMem: CPU allocation requires writable pages.");
            break;
        }
        if (page_class == 2 && matches!(pages, 0 | 0x2040_0000)) || (page_class == 1 && pages == 0)
        {
            report(E_FAIL, c"XMem: backing pages are smaller than requested.");
            break;
        }
        let end = info.BaseAddress.addr().saturating_add(info.RegionSize);
        if end <= address {
            break;
        }
        covered = end - p.addr();
    }
}
