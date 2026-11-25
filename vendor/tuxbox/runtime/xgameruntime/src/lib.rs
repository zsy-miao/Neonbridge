//! Xbox Game Runtime entry points used by the statically linked GDK wrappers.
//! The queue vtable is mapped in evidence/matrix-awakens/runtime-wrappers.csv.
mod probe;
mod queue;
mod save;
mod save_store;
mod services;
mod title_temp;

use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA;
use windows_sys::core::GUID;

const OK: i32 = 0;
const INVALID: i32 = 0x80070057u32 as i32;
const NO_INTERFACE: i32 = 0x80004002u32 as i32;
const POINTER: i32 = 0x80004003u32 as i32;
const NOT_IMPLEMENTED: i32 = 0x80004001u32 as i32;
const ABORT: i32 = 0x80004004u32 as i32;
const NOT_INITIALIZED: i32 = 0x89240100u32 as i32;
const THREADING: GUID = GUID::from_u128(0x073b7dcb_1fcf_4030_94be_e3c9eb623428);
const SYSTEM: GUID = GUID::from_u128(0xe349bd1a_fc20_4e40_b99c_4178cc6b409f);
const SYSTEM_V1: GUID = GUID::from_u128(0x6fd71f09_7513_49f0_89bc_bfaf5df6f852);
const SYSTEM_V2: GUID = GUID::from_u128(0x1861cf2e_e18b_4834_a9f5_b4a4e6efb4cf);
const UNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
static INITIALIZATIONS: AtomicU32 = AtomicU32::new(0);

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

#[unsafe(no_mangle)]
extern "system" fn InitializeApiImpl(build: u32, revision: u32) -> i32 {
    InitializeApiImplEx(build, revision, 0)
}

#[unsafe(no_mangle)]
extern "system" fn InitializeApiImplEx(_build: u32, _revision: u32, _flags: u64) -> i32 {
    static STORAGE: std::sync::Once = std::sync::Once::new();
    STORAGE.call_once(title_temp::attach);
    if INITIALIZATIONS.fetch_add(1, Ordering::AcqRel) == 0 {
        unsafe { OutputDebugStringA(c"TuxBox xgameruntime: initialized\n".as_ptr().cast()) };
    }
    OK
}

#[unsafe(no_mangle)]
extern "system" fn UninitializeApiImpl() {
    let _ = INITIALIZATIONS.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
}

#[unsafe(no_mangle)]
unsafe extern "system" fn XErrorReport(_error: i32, message: *const c_char) {
    if !message.is_null() {
        // The GDK error message is a borrowed, NUL-terminated ANSI string.
        unsafe { OutputDebugStringA(message.cast()) };
    }
}

#[unsafe(no_mangle)]
unsafe extern "system" fn QueryApiImpl(
    class: *const GUID,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    unsafe {
        out.write(ptr::null_mut());
    }
    let (Some(class), Some(iid)) = (unsafe { class.as_ref() }, unsafe { iid.as_ref() }) else {
        return INVALID;
    };
    if INITIALIZATIONS.load(Ordering::Acquire) == 0 {
        return NOT_INITIALIZED;
    }
    if same(class, &SYSTEM) {
        return unsafe { query(&SYSTEM_OBJECT, iid, out) };
    }
    if !same(class, &THREADING) {
        let by_interface = probe::CLASSES.iter().position(|(c, _)| same(c, iid))
            .filter(|&i| i == probe::GAMERTAG);
        if let Some(index) = by_interface.or_else(|| probe::CLASSES.iter().position(|(c, _)| same(c, class)))
            && probe::available(index)
        {
            // report() dedups, so each served class is logged once.
            report(format_args!("serving {} (interface {})", probe::CLASSES[index].1, guid_text(iid)));
            let object = &probe::OBJECTS[index];
            add_ref(unsafe { &*ptr::from_ref(object).cast::<Object>() });
            unsafe { out.write(ptr::from_ref(object).cast_mut().cast()) };
            return OK;
        }
        report(format_args!("unsupported API class {} (interface {})", guid_text(class), guid_text(iid)));
        return NO_INTERFACE;
    }
    unsafe { query(&THREAD_OBJECT, iid, out) }
}

// Function addresses only, all tables live for the lifetime of the DLL.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Slot(*const ());
unsafe impl Sync for Slot {}
macro_rules! slot {
    ($func:path) => {
        Slot($func as *const ())
    };
}

/// Same layout as `Object` with a wider table (see `probe`).
#[repr(C)]
struct ProbeObject {
    vtable: &'static [Slot; 64],
    references: AtomicU32,
}

unsafe extern "system" fn query_probe(this: &Object, _iid: *const GUID, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    add_ref(this);
    unsafe { out.write(ptr::from_ref(this).cast_mut().cast()) };
    OK
}

#[repr(C)]
struct Object {
    vtable: &'static [Slot; 32],
    references: AtomicU32,
}

unsafe extern "system" fn query(this: &Object, iid: *const GUID, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    unsafe {
        out.write(ptr::null_mut());
    }
    let Some(iid) = (unsafe { iid.as_ref() }) else {
        return INVALID;
    };
    let supported = if ptr::eq(this, &SYSTEM_OBJECT) {
        same(iid, &SYSTEM_V1) || same(iid, &SYSTEM_V2)
    } else {
        same(iid, &THREADING)
    };
    if !supported && !same(iid, &UNKNOWN) {
        return NO_INTERFACE;
    }
    add_ref(this);
    unsafe {
        out.write(ptr::from_ref(this).cast_mut().cast());
    }
    OK
}

extern "system" fn add_ref(this: &Object) -> u32 {
    this.references.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn release(this: &Object) -> u32 {
    // The DLL owns the permanent interface. The external count can reach zero.
    this.references.fetch_sub(1, Ordering::Relaxed) - 1
}

// Each unmapped method reports its interface and slot once, then returns E_NOTIMPL.
extern "system" fn unsupported_method<const SYSTEM: bool, const N: usize>(_: &Object) -> i32 {
    use std::sync::Mutex;
    static REPORTED: Mutex<Vec<(bool, usize)>> = Mutex::new(Vec::new());
    let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
    if !reported.contains(&(SYSTEM, N)) {
        reported.push((SYSTEM, N));
        drop(reported);
        report(format_args!("unsupported {} method slot {N}", if SYSTEM { "XSystem" } else { "XThreading" }));
    }
    NOT_IMPLEMENTED
}

macro_rules! unsupported_methods {
    ($system:literal; $($n:literal),*) => { [$(slot!(unsupported_method::<$system, $n>)),*] };
}

fn guid_text(g: &GUID) -> String {
    let d = g.data4;
    format!("{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7])
}

/// Report each distinct unsupported request once (bounded).
fn report(text: std::fmt::Arguments<'_>) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let line = format!("TuxBox xgameruntime: {text}");
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() >= 600 || seen.contains(&line) {
        return;
    }
    seen.push(line.clone());
    drop(seen);
    let message = format!("{line}\n\0");
    unsafe { OutputDebugStringA(message.as_ptr()) };
}
extern "system" fn idle_thread(_: &Object) {} // No guest thread is designated time-sensitive.
static THREAD_TABLE: [Slot; 32] = {
    let mut t = unsupported_methods!(false; 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31);
    t[0] = slot!(query);
    t[1] = slot!(add_ref);
    t[2] = slot!(release);
    t[3] = slot!(services::async_status);
    t[4] = slot!(services::async_result_size);
    t[5] = slot!(services::async_cancel);
    t[6] = slot!(services::async_run);
    t[12] = slot!(queue::create);
    t[13] = slot!(queue::create_composite);
    t[14] = slot!(queue::get_port);
    t[15] = slot!(queue::duplicate);
    t[16] = slot!(queue::dispatch);
    t[17] = slot!(queue::close);
    t[18] = slot!(queue::submit);
    t[19] = slot!(queue::submit_delayed);
    t[22] = slot!(queue::terminate);
    t[25] = slot!(queue::process_queue);
    t[29] = slot!(idle_thread);
    t
};
static THREAD_OBJECT: Object = Object {
    vtable: &THREAD_TABLE,
    references: AtomicU32::new(0),
};

// XSystemGetXboxLiveSandboxId on a host without a provisioned Xbox Live sandbox.
unsafe extern "system" fn sandbox_id(
    _: &Object,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    const CAPACITY: usize = 16;
    const ID: &[u8] = b"RETAIL\0";
    if size < CAPACITY {
        return 0x8007007au32 as i32;
    }
    if buffer.is_null() {
        return INVALID;
    }
    unsafe {
        ptr::write_bytes(buffer, 0, CAPACITY);
        ptr::copy_nonoverlapping(ID.as_ptr(), buffer, ID.len());
    }
    if !used.is_null() {
        unsafe {
            used.write_unaligned(ID.len());
        }
    }
    OK
}
static SYSTEM_TABLE: [Slot; 32] = {
    let mut slots = unsupported_methods!(true; 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31);
    slots[0] = slot!(query);
    slots[1] = slot!(add_ref);
    slots[2] = slot!(release);
    slots[3] = slot!(services::system_console_id);
    slots[4] = slot!(sandbox_id);
    slots[5] = slot!(services::system_app_device_id);
    slots
};
static SYSTEM_OBJECT: Object = Object {
    vtable: &SYSTEM_TABLE,
    references: AtomicU32::new(0),
};

