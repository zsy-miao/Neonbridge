//! Copied from ztrx victory-loader crash.rs:85-140 and 359-660 (GPL-3.0,
//! credit to its authors): names the faulting code when title code crashes.
//!
//! Differences, and why:
//! - Output goes to `crate::loader::log` (xmem's log) instead of eprintln and
//!   the leader's crash-log file, so it lands with the rest of our lines.
//! - Only the vectored handler is installed, at the leader's position
//!   (`AddVectoredExceptionHandler(0, ..)`, last in the chain). The leader's
//!   SetUnhandledExceptionFilter is left out: tuxbox-d3d12's
//!   diagnostics/crash.rs already owns the unhandled filter, and a second one
//!   would replace it.
//! - The AMD-only instruction emulation in the leader's handler is skipped:
//!   console_cpu.rs does that work ahead of us (it is first in the chain).
//! - Each (code, address) pair is reported once, and at most `MAX_REPORTS`
//!   reports are made in all, so a fault storm cannot flood the log; the
//!   leader's MAX_FRAMES (40) and SAID_LINES (8) limits are kept.
//! - The leader's `diagnostics` memory saving and panic hook are left out.
//! - `note_title_output` has no caller yet (the title's debug output is
//!   captured in a file another coder owns); until it is fed, reports carry
//!   no "title said" lines.
//! - The handler never swallows an exception: it always returns
//!   EXCEPTION_CONTINUE_SEARCH.
//! TUXBOX_CRASH_REPORT=0 installs nothing (default on).

use std::ptr::null_mut;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_BREAKPOINT, EXCEPTION_ILLEGAL_INSTRUCTION,
    EXCEPTION_INT_DIVIDE_BY_ZERO, EXCEPTION_PRIV_INSTRUCTION, EXCEPTION_STACK_OVERFLOW, HMODULE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, EXCEPTION_POINTERS, RtlLookupFunctionEntry,
    RtlVirtualUnwind, UNW_FLAG_NHANDLER,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadStackLimits;

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// Frames listed per report.
const MAX_FRAMES: usize = 40;
/// Reports made in all (ours, not the leader's: it prints every one).
const MAX_REPORTS: usize = 32;

/// (code, address) pairs already reported.
static SEEN: Mutex<Vec<(i32, usize)>> = Mutex::new(Vec::new());

/// The title's last lines of debug output, kept for crash reports.
static SAID: Mutex<std::collections::VecDeque<String>> =
    Mutex::new(std::collections::VecDeque::new());
const SAID_LINES: usize = 8;

/// Keeps a line of the title's debug output for the next crash report.
#[allow(dead_code)]
pub(crate) fn note_title_output(line: &str) {
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
fn title_output() -> Vec<String> {
    let Ok(said) = SAID.try_lock() else {
        return Vec::new();
    };
    said.iter().map(|l| format!("  title said: {l}")).collect()
}

/// Installs the handler; call once before running title code.
pub(crate) fn install() {
    if std::env::var("TUXBOX_CRASH_REPORT").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    // SAFETY: registers a handler that only reads the exception it is given.
    let installed = unsafe { AddVectoredExceptionHandler(0, Some(handler)) };
    if installed.is_null() {
        crate::loader::log("crash report handler was not installed");
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

/// Whether this (code, address) is new and the report budget lasts; never
/// blocks (the faulting thread may hold the lock).
fn first_time(code: i32, at: usize) -> bool {
    let Ok(mut seen) = SEEN.try_lock() else {
        return false;
    };
    if seen.len() >= MAX_REPORTS || seen.contains(&(code, at)) {
        return false;
    }
    seen.push((code, at));
    true
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes valid exception pointers.
    let (code, at) = unsafe {
        let r = &*(*info).ExceptionRecord;
        (r.ExceptionCode, r.ExceptionAddress as usize)
    };
    if FATAL.contains(&code) && first_time(code, at) {
        // SAFETY: as above.
        unsafe { report("first-chance exception", info) };
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// The bytes at `at` as hex, as many as the longest x86 instruction (15),
/// but none past the end of `at`'s page: the next page may not be mapped.
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
/// `info` comes from the system, for an exception being dispatched.
unsafe fn report(what: &str, info: *const EXCEPTION_POINTERS) {
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
    // Logged line by line: a fault while unwinding must not lose the lines
    // before it.
    let line = |l: String| crate::loader::log(&l);
    line(format!(
        "crash report: {what} {:#010x} at {}{detail}",
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
    for said in title_output() {
        line(said);
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
struct Aligned16(CONTEXT);

/// [`unwind`] for any thread of the process, its stack being `low..high`;
/// pushes no more than `MAX_FRAMES`, so it does not allocate when `frames`
/// has that capacity (another thread may be suspended holding the heap).
fn unwind_into(context: CONTEXT, low: usize, high: usize, frames: &mut Vec<u64>) {
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
fn locate(address: usize) -> String {
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
