//! Who ends the process. test57 ended with exit code 1 at 117 s, no int3,
//! with system commit nearly exhausted; nothing of ours exits with 1. The
//! exe's and ucrtbase's ExitProcess, TerminateProcess, exit, _exit, _Exit,
//! quick_exit and abort imports are wrapped to log the code and the
//! caller's stack (module+offset) before the real call.
//! TUXBOX_EXIT_TRACE=0 leaves them alone.
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::loader::log;

const NAMES: [&str; 7] = ["ExitProcess", "TerminateProcess", "exit", "_exit", "_Exit", "quick_exit", "abort"];
static REAL: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];

fn report(name: &str, code: i64) {
    unsafe extern "system" {
        fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut *mut c_void, hash: *mut u32) -> u16;
    }
    let mut frames = [std::ptr::null_mut(); 24];
    let n = unsafe { RtlCaptureStackBackTrace(2, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) };
    let stack: Vec<String> = frames[..usize::from(n)].iter().map(|f| crate::bink::module_of(*f as u64)).collect();
    log(&format!("exit: {name}({code}) from {}", stack.join(" <- ")));
}

fn real(i: usize) -> usize {
    REAL[i].load(Ordering::Acquire)
}

unsafe extern "system" fn exit_process(code: u32) {
    report("ExitProcess", i64::from(code));
    let f: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(real(0)) };
    unsafe { f(code) }
}

unsafe extern "system" fn terminate_process(process: *mut c_void, code: u32) -> i32 {
    // Only this process ending is of interest.
    if process as isize == -1 || process.is_null() {
        report("TerminateProcess", i64::from(code));
    }
    let f: unsafe extern "system" fn(*mut c_void, u32) -> i32 = unsafe { std::mem::transmute(real(1)) };
    unsafe { f(process, code) }
}

macro_rules! crt_exit {
    ($name:ident, $i:expr) => {
        unsafe extern "C" fn $name(code: i32) {
            report(NAMES[$i], i64::from(code));
            let f: unsafe extern "C" fn(i32) = unsafe { std::mem::transmute(real($i)) };
            unsafe { f(code) }
        }
    };
}
crt_exit!(crt_exit, 2);
crt_exit!(crt_underscore_exit, 3);
crt_exit!(crt_capital_exit, 4);
crt_exit!(crt_quick_exit, 5);

unsafe extern "C" fn crt_abort() {
    report("abort", 3);
    let f: unsafe extern "C" fn() = unsafe { std::mem::transmute(real(6)) };
    unsafe { f() }
}

/// Called from loader::install, after the exe's imports are bound.
pub(crate) unsafe fn install() {
    if std::env::var("TUXBOX_EXIT_TRACE").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }
    let module = |n: &str| {
        let w: Vec<u16> = n.encode_utf16().chain([0]).collect();
        unsafe { GetModuleHandleW(w.as_ptr()) }
    };
    let (kernel, ucrt) = (module("kernel32.dll"), module("ucrtbase.dll"));
    let wrappers: [usize; 7] = [
        exit_process as *const () as usize, terminate_process as *const () as usize, crt_exit as *const () as usize,
        crt_underscore_exit as *const () as usize, crt_capital_exit as *const () as usize, crt_quick_exit as *const () as usize,
        crt_abort as *const () as usize,
    ];
    let mut hooks = Vec::new();
    for (i, name) in NAMES.iter().enumerate() {
        let from = if i < 2 { kernel } else { ucrt };
        if from.is_null() {
            continue;
        }
        let c = format!("{name}\0");
        let address = unsafe { GetProcAddress(from, c.as_ptr()) } as usize;
        if address != 0 {
            REAL[i].store(address, Ordering::Release);
            hooks.push((*name, wrappers[i]));
        }
    }
    let n = unsafe { crate::loader::patch_imports(|_| true, &hooks) };
    // ucrtbase's exit ends in its own ExitProcess/TerminateProcess imports.
    let m = if ucrt.is_null() { 0 } else {
        unsafe { crate::loader::patch_module_imports(ucrt.cast(), |_| true, &hooks[..hooks.iter().take_while(|(n, _)| n.starts_with(|c: char| c.is_ascii_uppercase())).count()], &[]) }
    };
    log(&format!("exit: {n} exe and {m} ucrtbase exit import(s) traced (TUXBOX_EXIT_TRACE=0 skips)"));
}
