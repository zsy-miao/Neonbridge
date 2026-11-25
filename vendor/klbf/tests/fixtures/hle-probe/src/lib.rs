//! A stand-in for a title, written by us. It imports from DLLs that do not
//! exist on a PC, which is exactly the situation victory-loader handles.
//! Loaded normally, this DLL would fail with "module not found".

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const DLL_THREAD_ATTACH: u32 = 2;

#[link(name = "xgameruntime", kind = "raw-dylib")]
unsafe extern "system" {
    fn XGameRuntimeInitialize() -> i32;
}

#[link(name = "victory-probe-missing", kind = "raw-dylib")]
unsafe extern "system" {
    fn VIctoryProbeNotImplemented(x: u64) -> u64;
}

// A DLL the "title" ships; the test supplies a renamed copy of version.dll.
#[link(name = "victory-probe-title", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(path: *const u16, handle: *mut u32) -> u32;
}

// Delay-loaded (see build.rs): bound through the delay-load table.
#[link(name = "victory-probe-delayed", kind = "raw-dylib")]
unsafe extern "system" {
    fn VIctoryProbeDelayed() -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
    fn GetCurrentProcessId() -> u32;
}

/// Console-only API, answered by the resolver's HLE implementation.
#[unsafe(no_mangle)]
pub extern "system" fn probe_xgameruntime_init() -> i32 {
    unsafe { XGameRuntimeInitialize() }
}

/// Exists on the host too, but the resolver overrides it.
#[unsafe(no_mangle)]
pub extern "system" fn probe_tick() -> u32 {
    unsafe { GetTickCount() }
}

/// Exists on the host and is passed through to it.
#[unsafe(no_mangle)]
pub extern "system" fn probe_pid() -> u32 {
    unsafe { GetCurrentProcessId() }
}

/// An export of a title-shipped DLL: the version-info size of kernel32.dll.
#[unsafe(no_mangle)]
pub extern "system" fn probe_title() -> u32 {
    let path: Vec<u16> = r"C:\Windows\System32\kernel32.dll"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut handle = 0;
    unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut handle) }
}

/// A delay-loaded console API.
#[unsafe(no_mangle)]
pub extern "system" fn probe_delayed() -> u32 {
    unsafe { VIctoryProbeDelayed() }
}

/// Nobody implements this one: it must land in an unimplemented-import stub.
#[unsafe(no_mangle)]
pub extern "system" fn probe_missing() -> u64 {
    unsafe { VIctoryProbeNotImplemented(42) }
}

static TLS_THREAD_STARTS: AtomicU32 = AtomicU32::new(0);
static MAIN_THREAD_STARTS: AtomicU32 = AtomicU32::new(0);

/// A TLS callback, as titles have: counts thread starts.
unsafe extern "system" fn on_thread(_: *mut c_void, reason: u32, _: *mut c_void) {
    if reason == DLL_THREAD_ATTACH {
        TLS_THREAD_STARTS.fetch_add(1, Ordering::Relaxed);
    }
}

#[used]
#[unsafe(link_section = ".CRT$XLB")]
static ON_THREAD: unsafe extern "system" fn(*mut c_void, u32, *mut c_void) = on_thread;

/// Called by the CRT's DLL entry point: counts thread starts too, and calls
/// an import the resolver overrides, as title code calls our runtime.
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == DLL_THREAD_ATTACH {
        MAIN_THREAD_STARTS.fetch_add(1, Ordering::Relaxed);
        unsafe { GetTickCount() };
    }
    if reason == DLL_PROCESS_DETACH {
        DETACHES.fetch_add(1, Ordering::Relaxed);
    }
    1
}

const DLL_PROCESS_DETACH: u32 = 0;
static DETACHES: AtomicU32 = AtomicU32::new(0);

/// `DLL_PROCESS_DETACH` notices seen by `DllMain`.
#[unsafe(no_mangle)]
pub extern "system" fn probe_detaches() -> u32 {
    DETACHES.load(Ordering::Relaxed)
}

/// Thread starts seen by the TLS callback (high half) and `DllMain` (low).
#[unsafe(no_mangle)]
pub extern "system" fn probe_thread_starts() -> u64 {
    // SAFETY: reading our own static keeps the linker from dropping it.
    unsafe { std::ptr::from_ref(&ON_THREAD).read_volatile() };
    u64::from(TLS_THREAD_STARTS.load(Ordering::Relaxed)) << 32
        | u64::from(MAIN_THREAD_STARTS.load(Ordering::Relaxed))
}
