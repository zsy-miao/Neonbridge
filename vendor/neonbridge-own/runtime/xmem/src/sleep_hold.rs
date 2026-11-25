//! Sleep(ms) as `ms` of the title's time. A title thread held in a bind or
//! draw waiting for a pipeline (d3d12's tuxbox_title_held) is not the
//! title's time: a console title never waits there. A retail title's stall
//! check (a thread that ticks once a second with Sleep and ends the process
//! after enough ticks without a sign of its main thread) could end it while
//! the driver builds the pipelines of its world, now that a frame may wait up
//! to TUXBOX_PIPELINE_WAIT_MS. Only a sleep of 100 ms or more is held: a
//! thread polling with short sleeps (streaming, sound) goes on. Ported from
//! xcl kernelx.rs:1452-1500 (XCL_SLEEP_HOLD, GPL-3.0, credit to its
//! authors). TUXBOX_SLEEP_HOLD=<ms> sets the shortest held sleep, 0 leaves
//! the exe's Sleep alone.
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

const SLEEP_STEP_MS: u32 = 20;
const SLEEP_HELD_POLL_MS: u32 = 5;
const SLEEP_HOLD_MIN_MS: u32 = 100;
const INFINITE: u32 = u32::MAX;

static REAL_SLEEP: AtomicUsize = AtomicUsize::new(0);
static MIN: AtomicUsize = AtomicUsize::new(SLEEP_HOLD_MIN_MS as usize);
static HELD: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

fn real_sleep(ms: u32) {
    let real: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(REAL_SLEEP.load(Ordering::Acquire)) };
    unsafe { real(ms) };
}

/// Whether d3d12 holds a title thread now (false until d3d12_xs is loaded).
fn held() -> bool {
    let mut f = HELD.load(Ordering::Acquire);
    if f == 0 {
        let name: Vec<u16> = "d3d12_xs.dll".encode_utf16().chain([0]).collect();
        let module = unsafe { GetModuleHandleW(name.as_ptr()) };
        if module.is_null() {
            return false;
        }
        f = unsafe { GetProcAddress(module, b"tuxbox_title_held\0".as_ptr()) } as usize;
        if f == 0 {
            return false;
        }
        HELD.store(f, Ordering::Release);
    }
    let f: extern "system" fn() -> u32 = unsafe { std::mem::transmute(f) };
    f() != 0
}

unsafe extern "system" fn title_sleep(ms: u32) {
    if ms == 0 || ms == INFINITE || (ms as usize) < MIN.load(Ordering::Relaxed) {
        return real_sleep(ms);
    }
    let mut left = std::time::Duration::from_millis(u64::from(ms));
    while !left.is_zero() {
        if held() {
            real_sleep(SLEEP_HELD_POLL_MS);
            continue;
        }
        let t = std::time::Instant::now();
        real_sleep((left.as_millis() as u32).clamp(1, SLEEP_STEP_MS));
        left = left.saturating_sub(t.elapsed());
    }
}

pub(crate) unsafe fn install() {
    let min = match std::env::var("TUXBOX_SLEEP_HOLD").ok().as_deref().map(str::trim) {
        Some("0") => return,
        Some(v) => v.parse().unwrap_or(SLEEP_HOLD_MIN_MS),
        None => SLEEP_HOLD_MIN_MS,
    };
    MIN.store(min as usize, Ordering::Relaxed);
    let k: Vec<u16> = "kernelbase.dll".encode_utf16().chain([0]).collect();
    let real = unsafe { GetProcAddress(GetModuleHandleW(k.as_ptr()), b"Sleep\0".as_ptr()) } as usize;
    if real == 0 {
        crate::loader::log("sleep hold: Sleep not found in kernelbase; the exe's Sleep is left alone");
        return;
    }
    REAL_SLEEP.store(real, Ordering::Release);
    let n = unsafe {
        crate::loader::patch_imports(|d| d.starts_with("api-ms-win-core-synch") || d == "kernel32.dll" || d == "kernelbase.dll",
            &[("Sleep", title_sleep as *const () as usize)])
    };
    crate::loader::log(&format!("sleep hold: {n} Sleep import(s) skip time a pipeline wait holds the title (sleeps of {min} ms or more; TUXBOX_SLEEP_HOLD=0 off)"));
}
