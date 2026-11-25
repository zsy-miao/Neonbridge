#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use windows_sys::Win32::Foundation::EXCEPTION_BREAKPOINT;
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
    EXCEPTION_POINTERS, RaiseException,
};

const TEST_LOG: &str = "VICTORY_TEST_CRASH_LOG";

#[test]
fn first_chance_breakpoint_logging() {
    if let Some(log) = std::env::var_os(TEST_LOG) {
        victory_loader::log_crashes_to(PathBuf::from(log));
        victory_loader::note_title_output("synthetic title output");
        victory_loader::report_crashes();

        unsafe extern "system" fn handle_breakpoint(info: *mut EXCEPTION_POINTERS) -> i32 {
            // SAFETY: the system passes valid exception pointers.
            if unsafe { (*(*info).ExceptionRecord).ExceptionCode } == EXCEPTION_BREAKPOINT {
                EXCEPTION_CONTINUE_EXECUTION
            } else {
                EXCEPTION_CONTINUE_SEARCH
            }
        }

        // SAFETY: register after VIctory's vectored handler. This synthetic
        // breakpoint is continuable and the handler needs no extra data.
        unsafe {
            assert!(!AddVectoredExceptionHandler(0, Some(handle_breakpoint)).is_null());
            RaiseException(EXCEPTION_BREAKPOINT as u32, 0, 0, std::ptr::null());
        }
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    for opt_in in [false, true] {
        let log = dir.path().join(format!("breakpoint-{opt_in}.log"));
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "first_chance_breakpoint_logging", "--nocapture"])
            .env(TEST_LOG, &log)
            .env_remove("VICTORY_CRASH_BREAKPOINTS");
        if opt_in {
            child.env("VICTORY_CRASH_BREAKPOINTS", "1");
        }
        let output = child.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{}\n{stderr}", output.status);
        assert!(stderr.contains("first-chance exception 0x80000003"));

        if cfg!(feature = "diagnostics") && opt_in {
            let report = std::fs::read_to_string(&log).unwrap();
            assert!(report.contains("first-chance exception 0x80000003 at "));
            assert!(report.contains("  #0 "));
            assert!(report.contains("  title said: synthetic title output"));
            assert!(!report.contains("unhandled exception"));
        } else {
            assert!(!log.exists());
        }
    }
}
