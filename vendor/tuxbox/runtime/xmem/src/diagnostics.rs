//! Guest memory diagnostics for a single desktop title partition.
use core::{ffi::c_char, mem::size_of};
use std::fmt::Write;
use windows_sys::Win32::Foundation::{ERROR_OBJECT_NOT_FOUND, FILETIME, SetLastError};
use windows_sys::Win32::System::ProcessStatus::{K32GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows_sys::Win32::System::Threading::GetSystemTimes;

#[unsafe(export_name = "XMemIsToolsMemAvailable")]
extern "system" fn tools_memory_available() -> i32 {
    // The retail check forwards an absent tools partition to xbthunk.
    unsafe { SetLastError(ERROR_OBJECT_NOT_FOUND) };
    0
}

#[unsafe(export_name = "XMemDumpMemoryStats")]
unsafe extern "system" fn dump(out: *mut c_char, capacity: usize) {
    if out.is_null() || capacity == 0 || capacity > i32::MAX as usize {
        return;
    }
    let mut perf = PERFORMANCE_INFORMATION::default();
    let mut text = String::from("SYSTEM PERFORMANCE INFO:\n\n");
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0 {
        let ticks = u64::from(idle.dwHighDateTime) << 32 | u64::from(idle.dwLowDateTime);
        let _ = writeln!(text, "IdleProcessTime: {ticks}");
    }
    if unsafe { K32GetPerformanceInfo(&mut perf, size_of::<PERFORMANCE_INFORMATION>() as u32) } != 0
    {
        let _ = writeln!(
            text,
            "AvailablePages: {}\nCommittedPages: {}\nCommitLimit: {}\nPeakCommitment: {}\nPagedPoolPages: {}\nNonPagedPoolPages: {}",
            perf.PhysicalAvailable,
            perf.CommitTotal,
            perf.CommitLimit,
            perf.CommitPeak,
            perf.KernelPaged,
            perf.KernelNonpaged
        );
    }
    let mut title = [0u64; 15];
    if unsafe { crate::working_set_statistics(0x2000_0001, &mut title) } != 0 {
        let _ = writeln!(
            text,
            "\nPARTITION CONFIG INFO: (System, Title, Tools)\n\nTitle\nCommittedPages: {}\nCommitLimit: {}\nPeakCommitment: {}",
            title[1] >> 12,
            title[0] >> 12,
            title[11] >> 12
        );
    }
    if text.len() >= capacity {
        // The console formatter requests fill-on-failure with byte 0x58.
        unsafe {
            out.write_bytes(b'X', capacity - 1);
            out.add(capacity - 1).write(0);
        }
    } else {
        unsafe {
            text.as_ptr().copy_to_nonoverlapping(out.cast(), text.len());
            out.add(text.len()).write(0);
        }
    }
}

/// `TUXBOX_WATCH_ADDRESS=<hex>[,<hex radius>]` (shared with the D3D12
/// provider): log each XMem allocation or mapping whose range comes within the
/// radius of that address, with the title return address.
pub(crate) fn watch(kind: &str, start: usize, len: usize, detail: impl FnOnce() -> String) {
    use std::sync::LazyLock;
    use windows_sys::Win32::System::Diagnostics::Debug::{OutputDebugStringA, RtlCaptureStackBackTrace};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    static ADDRESS: LazyLock<Option<(usize, usize)>> = LazyLock::new(|| {
        let text = std::env::var("TUXBOX_WATCH_ADDRESS").ok()?;
        let (at, radius) = text.split_once(',').unwrap_or((&text, "0"));
        let hex = |t: &str| usize::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok();
        Some((hex(at)?, hex(radius)?))
    });
    let Some((address, radius)) = *ADDRESS else { return };
    if start == 0 || !(start <= address.saturating_add(radius) && address.saturating_sub(radius) < start.saturating_add(len)) {
        return;
    }
    let image = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    let mut frames = [core::ptr::null_mut(); 24];
    let count = unsafe { RtlCaptureStackBackTrace(0, 24, frames.as_mut_ptr(), core::ptr::null_mut()) } as usize;
    // The executable's image is the first module above its base in practice;
    // report the first frame within 512 MiB of it.
    let rva = frames[..count].iter().map(|&pc| pc as usize)
        .find(|&pc| pc >= image && pc - image < 0x2000_0000).map_or(0, |pc| pc - image);
    let line = format!("TuxBox xmem: watch {address:#x}: {kind} {start:#x}+{len:#x} {} from title RVA {rva:#x}\n\0", detail());
    unsafe { OutputDebugStringA(line.as_ptr()) };
}
