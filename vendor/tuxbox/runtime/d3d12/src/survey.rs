//! Survey mode (`TUXBOX_SURVEY=1`): keep a title running past recoverable
//! failures so one launch reports many of them instead of stopping at the first.
//!
//! One title assert is stepped over on every launch. Its continue path is the
//! mapping the title itself uses after the break. Survey mode steps over every
//! other title `int3` as well.
//!
//! - A title `int3` assert is stepped over. The shipping code follows each such
//!   break with its own recovery path (typically returning null), so the title
//!   continues as it would after a debugger resumed it.
//! - An unmapped D3D12.x vtable slot returns `E_NOTIMPL` instead of failing fast.
//!
//! Each distinct site is logged once, with a running hit count at powers of two.
//! This is a diagnostic mode: rendering may be wrong after the first failure.
use core::ffi::c_void;
use core::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{LazyLock, Mutex, Once};
use windows::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_BREAKPOINT, EXCEPTION_ILLEGAL_INSTRUCTION, HMODULE,
};
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS, RtlCaptureStackBackTrace, RtlPcToFileHeader,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

const CONTINUE_EXECUTION: i32 = -1;
const CONTINUE_SEARCH: i32 = 0;

static VEH_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

pub(crate) fn enabled() -> bool {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("TUXBOX_SURVEY").is_some_and(|v| v == "1"));
    *ENABLED
}

/// Title asserts whose own continue path is correct, matched by RVA and by the
/// exact bytes there so another build cannot match.
///
/// ProjectScarlett 1.0.2.1, RVA `0x4ffd879`: the tile-heap mapper asserts when a
/// request is shorter than a whole flagged page run (8 KiB against a 16 KiB
/// run). The continue path maps `min(run, remaining)`, which is the request.
///
/// RVA `0x4ffd859`, same function: the run walk's cursor equals the table end
/// (`cmp r12, end; je`). The continue path reads the last table entry, which is
/// valid memory, and rejoins the walk at `0x4ffd604`.
const RECOVERABLE: [(usize, [u8; 9]); 2] = [
    (0x4ffd879, [0xcc, 0x48, 0x8b, 0x1f, 0xe9, 0xf0, 0xfd, 0xff, 0xff]),
    (0x4ffd859, [0xcc, 0xe9, 0xa5, 0xfd, 0xff, 0xff, 0xcc, 0x48, 0x8b]),
];

fn is_recoverable(rva: usize, bytes: &[u8]) -> bool {
    RECOVERABLE.iter().any(|&(site, expected)| rva == site && bytes == expected)
}

/// Asserts in a leaf function that has no stack frame, where stepping over the
/// `int3` would scan past the end of the title's table. Execution resumes at
/// the function's `ret`, which returns the value found so far. Matched by RVA
/// and exact bytes; the third field is the distance back to that `ret`.
///
/// ProjectScarlett 1.0.2.1, RVA `0x51d251e`: counts the page runs that cover a
/// byte range of the tile heap. It reaches the end of the run table with
/// 4 KiB still uncovered. The count so far (in `eax`) is the runs that exist.
const RETURN_EARLY: [(usize, [u8; 9], usize); 1] =
    [(0x51d251e, [0xcc, 0xeb, 0xc5, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc], 0x10)];

fn early_return_distance(rva: usize, bytes: &[u8]) -> Option<usize> {
    RETURN_EARLY
        .iter()
        .find(|&&(site, expected, _)| rva == site && bytes == expected)
        .map(|&(_, _, back)| back)
}

/// Register the vectored exception handler once. Safe to call from `DllMain`
/// so early title code (such as `clwb` on pre-Ice Lake CPUs, or `clzero` and
/// `insertq` on Intel CPUs) is covered before
/// the first device is created.
pub(crate) fn install_handler() -> bool {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
        VEH_HANDLE.store(handle, Ordering::Release);
    });
    !VEH_HANDLE.load(Ordering::Acquire).is_null()
}

/// Remove the vectored exception handler when `d3d12_xs.dll` is dynamically
/// unloaded.
#[cfg(not(test))]
pub(crate) fn uninstall_handler() {
    use windows::Win32::System::Diagnostics::Debug::RemoveVectoredExceptionHandler;
    let handle = VEH_HANDLE.swap(ptr::null_mut(), Ordering::AcqRel);
    if !handle.is_null() {
        unsafe { RemoveVectoredExceptionHandler(handle) };
    }
}

/// Install the assert handler once, when the title creates its first device.
/// Outside survey mode the handler only steps over `RECOVERABLE`, rewrites
/// unsupported `clwb` instructions, emulates unsupported `clzero` and `insertq`
/// and rewrites unsupported `monitorx`/`mwaitx` to NOPs.
pub(crate) fn install() {
    let installed = install_handler();
    static LOGGED: Once = Once::new();
    LOGGED.call_once(|| {
        if enabled() {
            log(format!("survey mode active (assert handler installed: {installed})"));
        }
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
    if record_.ExceptionCode == EXCEPTION_ILLEGAL_INSTRUCTION {
        if let Some((start, end)) = title_image() {
            if crate::clwb::handle_illegal_instruction(context, start, end)
                || crate::clzero::handle_illegal_instruction(context, start, end)
                || crate::mwaitx::handle_illegal_instruction(context, start, end)
                || crate::insertq::handle_illegal_instruction(context, start, end)
            {
                return CONTINUE_EXECUTION;
            }
        }
        return CONTINUE_SEARCH;
    }
    if record_.ExceptionCode == EXCEPTION_ACCESS_VIOLATION {
        if enabled() {
            describe_fault(context);
        }
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
    // Windows reports an int3 at its own address, or one byte past it.
    // SAFETY: rip lies within the mapped title image, and rip - 1 does too when used.
    let at_break = unsafe { *(rip as *const u8) } == 0xcc;
    let int3 = if at_break {
        rip
    } else if rip > start && unsafe { *((rip - 1) as *const u8) } == 0xcc {
        rip - 1
    } else {
        return CONTINUE_SEARCH;
    };
    if !enabled() {
        if int3 + 9 > end {
            return CONTINUE_SEARCH;
        }
        // SAFETY: int3..int3+9 is inside the mapped title image.
        let bytes = unsafe { core::slice::from_raw_parts(int3 as *const u8, 9) };
        if let Some(back) = early_return_distance(int3 - start, bytes) {
            let ret = int3 - back;
            // SAFETY: ret lies within the mapped title image before int3.
            if ret < start || unsafe { *(ret as *const u8) } != 0xc3 {
                return CONTINUE_SEARCH;
            }
            context.Rip = ret as u64;
            let hits = record(&ASSERTS, int3 - start);
            if hits.is_power_of_two() {
                log(format!("returned early from title assert RVA {:#x} (hit {hits})", int3 - start));
            }
            return CONTINUE_EXECUTION;
        }
        if !is_recoverable(int3 - start, bytes) {
            return CONTINUE_SEARCH;
        }
        if at_break {
            context.Rip += 1;
        }
        let hits = record(&ASSERTS, int3 - start);
        if hits.is_power_of_two() {
            log(format!("continued past recoverable title assert RVA {:#x} (hit {hits})", int3 - start));
        }
        return CONTINUE_EXECUTION;
    }
    if at_break {
        context.Rip += 1;
    }
    let at = int3;
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

#[cfg(test)]
mod tests {
    use super::{early_return_distance, is_recoverable};

    const SCARLETT_TILE_HEAP: [u8; 9] = [0xcc, 0x48, 0x8b, 0x1f, 0xe9, 0xf0, 0xfd, 0xff, 0xff];

    #[test]
    fn run_walk_end_iterator_assert_is_recoverable_on_exact_bytes() {
        let site = [0xcc, 0xe9, 0xa5, 0xfd, 0xff, 0xff, 0xcc, 0x48, 0x8b];
        assert!(is_recoverable(0x4ffd859, &site));
        assert!(!is_recoverable(0x4ffd85a, &site));
        let mut other = site;
        other[2] = 0xa6;
        assert!(!is_recoverable(0x4ffd859, &other));
    }

    #[test]
    fn run_table_walk_assert_returns_early_only_on_exact_bytes() {
        let site = [0xcc, 0xeb, 0xc5, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc];
        assert_eq!(early_return_distance(0x51d251e, &site), Some(0x10));
        assert_eq!(early_return_distance(0x51d251f, &site), None);
        let mut other = site;
        other[2] = 0xc6;
        assert_eq!(early_return_distance(0x51d251e, &other), None);
    }

    #[test]
    fn only_the_scarlett_tile_heap_assert_is_recoverable() {
        assert!(is_recoverable(0x4ffd879, &SCARLETT_TILE_HEAP));
        let mut other = SCARLETT_TILE_HEAP;
        other[4] = 0xeb;
        assert!(!is_recoverable(0x4ffd879, &other));
        assert!(!is_recoverable(0x4ffd868, &SCARLETT_TILE_HEAP));
    }
}
