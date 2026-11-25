//! Guest device-factory ABI for d3d12_xs.dll.

mod big_stack;
mod clwb;
mod clzero;
mod insertq;
mod mwaitx;
mod commands;
mod compile_gate;
mod device;
mod diagnostics;
mod exports;
mod failures;
mod guids;
mod present;
mod private_driver;
mod progress;
mod proxy;
mod survey;

// Guest vtables mix signatures. Each entry is called by its x64 ABI slot.
macro_rules! slot {
    ($method:expr) => {
        unsafe { core::mem::transmute::<*const (), $crate::proxy::Slot>($method as *const ()) }
    };
}
pub(crate) use slot;

use core::ffi::c_void;
use core::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER};
use windows::core::{GUID, HRESULT};

// Release/default record supplied by the 0x20002 caller.
const PROFILE_20002: [u32; 16] = [
    0x20002,
    0,
    u32::MAX,
    0,
    0,
    0,
    u32::MAX,
    u32::MAX,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];
// The 0x20004 wrapper sets the SDK version and three default-size fields.
// Frosty's direct Xbox factory call also sets word 9 to one.
const PROFILE_20004_DEFAULT: [u32; 18] = [
    0x20004,
    0,
    u32::MAX,
    0,
    0,
    0,
    u32::MAX,
    u32::MAX,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];
const PROCESS_DEBUG_FLAGS_MASK: u32 = 0x83f0_0001;
static PROCESS_DEBUG_FLAGS: AtomicU32 = AtomicU32::new(0);

/// Set the masked Xbox D3D12 process debug flags.
///
/// The reference implementation stores only bits in `0x83f00001` and returns
/// success. The host provider retains that process-local value for its getter.
#[unsafe(export_name = "D3D12XboxSetProcessDebugFlags")]
pub extern "system" fn set_process_debug_flags(flags: u32) -> HRESULT {
    PROCESS_DEBUG_FLAGS.store(flags & PROCESS_DEBUG_FLAGS_MASK, Ordering::Relaxed);
    HRESULT(0)
}

/// Read the masked Xbox D3D12 process debug flags.
#[unsafe(export_name = "D3D12XboxGetProcessDebugFlags")]
pub extern "system" fn get_process_debug_flags() -> u32 {
    PROCESS_DEBUG_FLAGS.load(Ordering::Relaxed)
}

/// Creates the supported guest device, returning one owned COM reference.
///
/// # Safety
/// A non-null adapter must be a live DXGI adapter. Non-null IID and output
/// pointers must denote a readable GUID and writable pointer respectively.
/// A non-null parameter record must have a readable version DWORD. Version
/// 0x20002 requires 64 readable bytes and 0x20004 requires 72. Inputs must
/// remain valid throughout the call and must not overlap the output pointer.
#[unsafe(export_name = "D3D12XboxCreateDevice")]
pub unsafe extern "system" fn create_device(
    adapter: *mut c_void,
    params: *const c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: The caller supplies a writable output pointer. Clear it even
    // when subsequent interface negotiation or parameter validation fails.
    unsafe { out.write(ptr::null_mut()) };
    if iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: The caller supplies a readable GUID. Compare Windows GUID fields
    // without casting guest storage to an aligned Rust reference.
    let requested = unsafe { iid.read_unaligned() };
    if !device::DEVICE_INTERFACES.contains(&requested) {
        return E_NOINTERFACE;
    }
    if params.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: Only the version DWORD is guaranteed readable for unknown records.
    let supported = match unsafe { params.cast::<u32>().read_unaligned() } {
        // SAFETY: The caller provides a complete record for the selected version.
        0x20002 => (unsafe { params.cast::<[u32; 16]>().read_unaligned() }) == PROFILE_20002,
        0x20004 => {
            // SAFETY: This version defines an 18-DWORD record.
            let mut record = unsafe { params.cast::<[u32; 18]>().read_unaligned() };
            let mode = record[9];
            record[9] = 0;
            let retail = [
                0x20004, 0, 0x100000, 0, 0, 0, 0, 0,
                record[8], 0, 1, 0, 0, 0, 0, 0x20, 0, 0,
            ];
            mode <= 1 && (record == PROFILE_20004_DEFAULT || (record[8] <= 1 && record == retail))
        }
        _ => false,
    };
    if !supported {
        return E_NOTIMPL;
    }
    // SAFETY: The host helper accepts a null or borrowed adapter and writes
    // a single owned guest device reference to the caller's cleared output.
    let status = unsafe { device::create(adapter, requested, out) };
    if status.is_ok() {
        exports::device_vm_created();
        private_driver::activate();
    }
    status
}

/// The console's standard-named factory selects its default Xbox record.
///
/// # Safety
/// Adapter, interface ID and output follow the contract of [`create_device`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn tuxbox_create_device(
    adapter: *mut c_void,
    _minimum_feature_level: u32,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    let mut params = PROFILE_20004_DEFAULT;
    params[1] = get_process_debug_flags();
    // SAFETY: The local profile remains live until the factory returns.
    unsafe { create_device(adapter, params.as_ptr().cast(), iid, out) }
}

// Install the vectored exception handler when `d3d12_xs.dll` attaches so early
// title code (such as `clwb` on pre-Ice Lake CPUs) is covered before the first
// device is created. Using `.CRT$XCU` avoids colliding with `libcmt`'s default
// `DllMain` stub when linking this staticlib with MSVC `link.exe`.
#[cfg(not(test))]
unsafe extern "C" {
    fn atexit(func: extern "C" fn()) -> i32;
}

#[cfg(not(test))]
extern "C" fn uninstall_on_detach() {
    survey::uninstall_handler();
}

#[cfg(not(test))]
extern "C" fn install_on_attach() {
    if survey::install_handler() {
        // SAFETY: `atexit` registers a per-DLL detach callback in the MSVC CRT.
        unsafe { atexit(uninstall_on_detach) };
    }
}

#[cfg(not(test))]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INSTALL_VEH_INIT: extern "C" fn() = install_on_attach;
