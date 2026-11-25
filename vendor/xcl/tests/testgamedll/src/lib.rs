//! Fake middleware DLL shipped "with the game". It is loaded by XCL's own loader, not by
//! Windows: it imports a console DLL (kernelx.dll), has static TLS and a DllMain.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

static ATTACHED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static COUNTER: Cell<u32> = const { Cell::new(7) };
}

#[link(name = "kernelx", kind = "raw-dylib")]
extern "system" {
    fn XMemAlloc(size: usize, attrs: u64) -> *mut c_void;
    fn XMemFree(p: *mut c_void, attrs: u64);
}

#[no_mangle]
pub extern "system" fn DllMain(_h: *mut c_void, reason: u32, _r: *mut c_void) -> i32 {
    if reason == 1 {
        ATTACHED.store(1, Ordering::SeqCst);
    }
    1
}

#[no_mangle]
pub extern "C" fn mw_add(a: i32, b: i32) -> i32 {
    a + b
}

#[no_mangle]
pub extern "C" fn mw_attached() -> u32 {
    ATTACHED.load(Ordering::SeqCst)
}

/// First call on a thread returns 8 when the TLS template (7) was applied.
#[no_mangle]
pub extern "C" fn mw_tls_bump() -> u32 {
    COUNTER.with(|c| {
        c.set(c.get() + 1);
        c.get()
    })
}

#[no_mangle]
pub extern "C" fn mw_xmem_roundtrip() -> u32 {
    unsafe {
        let p = XMemAlloc(128, 6u64 << 24);
        if p.is_null() || p as usize % 64 != 0 {
            return 0;
        }
        XMemFree(p, 6u64 << 24);
    }
    1
}
