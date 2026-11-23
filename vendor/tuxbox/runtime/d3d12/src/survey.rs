//! Survey mode (`TUXBOX_SURVEY=1`): keep a title running past recoverable
//! failures so one launch reports many of them instead of stopping at the first.
//!
//! - A title `int3` assert is stepped over. The shipping code follows each such
//!   break with its own recovery path (typically returning null), so the title
//!   continues as it would after a debugger resumed it.
//! - An unmapped D3D12.x vtable slot returns `E_NOTIMPL` instead of failing fast.
//!
//! Each distinct site is logged once, with a running hit count at powers of two.
//! This is a diagnostic mode: rendering may be wrong after the first failure.
use std::sync::{LazyLock, Mutex, Once};
use windows::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, EXCEPTION_BREAKPOINT, HMODULE};
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS, RtlCaptureStackBackTrace, RtlPcToFileHeader,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

const CONTINUE_EXECUTION: i32 = -1;
const CONTINUE_SEARCH: i32 = 0;

pub(crate) fn enabled() -> bool {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("TUXBOX_SURVEY").is_some_and(|v| v == "1"));
    *ENABLED
}

/// Install the assert handler once, when the title creates its first device.
pub(crate) fn install() {
    static INSTALL: Once = Once::new();
    if !enabled() {
        return;
    }
    INSTALL.call_once(|| {
        let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
        log(format!("survey mode active (assert handler installed: {})", !handle.is_null()));
    });
}

fn log(line: String) {
    crate::diagnostics::log_once(&Once::new(), || line);
}

/// Bounds of the title image (the process executable).
fn title_image() -> Option<(usize, usize)> {
    static IMAGE: LazyLock<Option<(usize, usize)>> = LazyLock::new(|| {
        let HMODULE(base) = unsafe { GetModuleHandleW(None) }.ok()?;
        let base = base as usize;
        // SAFETY: the loaded executable starts with valid DOS and PE32+ headers.
        unsafe {
            let nt = base + (base as *const u8).add(0x3c).cast::<u32>().read_unaligned() as usize;
            // Signature (4) + file header (20) + SizeOfImage offset in the optional header (56).
            let size = (nt as *const u8).add(4 + 20 + 56).cast::<u32>().read_unaligned() as usize;
            Some((base, base + size))
        }
    });
    *IMAGE
}

/// Count a hit for `site`. Returns the new count.
fn record(table: &Mutex<Vec<(usize, u64)>>, site: usize) -> u64 {
    let mut sites = table.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = sites.iter_mut().find(|(at, _)| *at == site) {
        entry.1 += 1;
        return entry.1;
    }
    sites.push((site, 1));
    1
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    static ASSERTS: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());
    let Some(info) = (unsafe { info.as_mut() }) else { return CONTINUE_SEARCH };
    let (Some(record_), Some(context)) = (unsafe { info.ExceptionRecord.as_ref() }, unsafe { info.ContextRecord.as_mut() })
    else {
        return CONTINUE_SEARCH;
    };
    if record_.ExceptionCode == EXCEPTION_ACCESS_VIOLATION {
        describe_fault(context);
        return CONTINUE_SEARCH;
    }
    if record_.ExceptionCode != EXCEPTION_BREAKPOINT {
        return CONTINUE_SEARCH;
    }
    let Some((start, end)) = title_image() else { return CONTINUE_SEARCH };
    let rip = context.Rip as usize;
    if !(start..end).contains(&rip) {
        return CONTINUE_SEARCH;
    }
    // Windows reports an int3 at its own address; resume after it.
    // SAFETY: rip lies within the mapped title image.
    let at = if unsafe { *(rip as *const u8) } == 0xcc {
        context.Rip += 1;
        rip
    } else if rip > start && unsafe { *((rip - 1) as *const u8) } == 0xcc {
        rip - 1
    } else {
        return CONTINUE_SEARCH;
    };
    let hits = record(&ASSERTS, at - start);
    if hits.is_power_of_two() {
        log(format!("survey: stepped over title assert RVA {:#x} (hit {hits})", at - start));
    }
    CONTINUE_EXECUTION
}

/// Return address of the first title frame on this thread, as an RVA.
pub(crate) fn title_rva() -> Option<usize> {
    let mut frames = [core::ptr::null_mut(); 32];
    let count = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
    let (start, _) = title_image()?;
    frames[..count].iter().find_map(|&pc| {
        let mut base = core::ptr::null_mut();
        unsafe { RtlPcToFileHeader(pc, &mut base) };
        (base as usize == start).then(|| pc as usize - start)
    })
}

/// Log a call to an unmapped vtable slot (once per slot and caller).
pub(crate) fn unsupported_slot(slot: usize) {
    static SLOTS: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());
    let rva = title_rva().unwrap_or(0);
    let hits = record(&SLOTS, (slot << 40) | rva);
    if hits.is_power_of_two() {
        log(format!("survey: unsupported D3D12.x vtable slot {slot} from title RVA {rva:#x} (hit {hits})"));
    }
}

/// Log the memory region behind each register of a title access violation.
fn describe_fault(context: &windows::Win32::System::Diagnostics::Debug::CONTEXT) {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    static FAULTS: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());
    let Some((start, end)) = title_image() else { return };
    let rip = context.Rip as usize;
    if !(start..end).contains(&rip) || record(&FAULTS, rip - start) != 1 {
        return;
    }
    let registers = [("rax", context.Rax), ("rbx", context.Rbx), ("rcx", context.Rcx), ("rdx", context.Rdx),
        ("rsi", context.Rsi), ("rdi", context.Rdi), ("r8", context.R8), ("r9", context.R9),
        ("r12", context.R12), ("r13", context.R13), ("r14", context.R14), ("r15", context.R15)];
    let mut line = format!("survey: title access violation at RVA {:#x};", rip - start);
    for (name, value) in registers {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if value < 0x10000 || unsafe { VirtualQuery(Some(value as _), &mut region, size_of_val(&region)) } == 0
            || region.State.0 != 0x1000 {
            continue;
        }
        line += &format!(" {name}={value:#x} in allocation {:p} (region {:p}+{:#x} protect={:#x} type={:#x});",
            region.AllocationBase, region.BaseAddress, region.RegionSize, region.Protect.0, region.Type.0);
    }
    log(line);
    // Extent of a repeated 16-byte pattern at rcx (a fill over title memory).
    'fill: {
        let at = (context.Rcx as usize) & !0xf;
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(at as _), &mut region, size_of_val(&region)) } == 0
            || region.State.0 != 0x1000 || region.Protect.0 & 0x66 == 0 {
            break 'fill;
        }
        let (low, high) = (region.BaseAddress as usize, region.BaseAddress as usize + region.RegionSize);
        // SAFETY: every read below stays inside the committed readable region.
        let read = |a: usize| unsafe { (a as *const u128).read_unaligned() };
        let pattern = read(at);
        let (mut start, mut end) = (at, at + 16);
        while start >= low + 16 && read(start - 16) == pattern { start -= 16; }
        while end + 16 <= high && read(end) == pattern { end += 16; }
        log(format!("survey: fill {pattern:#034x} covers {start:#x}..{end:#x} ({:#x} bytes; before {:#034x} after {:#034x})",
            end - start, if start >= low + 16 { read(start - 16) } else { 0 }, if end + 16 <= high { read(end) } else { 0 }));
    }
    // Contents around each pointer register that lies in readable memory.
    for (name, value) in [("rax", context.Rax), ("rcx", context.Rcx), ("rsi", context.Rsi)] {
        let base = (value as usize).saturating_sub(0x80) & !0xf;
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(base as _), &mut region, size_of_val(&region)) } == 0
            || region.State.0 != 0x1000 || region.Protect.0 & 0x66 == 0
            || base + 0x180 > region.BaseAddress as usize + region.RegionSize {
            continue;
        }
        for row in (0..0x180).step_by(32) {
            // SAFETY: the committed readable region covers these bytes.
            let bytes = unsafe { core::slice::from_raw_parts((base + row) as *const u8, 32) };
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            log(format!("survey: {name} {:#x}: {hex}", base + row));
        }
    }
}
