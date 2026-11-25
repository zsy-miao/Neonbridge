//! Names the faulting code when title code crashes.
//!
//! A vectored handler sees every exception first, so it only prints the
//! kinds that end a process (access violations and the like, and
//! breakpoints: titles assert with `int3`, fatal without a debugger) as
//! first-chance, since a handler may still take them (ntdll's loader does
//! with its own probes). What nobody handles, of any kind (an uncaught C++
//! exception too), is reported from the unhandled-exception filter
//! (`report_unhandled`, which the runtime calls ahead of the title's own)
//! and appended to the crash log, with the title's last debug output, which
//! often states the reason. Diagnostics builds can also append first-chance
//! breakpoints with `VICTORY_CRASH_BREAKPOINTS=1`. Panics of ours go to the
//! crash log too.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_BREAKPOINT, EXCEPTION_ILLEGAL_INSTRUCTION,
    EXCEPTION_INT_DIVIDE_BY_ZERO, EXCEPTION_PRIV_INSTRUCTION, EXCEPTION_STACK_OVERFLOW, HMODULE,
};

use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, EXCEPTION_POINTERS, M128A, RtlLookupFunctionEntry,
    RtlVirtualUnwind, SetUnhandledExceptionFilter, UNW_FLAG_NHANDLER,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadStackLimits;

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// Frames listed per report.
pub(crate) const MAX_FRAMES: usize = 40;

/// Memory saved to files when a crash is reported: (address, length, file).
#[cfg(feature = "diagnostics")]
static SAVE: std::sync::OnceLock<Vec<(usize, usize, std::path::PathBuf)>> =
    std::sync::OnceLock::new();

/// Debugging aid: on a reported crash, also write each `[address, address
/// + len)` (readable title memory, e.g. an engine table) to its file.
#[cfg(feature = "diagnostics")]
pub fn save_on_crash(ranges: Vec<(usize, usize, std::path::PathBuf)>) {
    let _ = SAVE.set(ranges);
}

/// The file crash reports are appended to, besides being printed.
static LOG: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Read before installing the exception handler.
static LOG_FIRST_CHANCE_BREAKPOINTS: AtomicBool = AtomicBool::new(false);

/// Also appends every unhandled-exception report and panic to `file`, so a
/// crash can be read after the console is gone.
pub fn log_crashes_to(file: std::path::PathBuf) {
    if LOG.set(file).is_ok() {
        let _ = HOST.set(host());
        let print = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            print(info);
            append_to_log(&format!(
                "VIctory: panic on thread {:?}: {info}\n{}",
                std::thread::current().name().unwrap_or("?"),
                report_end()
            ));
        }));
    }
}

/// Appends a failure that is not an exception (the host GPU's) to the crash
/// log, with the title's last debug output, and prints it.
pub fn log_crash(text: &str) {
    let text = format!("{text}
{}", report_end());
    eprint!("{text}");
    append_to_log(&text);
}

fn append_to_log(text: &str) {
    use std::io::Write;
    let Some(file) = LOG.get() else { return };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let opened = std::fs::OpenOptions::new().create(true).append(true).open(file);
    if let Ok(mut f) = opened {
        let _ = write!(f, "unix time {now}, process {}\n{text}\n", std::process::id());
    }
}

/// The title's last lines of debug output, kept for crash reports.
static SAID: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
const SAID_LINES: usize = 8;

/// Keeps a line of the title's debug output for the next crash report.
pub fn note_title_output(line: &str) {
    if line.is_empty() {
        return;
    }
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if said.len() == SAID_LINES {
        said.pop_front();
    }
    said.push_back(line.to_owned());
}

/// The kept lines, one per report line; nothing if another thread holds
/// them (it may be the one that faulted).
fn title_output() -> String {
    let Ok(said) = SAID.try_lock() else {
        return String::new();
    };
    said.iter().map(|l| format!("  title said: {l}\n")).collect()
}

/// The host's line in a crash report. Its CPU and Windows build decide what
/// title code runs (Intel CPUs lack some of the console's AMD-only
/// instructions) and which D3D12 runtime there is, so a report needs both.
/// Set along with the crash log, so a crash queries nothing.
static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// What ends a crash report: the host, then the title's last debug output.
fn report_end() -> String {
    let host = HOST.get().map_or("", String::as_str);
    format!("{host}{}", title_output())
}

/// The line [`HOST`] holds.
fn host() -> String {
    format!("  host: {}, Windows {}\n", cpu_name(), windows_version())
}

/// The CPU's brand string, e.g. `AMD Ryzen 7 5700X3D 8-Core Processor`.
fn cpu_name() -> String {
    use std::arch::x86_64::__cpuid;
    if __cpuid(0x8000_0000).eax < 0x8000_0004 {
        return "an unnamed CPU".into();
    }
    let bytes: Vec<u8> = (0x8000_0002..=0x8000_0004)
        .flat_map(|leaf| {
            let r = __cpuid(leaf);
            [r.eax, r.ebx, r.ecx, r.edx]
        })
        .flat_map(u32::to_le_bytes)
        .collect();
    let padding = |c: char| c == '\0' || c.is_whitespace();
    let name = String::from_utf8_lossy(&bytes);
    name.trim_matches(padding).to_owned()
}

/// The running Windows as `major.minor.build`. `RtlGetVersion` gives the
/// real one; `GetVersionEx` gives what the executable's manifest declares.
fn windows_version() -> String {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    // SAFETY: a zeroed OSVERSIONINFOW is valid; its size field is set.
    let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    info.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
    // SAFETY: fills the structure passed, whose size it was told.
    if unsafe { RtlGetVersion(&mut info) } != 0 {
        return "of unknown version".into();
    }
    let (major, minor, build) = (info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber);
    format!("{major}.{minor}.{build}")
}

/// Installs the handlers; call once before running title code.
pub fn report_crashes() {
    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        unsafe { report_unhandled(info) };
        EXCEPTION_CONTINUE_SEARCH
    }
    LOG_FIRST_CHANCE_BREAKPOINTS.store(
        cfg!(feature = "diagnostics")
            && std::env::var_os("VICTORY_CRASH_BREAKPOINTS").is_some_and(|value| value == "1"),
        Ordering::Relaxed,
    );
    // SAFETY: registers handlers that only read the exception they are
    // given; the title's own filter, once it sets one, takes the place of
    // ours and calls `report_unhandled` first.
    unsafe {
        AddVectoredExceptionHandler(0, Some(handler));
        SetUnhandledExceptionFilter(Some(filter));
    }
}

/// What the vectored handler reports as it happens.
const FATAL: [i32; 6] = [
    EXCEPTION_ACCESS_VIOLATION,
    EXCEPTION_ILLEGAL_INSTRUCTION,
    EXCEPTION_PRIV_INSTRUCTION,
    EXCEPTION_INT_DIVIDE_BY_ZERO,
    EXCEPTION_STACK_OVERFLOW,
    EXCEPTION_BREAKPOINT,
];

/// Console CPUs are Zen 2: titles use AMD-only instructions Intel CPUs
/// lack. Does their job instead: `clzero` (`0f 01 fc`) zeroes the 64-byte
/// line at `rax` (see [`clzero`] for the loops around it);
/// `monitorx`/`mwaitx` (`0f 01 fa`/`fb`) are wait hints, so a spin loop
/// around them works with mwaitx as a yield; `rdpru` (`0f 01 fd`) reads
/// MPERF (`ecx` 0) or APERF (1), cycles at the nominal and the actual
/// frequency, and gets the time-stamp counter for both: a core that never
/// changes frequency.
unsafe fn emulate_amd_only(info: *mut EXCEPTION_POINTERS) -> bool {
    // SAFETY: the system passes valid exception pointers; the faulting
    // instruction's bytes are readable, as the CPU just fetched them.
    unsafe {
        let record = &*(*info).ExceptionRecord;
        let context = &mut *(*info).ContextRecord;
        if record.ExceptionCode != EXCEPTION_ILLEGAL_INSTRUCTION {
            return false;
        }
        let code = context.Rip as *const u8;
        match code.cast::<[u8; 3]>().read_unaligned() {
            [0x0f, 0x01, 0xfc] => return clzero(context),
            [0x0f, 0x01, 0xfa] => {}
            [0x0f, 0x01, 0xfb] => std::thread::yield_now(),
            [0x0f, 0x01, 0xfd] => {
                // Flags are left as they are: a Zen 3 changes none, for
                // known counters or not (measured in user mode).
                let known = context.Rcx as u32 <= 1;
                let cycles = if known { std::arch::x86_64::_rdtsc() } else { 0 };
                context.Rax = cycles & 0xffff_ffff;
                context.Rdx = cycles >> 32;
            }
            _ => {
                // Only bytes inside the faulting instruction's page are read: the next page may
                // not be mapped.
                let room = 0x1000 - (context.Rip as usize & 0xfff);
                let mut bytes = [0u8; 32];
                let n = room.min(bytes.len());
                std::ptr::copy_nonoverlapping(code, bytes.as_mut_ptr(), n);
                if let Some(pass) = clwb_loop(&bytes[..n])
                    && let Some(end) =
                        clwb_loop_end(*gpr(context, pass.base), *gpr(context, pass.limit))
                {
                    // A whole write-back loop in one exception (see `clwb_loop`).
                    let limit = *gpr(context, pass.limit);
                    *gpr(context, pass.base) = end;
                    let flags = cmp_flags(end, limit);
                    context.EFlags = context.EFlags & !CMP_FLAGS | flags;
                    context.Rip += pass.len as u64;
                    return true;
                }
                if let Some(len) = clwb_len(&bytes[..n]) {
                    // A write-back hint: the PC's memory is coherent, so it has nothing to do.
                    context.Rip += len as u64;
                    return true;
                }
                let Some(len) = sse4a(code, &mut context.Anonymous.FltSave.XmmRegisters) else {
                    return false;
                };
                context.Rip += len as u64;
                return true;
            }
        }
        context.Rip += 3;
    }
    true
}

/// `clwb m8` (`66 [rex] 0f ae /6`, a memory operand), which Intel CPUs before
/// Ice Lake lack: its length, or `None` for any other instruction.
fn clwb_len(code: &[u8]) -> Option<usize> {
    let at = if *code.get(1)? & 0xf0 == 0x40 { 2 } else { 1 };
    if *code.first()? != 0x66 || code.get(at..at + 2)? != [0x0f, 0xae] {
        return None;
    }
    let modrm = *code.get(at + 2)?;
    let (mode, rm) = (modrm >> 6, modrm & 7);
    if mode == 3 || (modrm >> 3) & 7 != 6 {
        return None;
    }
    let mut len = at + 3;
    let mut base = rm;
    if rm == 4 {
        base = *code.get(len)? & 7;
        len += 1;
    }
    len += match mode {
        0 if rm == 5 || (rm == 4 && base == 5) => 4,
        1 => 1,
        2 => 4,
        _ => 0,
    };
    (len <= code.len()).then_some(len)
}

/// A `clwb` write-back loop as the title's compilers build it, one cache line a
/// pass (`base` and `limit` any registers):
///
/// ```text
/// top: clwb [base]
///      add  base, 0x40
///      cmp  base, limit
///      jbe  top
/// ```
///
/// An exception costs about a microsecond and the title writes back whole
/// buffers this way while it loads, so the loop is done in one: `clwb` has
/// nothing to do on the PC, `base` ends past `limit` as the last pass leaves it.
struct ClwbLoop {
    base: u8,
    limit: u8,
    /// Bytes from `clwb` to the instruction after `jbe`.
    len: usize,
}

/// The `base` value left after a `clwb_loop` finishes without wrapping past
/// `u64::MAX`, or `None` if `add base, 0x40` would overflow or wrap (in which
/// case we fall through to single-instruction `clwb` emulation).
fn clwb_loop_end(base: u64, limit: u64) -> Option<u64> {
    let step = if base <= limit {
        ((limit - base) / 64 + 1).checked_mul(64)?
    } else {
        64
    };
    let end = base.checked_add(step)?;
    (end > limit).then_some(end)
}

fn clwb_loop(code: &[u8]) -> Option<ClwbLoop> {
    // clwb [base]: 66 [40|41] 0f ae, ModRM mod 00, reg 6, no SIB or displacement.
    let (rex, at) = match code.get(1)? {
        0x40 | 0x41 => (code[1], 2),
        _ => (0, 1),
    };
    if *code.first()? != 0x66 || code.get(at..at + 2)? != [0x0f, 0xae] {
        return None;
    }
    let modrm = *code.get(at + 2)?;
    let rm = modrm & 7;
    if modrm >> 6 != 0 || (modrm >> 3) & 7 != 6 || rm == 4 || rm == 5 {
        return None;
    }
    let base = rm | (rex & 1) << 3;
    // add base, 0x40: REX.W(+B) 83 /0 ib.
    let add = at + 3;
    if *code.get(add..add + 4)? != [0x48 | base >> 3, 0x83, 0xc0 | (base & 7), 0x40] {
        return None;
    }
    // cmp base, limit: REX.W 39 /r with base in r/m.
    let cmp = add + 4;
    let &[cmp_rex @ 0x48..=0x4f, 0x39, cmp_modrm] = code.get(cmp..cmp + 3)? else {
        return None;
    };
    if cmp_modrm >> 6 != 3 || (cmp_modrm & 7 | (cmp_rex & 1) << 3) != base {
        return None;
    }
    let limit = (cmp_modrm >> 3) & 7 | (cmp_rex & 4) << 1;
    // jbe top.
    let jump = cmp + 3;
    let len = jump + 2;
    if *code.get(jump)? != 0x76 || i64::from(*code.get(jump + 1)? as i8) != -(len as i64) {
        return None;
    }
    (limit != base).then_some(ClwbLoop { base, limit, len })
}

/// `clzero` at `context.Rip`. An exception costs about a microsecond and
/// titles clear tens of kilobytes at a time, a line per pass, so the two
/// loops compilers build around the instruction are done whole in one
/// exception, leaving registers and flags as their last pass would (`a`
/// and `b` any registers but `rax`):
///
/// ```text
/// top: clzero                 top: mov rax, a
///      add rax, 0x40               add a, 0x40
///      cmp rax, b                  clzero
///      jb  top                     cmp a, b
///                                  jb  top
/// ```
///
/// Anything else around it is one line and the next instruction.
///
/// # Safety
/// `context` is the state of a thread that faulted at a `clzero`; the
/// lines its registers name are the title's to zero.
unsafe fn clzero(context: &mut CONTEXT) -> bool {
    // SAFETY: (every use) a line the title asked to zero.
    let zero = |rax: u64| unsafe { ((rax & !63) as *mut u8).write_bytes(0, 64) };
    let at = context.Rip as usize;
    // The loops' other instructions are read only where that stays inside
    // the faulting instruction's page (the next one may not be mapped).
    let page = at & 0xfff;
    if page + 12 <= 0x1000 {
        // SAFETY: inside the page the CPU fetched the instruction from.
        let after = unsafe { &*((at + 3) as *const [u8; 9]) };
        if let [0x48, 0x83, 0xc0, 0x40, cmp @ .., 0x72, 0xf4] = after
            && let Some((0, limit)) = compare_registers(cmp)
            && limit != 0
        {
            let limit = *gpr(context, limit);
            loop {
                zero(context.Rax);
                context.Rax = context.Rax.wrapping_add(0x40);
                if context.Rax >= limit {
                    break;
                }
            }
            context.EFlags = context.EFlags & !CMP_FLAGS | cmp_flags(context.Rax, limit);
            context.Rip += 12;
            return true;
        }
    }
    if (7..=0x1000 - 8).contains(&page) {
        // SAFETY: inside the page the CPU fetched the instruction from.
        let (before, after) =
            unsafe { (&*((at - 7) as *const [u8; 7]), &*((at + 3) as *const [u8; 5])) };
        if let [rex @ (0x48 | 0x4c), 0x89, load, add_rex @ (0x48 | 0x49), 0x83, add, 0x40] = *before
            && let [cmp @ .., 0x72, 0xf1] = after
            && let Some((a, b)) = compare_registers(cmp)
            && load & 0xc7 == 0xc0
            && a == (load >> 3) & 7 | (rex & 4) << 1
            && add & 0xf8 == 0xc0
            && a == add & 7 | (add_rex & 1) << 3
            && a != 0
            && b != 0
        {
            let limit = *gpr(context, b);
            loop {
                zero(context.Rax);
                let next = *gpr(context, a);
                if next >= limit {
                    break;
                }
                context.Rax = next;
                *gpr(context, a) = next.wrapping_add(0x40);
            }
            context.EFlags =
                context.EFlags & !CMP_FLAGS | cmp_flags(*gpr(context, a), limit);
            context.Rip += 8;
            return true;
        }
    }
    zero(context.Rax);
    context.Rip += 3;
    true
}

/// `cmp rm, reg` between 64-bit registers (`REX.W 39 /r`): (`rm`, `reg`).
fn compare_registers(code: &[u8]) -> Option<(u8, u8)> {
    let &[rex @ 0x48..=0x4f, 0x39, modrm] = code else {
        return None;
    };
    (modrm >> 6 == 3).then_some((modrm & 7 | (rex & 1) << 3, (modrm >> 3) & 7 | (rex & 4) << 1))
}

/// The general register numbered `n` as instructions encode it.
fn gpr(context: &mut CONTEXT, n: u8) -> &mut u64 {
    match n & 15 {
        0 => &mut context.Rax,
        1 => &mut context.Rcx,
        2 => &mut context.Rdx,
        3 => &mut context.Rbx,
        4 => &mut context.Rsp,
        5 => &mut context.Rbp,
        6 => &mut context.Rsi,
        7 => &mut context.Rdi,
        8 => &mut context.R8,
        9 => &mut context.R9,
        10 => &mut context.R10,
        11 => &mut context.R11,
        12 => &mut context.R12,
        13 => &mut context.R13,
        14 => &mut context.R14,
        _ => &mut context.R15,
    }
}

/// The status flags `cmp a, b` leaves (CF, PF, AF, ZF, SF, OF) as `EFLAGS` bits.
fn cmp_flags(a: u64, b: u64) -> u32 {
    let r = a.wrapping_sub(b);
    u32::from(a < b)
        | u32::from((r as u8).count_ones().is_multiple_of(2)) << 2
        | (((a ^ b ^ r) >> 4) as u32 & 1) << 4
        | u32::from(r == 0) << 6
        | ((r >> 63) as u32) << 7
        | ((((a ^ b) & (a ^ r)) >> 63) as u32) << 11
}
const CMP_FLAGS: u32 = 0x8d5;

/// SSE4a's `extrq`/`insertq` (register forms, the only ones there are) on
/// the saved XMM registers; returns the instruction's length.
unsafe fn sse4a(code: *const u8, xmm: &mut [M128A; 16]) -> Option<usize> {
    // SAFETY: the caller's code bytes are readable up to the instruction's end.
    let b = |i: usize| unsafe { code.add(i).read() };
    let prefix = b(0);
    let (rex, at) = if b(1) & 0xf0 == 0x40 {
        (b(1), 2)
    } else {
        (0, 1)
    };
    if !matches!(prefix, 0x66 | 0xf2) || b(at) != 0x0f || !matches!(b(at + 1), 0x78 | 0x79) {
        return None;
    }
    let modrm = b(at + 2);
    if modrm >> 6 != 3 {
        return None;
    }
    let dst = usize::from((modrm >> 3) & 7 | (rex & 4) << 1);
    let src = usize::from(modrm & 7 | (rex & 1) << 3);
    let immediate = b(at + 1) == 0x78;
    let (fields, len) = if immediate {
        ((b(at + 3), b(at + 4)), at + 5)
    } else {
        // extrq reads its fields from src's low half, insertq from its high.
        let f = if prefix == 0x66 { xmm[src].Low } else { xmm[src].High as u64 };
        ((f as u8, (f >> 8) as u8), at + 3)
    };
    let (n, at_bit) = (u32::from(fields.0 & 63), u32::from(fields.1 & 63));
    let bits = if n == 0 { 64 } else { n };
    let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
    if prefix == 0x66 {
        // extrq: register form takes its fields from src, works on dst.
        if immediate && (modrm >> 3) & 7 != 0 {
            return None;
        }
        let target = if immediate { src } else { dst };
        xmm[target].Low = (xmm[target].Low >> at_bit) & mask;
    } else {
        let value = (xmm[src].Low & mask) << at_bit;
        xmm[dst].Low = xmm[dst].Low & !(mask << at_bit) | value;
    }
    Some(len)
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes valid exception pointers.
    if unsafe { emulate_amd_only(info) } {
        return -1; // EXCEPTION_CONTINUE_EXECUTION
    }
    // SAFETY: as above.
    let code = unsafe { (*(*info).ExceptionRecord).ExceptionCode };
    if FATAL.contains(&code) {
        let to_file =
            code == EXCEPTION_BREAKPOINT && LOG_FIRST_CHANCE_BREAKPOINTS.load(Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { report("first-chance exception", info, to_file) };
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// For an unhandled-exception filter: reports the exception that is about
/// to end the process, and appends it to the crash log.
///
/// # Safety
/// `info` comes from the system, for an exception being dispatched.
pub unsafe fn report_unhandled(info: *const EXCEPTION_POINTERS) {
    // SAFETY: forwarded from the caller.
    unsafe { report("unhandled exception", info, true) };
}

/// The bytes at `at` as hex, as many as the longest x86 instruction (15),
/// but none past the end of `at`'s page: the next page may not be mapped.
/// For an instruction the CPU refused, they name it (`0f 01 fd` is
/// `rdpru`), which the address alone does not.
///
/// # Safety
/// `at`'s page is readable.
unsafe fn instruction_bytes(at: usize) -> String {
    let n = (0x1000 - (at & 0xfff)).min(15);
    // SAFETY: `n` bytes from `at` stay inside its page, which the caller
    // says is readable.
    let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, n) };
    let hex: Vec<_> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    hex.join(" ")
}

/// MSVC's code for a C++ `throw`, and its magic first parameter.
const CPP_EXCEPTION: i32 = 0xE06D_7363_u32 as i32;
const CPP_MAGIC: usize = 0x1993_0520;

/// What a C++ throw threw: the decorated name of its type, then its value
/// where that names the cause (an integer, e.g. an HRESULT; a standard
/// exception's message).
///
/// # Safety
/// `p` are the parameters of a live MSVC throw (`p[0]` the magic).
unsafe fn thrown(p: &[usize]) -> String {
    // SAFETY: the throw's ThrowInfo, its RVAs relative to `p[3]`: catchable
    // type array at +12 (count, then type RVAs), each type's descriptor at
    // +4, whose decorated name starts at +16; the object is at `p[1]`.
    unsafe {
        let rva = |at: usize| (at as *const u32).read_unaligned() as usize;
        let types = p[3] + rva(p[2] + 12);
        let name = |i: usize| {
            let descriptor = p[3] + rva(p[3] + rva(types + 4 + 4 * i) + 4);
            std::ffi::CStr::from_ptr((descriptor + 16) as *const std::ffi::c_char)
                .to_string_lossy()
        };
        let first = name(0);
        let value = match &*first {
            // int, unsigned, long (HRESULT), unsigned long
            ".H" | ".I" | ".J" | ".K" if p[1] != 0 => {
                format!(" = {:#010x}", (p[1] as *const u32).read_unaligned())
            }
            // std::exception keeps its message after the table pointer.
            _ if (0..rva(types)).any(|i| name(i) == ".?AVexception@std@@") => {
                let message = (p[1] as *const *const std::ffi::c_char).add(1).read();
                if message.is_null() {
                    String::new()
                } else {
                    format!(": {}", std::ffi::CStr::from_ptr(message).to_string_lossy())
                }
            }
            _ => String::new(),
        };
        format!("{first}{value}")
    }
}

/// # Safety
/// See [`report_unhandled`].
unsafe fn report(what: &str, info: *const EXCEPTION_POINTERS, to_file: bool) {
    // SAFETY: the system passes valid exception pointers.
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &*(*info).ContextRecord) };
    let code = record.ExceptionCode;
    let at = record.ExceptionAddress as usize;
    let p = record.ExceptionInformation;
    let detail = if code == EXCEPTION_ACCESS_VIOLATION {
        let kind = match p[0] {
            0 => "read",
            1 => "write",
            _ => "execute",
        };
        format!(" ({kind} {:#x}: {})", p[1], memory_at(p[1]))
    } else if code == CPP_EXCEPTION && record.NumberParameters >= 4 && p[0] == CPP_MAGIC {
        // SAFETY: a live MSVC throw.
        format!(" (C++ {} thrown)", unsafe { thrown(&p) })
    } else if code == EXCEPTION_ILLEGAL_INSTRUCTION || code == EXCEPTION_PRIV_INSTRUCTION {
        // SAFETY: the CPU refused the instruction at `at`, so it fetched it.
        format!(" (code {})", unsafe { instruction_bytes(at) })
    } else {
        String::new()
    };
    // Printed line by line (a fault while unwinding must not lose the
    // lines before it), and kept for the file.
    let mut text = String::new();
    let mut line = |l: String| {
        eprintln!("{l}");
        text.push_str(&l);
        text.push('\n');
    };
    line(format!(
        "VIctory: {what} {:#010x} at {}{detail}",
        code as u32,
        locate(at)
    ));
    line(format!(
        "  rax {:#x} rbx {:#x} rcx {:#x} rdx {:#x} rsi {:#x} rdi {:#x} rsp {:#x} rbp {:#x}",
        context.Rax,
        context.Rbx,
        context.Rcx,
        context.Rdx,
        context.Rsi,
        context.Rdi,
        context.Rsp,
        context.Rbp
    ));
    line(format!(
        "  r8 {:#x} r9 {:#x} r10 {:#x} r11 {:#x} r12 {:#x} r13 {:#x} r14 {:#x} r15 {:#x}",
        context.R8,
        context.R9,
        context.R10,
        context.R11,
        context.R12,
        context.R13,
        context.R14,
        context.R15
    ));
    for (i, pc) in unwind(*context).into_iter().enumerate() {
        line(format!("  #{i:<2} {}", locate(pc as usize)));
    }
    if to_file {
        let end = report_end();
        eprint!("{end}");
        text.push_str(&end);
        append_to_log(&text);
    }
    #[cfg(feature = "diagnostics")]
    for (address, len, file) in SAVE.get().into_iter().flatten() {
        // SAFETY: the caller of `save_on_crash` named readable memory.
        let bytes = unsafe { std::slice::from_raw_parts(*address as *const u8, *len) };
        let saved = std::fs::write(file, bytes);
        eprintln!(
            "  saved {address:#x}+{len:#x} to {} ({saved:?})",
            file.display()
        );
    }
}

/// Return addresses of the faulting thread's frames, from the faulting
/// instruction up, by the unwind data of each image (the Windows loader
/// registered it for title images too). Code without unwind data (our
/// thunks, leaf functions) is taken to have its return address at `rsp`.
fn unwind(context: CONTEXT) -> Vec<u64> {
    let (mut low, mut high) = (0, 0);
    // SAFETY: writes the current thread's stack bounds.
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
    let mut frames = Vec::with_capacity(MAX_FRAMES);
    unwind_into(context, low, high, &mut frames);
    frames
}

/// A `CONTEXT` at the 16-byte alignment the unwind and capture functions need.
#[repr(C, align(16))]
pub(crate) struct Aligned16(pub(crate) CONTEXT);

/// [`unwind`] for any thread of the process, its stack being `low..high`;
/// pushes no more than `MAX_FRAMES`, so it does not allocate when `frames`
/// has that capacity (another thread may be suspended holding the heap).
pub(crate) fn unwind_into(context: CONTEXT, low: usize, high: usize, frames: &mut Vec<u64>) {
    // `RtlVirtualUnwind` writes the context with 16-byte aligned stores, and
    // windows-sys declares `CONTEXT` without that alignment: a by-value copy on
    // the stack can land 8 bytes off and fault (seen in a debug build).
    let mut aligned = Aligned16(context);
    let context = &mut aligned.0;
    frames.push(context.Rip);
    while frames.len() < MAX_FRAMES {
        // Stop where reading the stack would fault inside this handler.
        let rsp = context.Rsp as usize;
        if rsp < low || rsp + 8 > high {
            break;
        }
        let mut image = 0;
        // SAFETY: looks up the function table entry covering `Rip`, if any.
        let entry = unsafe { RtlLookupFunctionEntry(context.Rip, &mut image, null_mut()) };
        if entry.is_null() {
            // SAFETY: `rsp` is inside this thread's stack.
            context.Rip = unsafe { (rsp as *const u64).read() };
            context.Rsp += 8;
        } else {
            let (mut data, mut frame) = (null_mut(), 0);
            // SAFETY: `entry` belongs to `image`; the context is our copy.
            unsafe {
                RtlVirtualUnwind(
                    UNW_FLAG_NHANDLER,
                    image,
                    context.Rip,
                    entry,
                    context,
                    &mut data,
                    &mut frame,
                    null_mut(),
                )
            };
        }
        if context.Rip == 0 {
            break;
        }
        frames.push(context.Rip);
    }
}

/// `module+rva`, or the bare address.
pub fn locate(address: usize) -> String {
    module_of(address).unwrap_or_else(|| format!("{address:#x}"))
}

fn module_of(address: usize) -> Option<String> {
    let mut module: HMODULE = std::ptr::null_mut();
    // SAFETY: looks up the module containing `address`; no reference taken.
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            address as *const u16,
            &mut module,
        )
    };
    if found == 0 {
        return None;
    }
    let mut path = [0u16; 260];
    // SAFETY: `module` is a loaded module; the buffer length is passed.
    let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) };
    let path = String::from_utf16_lossy(&path[..len as usize]);
    let name = path.rsplit('\\').next().unwrap_or(&path);
    Some(format!("{name}+{:#x}", address - module as usize))
}

/// What is at `address`, for an access violation there: nothing, memory
/// reserved but never committed, a guard page, or the protection the access
/// broke (with the module, for an image's pages). Each points at a different
/// cause: a wild pointer, an allocation whose commit failed, a stack
/// overrun, a write to code or constants.
fn memory_at(address: usize) -> String {
    use windows_sys::Win32::System::Memory::{
        MEM_FREE, MEM_IMAGE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE,
        PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD,
        PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
    };
    // SAFETY: a zeroed MEMORY_BASIC_INFORMATION is valid.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: reads the address space's bookkeeping, not `address` itself.
    let n = unsafe { VirtualQuery(address as *const _, &mut info, size_of_val(&info)) };
    if n == 0 {
        // Above the process's address space: Windows reports a
        // non-canonical address as all ones.
        return "outside the address space".into();
    }
    let protect = match info.State {
        MEM_FREE => return "nothing mapped there".into(),
        MEM_RESERVE => return "reserved, never committed".into(),
        _ if info.Protect & PAGE_GUARD != 0 => return "a guard page".into(),
        _ => match info.Protect & 0xff {
            PAGE_NOACCESS => "no access".into(),
            PAGE_READONLY => "read-only".into(),
            PAGE_READWRITE => "read-write".into(),
            PAGE_WRITECOPY => "copy-on-write".into(),
            PAGE_EXECUTE => "execute-only".into(),
            PAGE_EXECUTE_READ => "read-execute".into(),
            PAGE_EXECUTE_READWRITE => "read-write-execute".into(),
            PAGE_EXECUTE_WRITECOPY => "copy-on-write, executable".into(),
            _ => format!("protection {:#x}", info.Protect),
        },
    };
    match module_of(address) {
        Some(module) if info.Type == MEM_IMAGE => format!("{protect}, in {module}"),
        _ => protect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_RECORD, RtlCaptureContext, RtlCaptureStackBackTrace,
    };

    /// What [`capture_frames`] saw: our [`unwind`], then the system's walk.
    type Captured = (Vec<u64>, Vec<usize>);

    /// A frame we expect [`unwind`] to report: the helper it is in, and the
    /// helper's name for the message.
    type Expected = (&'static str, fn() -> Captured);

    /// The runtime function entry start covering `at`: the image base the
    /// lookup writes, plus the entry's `BeginAddress`. Asking the system for
    /// both sides of the comparison means a thunk or an incremental-link jump
    /// is not mistaken for a different function.
    fn entry_start_at(at: u64) -> Option<u64> {
        let mut image = 0;
        // SAFETY: looks up the function table entry covering `at`; nothing
        // keeps a reference to the entry past this call.
        let entry = unsafe { RtlLookupFunctionEntry(at, &mut image, null_mut()) };
        if entry.is_null() {
            return None;
        }
        // SAFETY: a non-null entry belongs to `image`, which the lookup wrote,
        // and `BeginAddress` is an offset into that image.
        Some(image + u64::from(unsafe { (*entry).BeginAddress }))
    }

    #[test]
    fn unwinds_like_the_system_walk() {
        // Asking the system's own walk whether the stack looks right is not
        // reliable: on Windows 10 x64 debug builds `RtlCaptureStackBackTrace`
        // returns no frames about half the time. The frames above us are
        // known instead (see `level_a`), so check those first.
        let (frames, walked) = level_a();
        // Frame 0 is in `capture_frames`, which took the context; then its
        // callers in call order, each named by its runtime function entry.
        let helpers: [Expected; 4] = [
            ("capture_frames", capture_frames),
            ("level_c", level_c),
            ("level_b", level_b),
            ("level_a", level_a),
        ];
        assert!(frames.len() >= helpers.len(), "{frames:?}");
        for (i, (name, helper)) in helpers.iter().enumerate() {
            let want = entry_start_at(*helper as usize as u64)
                .unwrap_or_else(|| panic!("no unwind data for {name}"));
            let got = entry_start_at(frames[i])
                .unwrap_or_else(|| panic!("no unwind data for frame {i} at {:#x}", frames[i]));
            assert_eq!(got, want, "frame {i} should be {name}: {frames:?}");
        }
        // A walk that repeats an address is not a walk.
        let mut distinct = frames[..helpers.len()].to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), helpers.len(), "repeated: {frames:?}");
        // Where the system's walk does return frames, it must still agree with
        // ours: frame 0 differs (both are in `capture_frames`, at different
        // call sites), and the callers above are the same.
        if walked.len() >= helpers.len() {
            assert!(frames.len() >= walked.len(), "{frames:?} vs {walked:?}");
            let ours: Vec<usize> = frames[1..walked.len()]
                .iter()
                .map(|&f| f as usize)
                .collect();
            assert_eq!(ours.as_slice(), &walked[1..]);
        }
    }

    /// The frames of this call, both ways: our [`unwind`], and the system's
    /// walk, which Windows 10 can come up empty on.
    #[inline(never)]
    fn capture_frames() -> Captured {
        // SAFETY: a zeroed CONTEXT is valid and both calls write buffers of
        // ours; `RtlCaptureContext` wants the context 16-byte aligned too.
        unsafe {
            let mut context = Aligned16(std::mem::zeroed());
            RtlCaptureContext(&mut context.0);
            let mut walked = [null_mut(); 8];
            let n = RtlCaptureStackBackTrace(0, 8, walked.as_mut_ptr(), null_mut());
            let frames = unwind(context.0);
            let walked = walked[..usize::from(n)]
                .iter()
                .map(|&f| f as usize)
                .collect();
            (frames, walked)
        }
    }

    /// `level_a` → `level_b` → `level_c` → [`capture_frames`]: three known
    /// callers above the capture. `black_box` after each call keeps it from
    /// becoming a tail call, which would erase the caller's frame.
    #[inline(never)]
    fn level_a() -> Captured {
        let captured = level_b();
        std::hint::black_box(&captured);
        captured
    }

    /// See [`level_a`].
    #[inline(never)]
    fn level_b() -> Captured {
        let captured = level_c();
        std::hint::black_box(&captured);
        captured
    }

    /// See [`level_a`].
    #[inline(never)]
    fn level_c() -> Captured {
        let captured = capture_frames();
        std::hint::black_box(&captured);
        captured
    }

    #[test]
    fn memory_at_names_what_an_access_hit() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY,
            PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        // SAFETY: reserves four pages of ours and commits three of them;
        // nothing reads or writes them.
        let base = unsafe {
            let base = VirtualAlloc(std::ptr::null(), 0x4000, MEM_RESERVE, PAGE_NOACCESS);
            assert!(!base.is_null());
            let page = |i: usize, protect| {
                let at = base.cast::<u8>().add(i * 0x1000).cast();
                assert!(!VirtualAlloc(at, 0x1000, MEM_COMMIT, protect).is_null());
            };
            page(1, PAGE_READONLY);
            page(2, PAGE_READWRITE | PAGE_GUARD);
            page(3, PAGE_NOACCESS);
            base as usize
        };
        assert_eq!(memory_at(base + 8), "reserved, never committed");
        assert_eq!(memory_at(base + 0x1008), "read-only");
        assert_eq!(memory_at(base + 0x2008), "a guard page");
        assert_eq!(memory_at(base + 0x3008), "no access");
        // SAFETY: releases the reservation made above.
        unsafe { VirtualFree(base as *mut _, 0, MEM_RELEASE) };
        assert_eq!(memory_at(base + 8), "nothing mapped there");
        assert_eq!(memory_at(0), "nothing mapped there");
        assert_eq!(memory_at(usize::MAX), "outside the address space");
        // This test's own code: an image page, named by its module.
        let code = memory_at(memory_at_names_what_an_access_hit as *const () as usize);
        let named = code.starts_with("read-execute, in victory_loader-");
        assert!(named, "{code}");
    }

    /// On an Intel CPU, the real instructions go through the vectored
    /// handler. On an AMD CPU they run natively, and the tests below test
    /// the emulation.
    #[test]
    fn clwb_loops() {
        // clwb [rcx]; add rcx, 0x40; cmp rcx, rax; jbe top
        let a = clwb_loop(&[
            0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x40, 0x48, 0x39, 0xc1, 0x76, 0xf3,
        ])
        .expect("rcx loop");
        assert_eq!((a.base, a.limit, a.len), (1, 0, 13));
        // clwb [r9]; add r9, 0x40; cmp r9, rax; jbe top
        let b = clwb_loop(&[
            0x66, 0x41, 0x0f, 0xae, 0x31, 0x49, 0x83, 0xc1, 0x40, 0x49, 0x39, 0xc1, 0x76, 0xf2,
        ])
        .expect("r9 loop");
        assert_eq!((b.base, b.limit, b.len), (9, 0, 14));
        // A different stride, a jump elsewhere, or a cut-off loop is not one.
        assert!(
            clwb_loop(&[
                0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x20, 0x48, 0x39, 0xc1, 0x76, 0xf3
            ])
            .is_none()
        );
        assert!(
            clwb_loop(&[
                0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x40, 0x48, 0x39, 0xc1, 0x76, 0xf0
            ])
            .is_none()
        );
        assert!(clwb_loop(&[0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x40]).is_none());
        // The flags the last cmp leaves: base just past limit.
        assert_eq!(cmp_flags(0x1040, 0x1000) & 0x41, 0); // CF and ZF clear: the loop ends
        assert_eq!(cmp_flags(0x1000, 0x1000) & 0x40, 0x40);
    }

    #[test]
    fn clwb_loop_wrapping_ranges_do_not_overflow_and_fall_back_to_single_clwb() {
        assert_eq!(clwb_loop_end(0x1000, 0x1080), Some(0x10c0));
        assert_eq!(clwb_loop_end(0x10c0, 0x1080), Some(0x1100));
        assert_eq!(clwb_loop_end(0, u64::MAX - 10), None);
        assert_eq!(clwb_loop_end(u64::MAX - 16, 0x100), None);
        assert_eq!(clwb_loop_end(u64::MAX - 63, u64::MAX), None);

        // clwb [rcx]; add rcx, 0x40; cmp rcx, rax; jbe top (13 bytes, clwb is 4 bytes).
        let mut bytes = [0xcc_u8; 16];
        bytes[..13].copy_from_slice(&[
            0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x40, 0x48, 0x39, 0xc1, 0x76, 0xf3,
        ]);
        let run = |base: u64, limit: u64| -> (u64, u64) {
            let start = bytes.as_ptr() as u64;
            // SAFETY: zeroed record/context are valid; `emulate_amd_only` only
            // reads `bytes` and updates `context`.
            unsafe {
                let mut record: EXCEPTION_RECORD = std::mem::zeroed();
                record.ExceptionCode = EXCEPTION_ILLEGAL_INSTRUCTION;
                let mut context: CONTEXT = std::mem::zeroed();
                context.Rip = start;
                context.Rcx = base;
                context.Rax = limit;
                let mut pointers = EXCEPTION_POINTERS {
                    ExceptionRecord: &mut record,
                    ContextRecord: &mut context,
                };
                assert!(emulate_amd_only(&mut pointers));
                (context.Rip - start, context.Rcx)
            }
        };
        // Normal range completes the whole 13-byte loop in one exception.
        assert_eq!(run(0x1000, 0x1080), (13, 0x10c0));
        // Overflowing/wrapping ranges must not panic or skip the loop: emulate
        // just the 4-byte `clwb` instruction and let the CPU run add/cmp/jbe.
        assert_eq!(run(0, u64::MAX - 10), (4, 0));
        assert_eq!(run(u64::MAX - 16, 0x100), (4, u64::MAX - 16));
    }

    #[test]
    fn clwb_lengths() {
        assert_eq!(clwb_len(&[0x66, 0x41, 0x0f, 0xae, 0x31]), Some(5)); // clwb [r9]
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x30]), Some(4)); // clwb [rax]
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x70, 0x40]), Some(5)); // clwb [rax+0x40]
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0xb0, 0, 1, 0, 0]), Some(8)); // [rax+disp32]
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x34, 0x24]), Some(5)); // clwb [rsp]
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x35, 0, 0, 0, 0]), Some(8)); // [rip+disp32]
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x38]), None); // clflushopt, /7
        assert_eq!(clwb_len(&[0x0f, 0xae, 0x30]), None); // xsaveopt, no 66
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0xf0]), None); // register form
    }

    #[test]
    fn a_clwb_at_the_end_of_a_page_is_decoded() {
        // Exact-length slices, as at the end of a page.
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x30]), Some(4));
        assert_eq!(clwb_len(&[0x66, 0x41, 0x0f, 0xae, 0x31]), Some(5));
        // Cut short by the slice: returns None without panicking.
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0xb0, 0, 1]), None); // disp32, 6 of 8 bytes
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x70]), None); // disp8, 4 of 5 bytes
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x34]), None); // SIB missing
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae, 0x34, 0x25, 0, 0]), None); // SIB disp32 cut short
        assert_eq!(clwb_len(&[0x66, 0x0f, 0xae]), None); // ModRM missing
        assert_eq!(clwb_len(&[0x66]), None);
        assert_eq!(clwb_len(&[]), None);

        // Emulation at the last 4 and 5 bytes of a page with an unreadable page after it.
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
            VirtualFree, VirtualProtect,
        };
        // SAFETY: two pages of ours, the second unreadable so reading past the first faults.
        unsafe {
            let page = VirtualAlloc(
                std::ptr::null(),
                0x2000,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
            .cast::<u8>();
            assert!(!page.is_null());
            let mut old = 0;
            let guard = page.add(0x1000).cast();
            assert_ne!(VirtualProtect(guard, 0x1000, PAGE_NOACCESS, &mut old), 0);
            for insn in [
                &[0x66, 0x0f, 0xae, 0x30][..],
                &[0x66, 0x41, 0x0f, 0xae, 0x31][..],
            ] {
                let end = page.add(0x1000 - insn.len());
                end.copy_from_nonoverlapping(insn.as_ptr(), insn.len());
                let mut record: EXCEPTION_RECORD = std::mem::zeroed();
                record.ExceptionCode = EXCEPTION_ILLEGAL_INSTRUCTION;
                let mut context: CONTEXT = std::mem::zeroed();
                context.Rip = end as u64;
                let mut pointers = EXCEPTION_POINTERS {
                    ExceptionRecord: &mut record,
                    ContextRecord: &mut context,
                };
                assert!(emulate_amd_only(&mut pointers));
                assert_eq!(context.Rip, end as u64 + insn.len() as u64);
            }
            VirtualFree(page.cast(), 0, MEM_RELEASE);
        }
    }

    #[test]
    fn the_host_line_names_the_cpu_and_windows() {
        // e.g. "  host: AMD EPYC 7763 64-Core Processor, Windows 10.0.26100\n"
        let line = host();
        let framed = line.starts_with("  host: ") && line.ends_with('\n');
        assert!(framed, "{line:?}");
        let inner = line.trim_start_matches("  host: ").trim_end();
        let (cpu, windows) = inner.rsplit_once(", Windows ").expect(&line);
        let named = !cpu.is_empty() && cpu == cpu.trim() && !cpu.contains('\0');
        assert!(named, "{line:?}");
        let version: Vec<u32> = windows.split('.').filter_map(|n| n.parse().ok()).collect();
        assert!(version.len() == 3 && version[0] >= 10, "{line:?}");
        println!("{line}");
    }

    #[test]
    fn clzero_zeroes_its_line_on_any_cpu() {
        report_crashes();
        #[repr(align(64))]
        struct Line([u8; 128]);
        let mut line = Line([0xff; 128]);
        let at = line.0.as_mut_ptr().wrapping_add(5); // inside the first line
        // SAFETY: clzero writes only the 64-byte line holding `at`, ours.
        unsafe { std::arch::asm!(".byte 0x0f, 0x01, 0xfc", in("rax") at) };
        assert!(line.0[..64].iter().all(|&b| b == 0));
        assert!(line.0[64..].iter().all(|&b| b == 0xff));
        // insertq xmm0, xmm1, len 16, at 16; then extrq xmm0, len 8, at 20.
        let out: u64;
        // SAFETY: touches only the named registers.
        unsafe {
            std::arch::asm!(
                "movq xmm0, {a}", "movq xmm1, {b}",
                ".byte 0xf2, 0x0f, 0x78, 0xc1, 0x10, 0x10",
                "movq {a}, xmm0",
                ".byte 0x66, 0x0f, 0x78, 0xc0, 0x08, 0x14",
                "movq {b}, xmm0",
                a = inout(reg) 0x1111_2222_3333_4444u64 => _, b = inout(reg) 0xabcdu64 => out,
                out("xmm0") _, out("xmm1") _)
        };
        assert_eq!(out, 0xbc); // 0x1111_2222_abcd_4444 >> 20, 8 bits
        // SAFETY: monitorx/mwaitx are wait hints that write no memory.
        unsafe {
            std::arch::asm!(".byte 0x0f, 0x01, 0xfa", ".byte 0x0f, 0x01, 0xfb",
                in("rax") at, in("ecx") 0, in("edx") 0)
        };
        // rdpru of MPERF: a count in edx:eax, their high halves clear.
        let (low, high): (u64, u64);
        // SAFETY: touches only the named registers.
        unsafe {
            std::arch::asm!(".byte 0x0f, 0x01, 0xfd",
                out("rax") low, inout("rcx") 0u64 => _, out("rdx") high)
        };
        assert_ne!(high << 32 | low, 0);
        assert!(high >> 32 == 0 && low >> 32 == 0);
    }

    /// The flags a native `cmp a, b` leaves.
    fn native_compare(a: u64, b: u64) -> u32 {
        let flags: u64;
        // SAFETY: compares two registers and reads the flags.
        unsafe {
            std::arch::asm!("cmp {a}, {b}", "pushfq", "pop {f}",
                a = in(reg) a, b = in(reg) b, f = out(reg) flags)
        };
        flags as u32 & CMP_FLAGS
    }

    #[test]
    fn compare_sets_flags_like_the_cpu() {
        let values = [0, 1, 0x40, 0x7f, 0x80, 0xff, 1 << 63, (1 << 63) - 1, !0, !0 - 0x40];
        for a in values {
            for b in values {
                assert_eq!(cmp_flags(a, b), native_compare(a, b), "cmp {a:#x}, {b:#x}");
            }
        }
    }

    #[test]
    fn clzero_loops_end_like_their_last_pass() {
        report_crashes();
        #[repr(align(64))]
        struct Lines([u8; 192]);
        let zeroed = |l: &Lines| [l.0[0] == 0, l.0[64] == 0, l.0[128] == 0];
        // clzero; add rax, 0x40; cmp rax, r10; jb
        let mut mem = Lines([0xff; 192]);
        let start = mem.0.as_mut_ptr() as u64;
        let (end, flags): (u64, u64);
        // SAFETY: zeroes the first two lines of `mem`.
        unsafe {
            std::arch::asm!(
                ".byte 0x0f, 0x01, 0xfc, 0x48, 0x83, 0xc0, 0x40, 0x4c, 0x39, 0xd0, 0x72, 0xf4",
                "pushfq", "pop {f}",
                inout("rax") start => end, in("r10") start + 100, f = out(reg) flags)
        };
        assert_eq!(end, start + 128);
        assert_eq!(flags as u32 & CMP_FLAGS, native_compare(end, start + 100));
        assert_eq!(zeroed(&mem), [true, true, false]);
        // mov rax, rdx; add rdx, 0x40; clzero; cmp rdx, r8; jb
        let mut mem = Lines([0xff; 192]);
        let start = mem.0.as_mut_ptr() as u64;
        let (last, next, flags): (u64, u64, u64);
        // SAFETY: zeroes the last two lines of `mem`.
        unsafe {
            std::arch::asm!(
                ".byte 0x48, 0x89, 0xd0, 0x48, 0x83, 0xc2, 0x40, 0x0f, 0x01, 0xfc",
                ".byte 0x4c, 0x39, 0xc2, 0x72, 0xf1",
                "pushfq", "pop {f}",
                out("rax") last, inout("rdx") start + 64 => next, in("r8") start + 192,
                f = out(reg) flags)
        };
        assert_eq!((last, next), (start + 128, start + 192));
        assert_eq!(flags as u32 & CMP_FLAGS, native_compare(next, start + 192));
        assert_eq!(zeroed(&mem), [false, true, true]);
    }

    fn reg(low: u64, high: u64) -> M128A {
        M128A {
            Low: low,
            High: high as i64,
        }
    }

    /// Gives `code` to the emulation as if it faulted with `exception` at
    /// its first byte, with `rax` and `xmm` as the saved registers. The
    /// emulation does not execute `code`, so this works on any CPU. Returns
    /// the number of bytes that the emulation skipped, or `None` if it did
    /// not emulate the instruction. `xmm` gets the saved registers after.
    fn emulate_as(exception: i32, code: &[u8], rax: u64, xmm: &mut [M128A; 16]) -> Option<u64> {
        // `int3` after the instruction, so that a decoder that reads too
        // far sees no valid bytes.
        let mut bytes = [0xcc_u8; 16];
        bytes[..code.len()].copy_from_slice(code);
        let start = bytes.as_ptr() as u64;
        // SAFETY: a zeroed record and context are valid; the emulation
        // reads only `bytes` and writes only `context` (and memory at
        // `rax` for clzero, which the caller owns).
        unsafe {
            let mut record: EXCEPTION_RECORD = std::mem::zeroed();
            record.ExceptionCode = exception;
            let mut context: CONTEXT = std::mem::zeroed();
            context.Rip = start;
            context.Rax = rax;
            context.Anonymous.FltSave.XmmRegisters = *xmm;
            let mut pointers = EXCEPTION_POINTERS {
                ExceptionRecord: &mut record,
                ContextRecord: &mut context,
            };
            let emulated = emulate_amd_only(&mut pointers);
            *xmm = context.Anonymous.FltSave.XmmRegisters;
            emulated.then(|| context.Rip - start)
        }
    }

    fn emulate(code: &[u8], rax: u64, xmm: &mut [M128A; 16]) -> Option<u64> {
        emulate_as(EXCEPTION_ILLEGAL_INSTRUCTION, code, rax, xmm)
    }

    fn low(xmm: &[M128A; 16], i: usize) -> u64 {
        xmm[i].Low
    }

    #[test]
    fn emulated_clzero_zeroes_its_line() {
        #[repr(align(64))]
        struct Line([u8; 128]);
        let mut line = Line([0xff; 128]);
        let at = line.0.as_mut_ptr().wrapping_add(5) as u64; // inside the first line
        let mut xmm = [reg(0, 0); 16];
        assert_eq!(emulate(&[0x0f, 0x01, 0xfc], at, &mut xmm), Some(3));
        assert!(line.0[..64].iter().all(|&b| b == 0));
        assert!(line.0[64..].iter().all(|&b| b == 0xff));
    }

    /// Gives `code` to the emulation as if it faulted at `code[at]` with
    /// `context` as the saved state, on any CPU; returns whether it
    /// emulated and leaves the state after in `context`.
    fn emulate_in(code: &[u8], at: usize, context: &mut CONTEXT) -> bool {
        // At a fixed place in a page, so the loops' neighbours are always
        // read (they are not near a page's edges).
        #[repr(align(4096))]
        struct Page([u8; 4096]);
        let mut page = Box::new(Page([0xcc; 4096]));
        let bytes = &mut page.0[0x800..];
        bytes[..code.len()].copy_from_slice(code);
        context.Rip = bytes.as_ptr() as u64 + at as u64;
        // SAFETY: a zeroed record is valid; the emulation reads only
        // `bytes` and writes `context` and the lines the caller owns.
        unsafe {
            let mut record: EXCEPTION_RECORD = std::mem::zeroed();
            record.ExceptionCode = EXCEPTION_ILLEGAL_INSTRUCTION;
            let mut pointers = EXCEPTION_POINTERS {
                ExceptionRecord: &mut record,
                ContextRecord: context,
            };
            let emulated = emulate_amd_only(&mut pointers);
            context.Rip -= bytes.as_ptr() as u64;
            emulated
        }
    }

    #[test]
    fn emulated_clzero_loops_end_like_their_last_pass() {
        #[repr(align(64))]
        struct Lines([u8; 192]);
        let zeroed = |l: &Lines| [l.0[0] == 0, l.0[64] == 0, l.0[128] == 0];
        // clzero; add rax, 0x40; cmp rax, r10; jb
        let mut mem = Lines([0xff; 192]);
        let start = mem.0.as_mut_ptr() as u64;
        // SAFETY: an all-zero context is valid.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rax = start;
        context.R10 = start + 100;
        context.EFlags = 0x202 | CMP_FLAGS;
        let code = [0x0f, 0x01, 0xfc, 0x48, 0x83, 0xc0, 0x40, 0x4c, 0x39, 0xd0, 0x72, 0xf4];
        assert!(emulate_in(&code, 0, &mut context));
        assert_eq!((context.Rip, context.Rax), (12, start + 128));
        assert_eq!(context.EFlags, 0x202 | native_compare(start + 128, start + 100));
        assert_eq!(zeroed(&mem), [true, true, false]);
        // mov rax, rdx; add rdx, 0x40; clzero; cmp rdx, r8; jb (faults at clzero,
        // after the first pass's mov and add)
        let mut mem = Lines([0xff; 192]);
        let start = mem.0.as_mut_ptr() as u64;
        // SAFETY: an all-zero context is valid.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rax = start + 64;
        context.Rdx = start + 128;
        context.R8 = start + 192;
        let code = [
            0x48, 0x89, 0xd0, 0x48, 0x83, 0xc2, 0x40, 0x0f, 0x01, 0xfc, 0x4c, 0x39, 0xc2, 0x72,
            0xf1,
        ];
        assert!(emulate_in(&code, 7, &mut context));
        assert_eq!((context.Rip, context.Rax, context.Rdx), (15, start + 128, start + 192));
        assert_eq!(context.EFlags, native_compare(start + 192, start + 192));
        assert_eq!(zeroed(&mem), [false, true, true]);
        // cmp rax, rax is not the loop (the limit would move with rax): one
        // line, flags untouched.
        let mut mem = Lines([0xff; 192]);
        // SAFETY: an all-zero context is valid.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rax = mem.0.as_mut_ptr() as u64;
        context.EFlags = 0x202;
        let code = [0x0f, 0x01, 0xfc, 0x48, 0x83, 0xc0, 0x40, 0x48, 0x39, 0xc0, 0x72, 0xf4];
        assert!(emulate_in(&code, 0, &mut context));
        assert_eq!((context.Rip, context.EFlags), (3, 0x202));
        assert_eq!(zeroed(&mem), [true, false, false]);
        // Any other neighbours: one line, then the next instruction.
        let mut mem = Lines([0xff; 192]);
        // SAFETY: an all-zero context is valid.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rax = mem.0.as_mut_ptr() as u64 + 70;
        context.R10 = !0;
        let code = [0x0f, 0x01, 0xfc, 0x48, 0x83, 0xc0, 0x20, 0x4c, 0x39, 0xd0, 0x72, 0xf4];
        assert!(emulate_in(&code, 0, &mut context));
        assert_eq!(context.Rip, 3);
        assert_eq!(zeroed(&mem), [false, true, false]);
    }

    #[test]
    fn emulated_clzero_loop_stops_when_the_limit_is_on_a_line_boundary() {
        #[repr(align(64))]
        struct Lines([u8; 256]);
        // clzero; add rax, 0x40; cmp rax, r10; jb, with r10 exactly two lines
        // on: the loop ends when rax reaches the limit (`>=`, not `>`).
        let mut mem = Lines([0xff; 256]);
        let start = mem.0.as_mut_ptr() as u64;
        // SAFETY: an all-zero context is valid.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rax = start;
        context.R10 = start + 128;
        let code = [
            0x0f, 0x01, 0xfc, 0x48, 0x83, 0xc0, 0x40, 0x4c, 0x39, 0xd0, 0x72, 0xf4,
        ];
        assert!(emulate_in(&code, 0, &mut context));
        assert_eq!((context.Rip, context.Rax), (12, start + 128));
        assert_eq!(
            context.EFlags & CMP_FLAGS,
            native_compare(start + 128, start + 128)
        );
        let zeroed: Vec<bool> = (0..4).map(|i| mem.0[i * 64] == 0).collect();
        assert_eq!(zeroed, [true, true, false, false]);
    }

    #[test]
    fn emulated_clzero_second_loop_with_extended_registers() {
        #[repr(align(64))]
        struct Lines([u8; 256]);
        // mov rax, r9; add r9, 0x40; clzero; cmp r9, r12; jb. `a` = r9 needs
        // REX.R in the mov and REX.B in the add and in the cmp; `b` = r12
        // needs REX.R in the cmp.
        let mut mem = Lines([0xff; 256]);
        let start = mem.0.as_mut_ptr() as u64;
        // SAFETY: an all-zero context is valid.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rax = start;
        context.R9 = start + 64;
        context.R12 = start + 192;
        let code = [
            0x4c, 0x89, 0xc8, 0x49, 0x83, 0xc1, 0x40, 0x0f, 0x01, 0xfc, 0x4d, 0x39, 0xe1, 0x72,
            0xf1,
        ];
        assert!(emulate_in(&code, 7, &mut context));
        assert_eq!(
            (context.Rip, context.Rax, context.R9),
            (15, start + 128, start + 192)
        );
        assert_eq!(
            context.EFlags & CMP_FLAGS,
            native_compare(start + 192, start + 192)
        );
        let zeroed: Vec<bool> = (0..4).map(|i| mem.0[i * 64] == 0).collect();
        assert_eq!(zeroed, [true, true, true, false]);
        // Every `a` in r9..r15 (r12 as the limit, so skip it as `a`).
        for a in [9_u8, 10, 11, 13, 14, 15] {
            let mut mem = Lines([0xff; 256]);
            let start = mem.0.as_mut_ptr() as u64;
            // SAFETY: an all-zero context is valid.
            let mut context: CONTEXT = unsafe { std::mem::zeroed() };
            context.Rax = start;
            *gpr(&mut context, a) = start + 64;
            context.R12 = start + 192;
            let n = a & 7;
            let code = [
                0x4c,
                0x89,
                0xc0 | n << 3,
                0x49,
                0x83,
                0xc0 | n,
                0x40,
                0x0f,
                0x01,
                0xfc,
                0x4d,
                0x39,
                0xe0 | n,
                0x72,
                0xf1,
            ];
            assert!(emulate_in(&code, 7, &mut context), "r{a}");
            assert_eq!(context.Rip, 15, "r{a}");
            assert_eq!(*gpr(&mut context, a), start + 192, "r{a}");
            assert_eq!(context.Rax, start + 128, "r{a}");
            let zeroed: Vec<bool> = (0..4).map(|i| mem.0[i * 64] == 0).collect();
            assert_eq!(zeroed, [true, true, true, false], "r{a}");
        }
    }

    #[test]
    fn emulated_clzero_loops_with_rax_as_the_limit_are_one_line() {
        #[repr(align(64))]
        struct Lines([u8; 192]);
        let zeroed = |l: &Lines| [l.0[0] == 0, l.0[64] == 0, l.0[128] == 0];
        // mov rax, rdx; add rdx, 0x40; clzero; cmp rdx, rax; jb: the limit
        // would be the line just zeroed, not a loop: one line, the next
        // instruction, flags untouched.
        let mut mem = Lines([0xff; 192]);
        let start = mem.0.as_mut_ptr() as u64;
        // SAFETY: an all-zero context is valid.
        let mut context: CONTEXT = unsafe { std::mem::zeroed() };
        context.Rax = start + 64;
        context.Rdx = start + 128;
        context.EFlags = 0x202;
        let code = [
            0x48, 0x89, 0xd0, 0x48, 0x83, 0xc2, 0x40, 0x0f, 0x01, 0xfc, 0x48, 0x39, 0xc2, 0x72,
            0xf1,
        ];
        assert!(emulate_in(&code, 7, &mut context));
        assert_eq!(
            (context.Rip, context.Rax, context.Rdx),
            (10, start + 64, start + 128)
        );
        assert_eq!(context.EFlags, 0x202);
        assert_eq!(zeroed(&mem), [false, true, false]);
    }

    #[test]
    fn emulated_rdpru_reads_known_counters_and_keeps_flags() {
        let cases = [(0, true), (1, true), (2, false), (0x1_0000_0000, true)];
        let starts = [0x202, 0x202 | CMP_FLAGS];
        for ((ecx, known), flags) in cases.into_iter().flat_map(|c| starts.map(|f| (c, f))) {
            // SAFETY: an all-zero context is valid.
            let mut context: CONTEXT = unsafe { std::mem::zeroed() };
            context.Rcx = ecx;
            context.Rax = !0;
            context.Rdx = !0;
            context.EFlags = flags;
            assert!(emulate_in(&[0x0f, 0x01, 0xfd], 0, &mut context));
            assert_eq!(context.Rip, 3);
            assert_eq!(context.EFlags, flags, "ecx {ecx:#x}");
            let count = context.Rdx << 32 | context.Rax;
            assert!(context.Rax >> 32 == 0 && context.Rdx >> 32 == 0);
            assert_eq!(count != 0, known, "ecx {ecx:#x}");
        }
    }

    #[test]
    fn emulated_monitorx_and_mwaitx_are_skipped() {
        let mut xmm = [reg(1, 2); 16];
        assert_eq!(emulate(&[0x0f, 0x01, 0xfa], 0, &mut xmm), Some(3));
        assert_eq!(emulate(&[0x0f, 0x01, 0xfb], 0, &mut xmm), Some(3));
        assert!(xmm.iter().all(|r| r.Low == 1 && r.High == 2));
    }

    #[test]
    fn emulated_insertq_and_extrq_with_immediates() {
        let mut xmm = [reg(0, 0); 16];
        xmm[0] = reg(0x1111_2222_3333_4444, 0x5555);
        xmm[1] = reg(0xabcd, 0x6666);
        // insertq xmm0, xmm1, length 16, index 16
        let insertq = [0xf2, 0x0f, 0x78, 0xc1, 0x10, 0x10];
        assert_eq!(emulate(&insertq, 0, &mut xmm), Some(6));
        assert_eq!(low(&xmm, 0), 0x1111_2222_abcd_4444);
        assert_eq!((low(&xmm, 1), xmm[1].High), (0xabcd, 0x6666));
        // extrq xmm0, length 8, index 20
        let extrq = [0x66, 0x0f, 0x78, 0xc0, 0x08, 0x14];
        assert_eq!(emulate(&extrq, 0, &mut xmm), Some(6));
        assert_eq!(low(&xmm, 0), 0xbc);
    }

    #[test]
    fn emulated_insertq_and_extrq_with_fields_in_a_register() {
        let mut xmm = [reg(0, 0); 16];
        // insertq xmm0, xmm1: length and index in xmm1's high half.
        xmm[0] = reg(0x1111_2222_3333_4444, 0);
        xmm[1] = reg(0xabcd, 0x10_10);
        assert_eq!(emulate(&[0xf2, 0x0f, 0x79, 0xc1], 0, &mut xmm), Some(4));
        assert_eq!(low(&xmm, 0), 0x1111_2222_abcd_4444);
        // extrq xmm0, xmm1: length and index in xmm1's low half.
        xmm[1] = reg(0x14_08, 0);
        assert_eq!(emulate(&[0x66, 0x0f, 0x79, 0xc1], 0, &mut xmm), Some(4));
        assert_eq!(low(&xmm, 0), 0xbc);
    }

    #[test]
    fn emulated_sse4a_reaches_xmm8_to_xmm15() {
        let mut xmm = [reg(0, 0); 16];
        xmm[8] = reg(0x1111_2222_3333_4444, 0);
        xmm[9] = reg(0xabcd, 0);
        // insertq xmm8, xmm9, length 16, index 16 (REX.R and REX.B)
        let insertq = [0xf2, 0x45, 0x0f, 0x78, 0xc1, 0x10, 0x10];
        assert_eq!(emulate(&insertq, 0, &mut xmm), Some(7));
        assert_eq!(low(&xmm, 8), 0x1111_2222_abcd_4444);
        assert_eq!((low(&xmm, 0), low(&xmm, 1)), (0, 0));
        // extrq xmm9, length 8, index 4 (REX.B)
        let extrq = [0x66, 0x41, 0x0f, 0x78, 0xc1, 0x08, 0x04];
        assert_eq!(emulate(&extrq, 0, &mut xmm), Some(7));
        assert_eq!(low(&xmm, 9), 0xbc);
        assert_eq!(low(&xmm, 1), 0);
    }

    #[test]
    fn emulated_sse4a_length_0_is_64_bits() {
        let mut xmm = [reg(0, 0); 16];
        xmm[0] = reg(0x1122_3344_5566_7788, 0);
        // extrq xmm0, length 0 (64), index 0. A non-zero index with
        // length 64 goes past bit 63, and AMD leaves that result undefined.
        assert_eq!(
            emulate(&[0x66, 0x0f, 0x78, 0xc0, 0x00, 0x00], 0, &mut xmm),
            Some(6)
        );
        assert_eq!(low(&xmm, 0), 0x1122_3344_5566_7788);
        // insertq xmm0, xmm1, length 0 (64), index 0
        xmm[1] = reg(0xdead_beef_cafe_f00d, 0);
        assert_eq!(
            emulate(&[0xf2, 0x0f, 0x78, 0xc1, 0x00, 0x00], 0, &mut xmm),
            Some(6)
        );
        assert_eq!(low(&xmm, 0), 0xdead_beef_cafe_f00d);
    }

    #[test]
    fn other_instructions_and_exceptions_are_not_emulated() {
        let mut xmm = [reg(0, 0); 16];
        assert_eq!(emulate(&[0x0f, 0x0b], 0, &mut xmm), None); // ud2
        // extrq with a memory operand, which does not exist
        assert_eq!(emulate(&[0x66, 0x0f, 0x79, 0x00], 0, &mut xmm), None);
        // extrq with immediates needs ModRM.reg 0
        let bad_extrq = [0x66, 0x0f, 0x78, 0xc8, 0x08, 0x14];
        assert_eq!(emulate(&bad_extrq, 0, &mut xmm), None);
        // a clzero that did not cause an illegal instruction
        let clzero = [0x0f, 0x01, 0xfc];
        assert_eq!(
            emulate_as(EXCEPTION_ACCESS_VIOLATION, &clzero, 0, &mut xmm),
            None
        );
    }

    #[test]
    fn instruction_bytes_stop_at_the_end_of_their_page() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
            VirtualFree, VirtualProtect,
        };
        // rdpru, then the next instruction (mov rcx, rax) and padding.
        let mut code = [0x90_u8; 16];
        code[..6].copy_from_slice(&[0x0f, 0x01, 0xfd, 0x48, 0x89, 0xc1]);
        // SAFETY: two fresh pages of ours, the second made unreadable, so a
        // read past the end of the first faults.
        unsafe {
            let page = VirtualAlloc(
                std::ptr::null(),
                0x2000,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
            .cast::<u8>();
            assert!(!page.is_null());
            let mut old = 0;
            let guard = page.add(0x1000).cast();
            assert_ne!(VirtualProtect(guard, 0x1000, PAGE_NOACCESS, &mut old), 0);
            page.copy_from_nonoverlapping(code.as_ptr(), code.len());
            let whole = "0f 01 fd 48 89 c1 90 90 90 90 90 90 90 90 90";
            assert_eq!(instruction_bytes(page as usize), whole);
            // The page's last three bytes: rdpru, and nothing read after it.
            let end = page.add(0x1000 - 3);
            end.copy_from_nonoverlapping(code.as_ptr(), 3);
            assert_eq!(instruction_bytes(end as usize), "0f 01 fd");
            VirtualFree(page.cast(), 0, MEM_RELEASE);
        }
    }
}
