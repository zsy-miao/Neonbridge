//! Crash record and hang stacks, always on (TUXBOX_CRASH_RECORD=0 off).
//!
//! - Crash: an unhandled exception (filter re-asserted every second, the
//!   title's own filter still runs after ours), a Rust panic in this provider,
//!   and fail_call's fail-fast each log one record: the code, the faulting
//!   address as module+RVA, the registers and the call stack as module+RVA.
//!   RaiseFailFastException and panic=abort skip exception filters, so those
//!   two write the record at the raise site.
//! - Hang: when no frame is presented for TUXBOX_HANG_SECONDS (default 15)
//!   after frames started, every thread's stack is logged once per stall.
//!
//! The launcher's `--summary` names "module+0xRVA" from the link maps.
use super::sampler::{Module, function_start, label, module_of, modules};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_CONTROL_AMD64, EXCEPTION_POINTERS, CONTEXT_INTEGER_AMD64, GetThreadContext, LPTOP_LEVEL_EXCEPTION_FILTER,
    RtlCaptureContext, SetUnhandledExceptionFilter,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
    THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

const FRAMES: usize = 12;
const SCAN_WORDS: usize = 4096;

static PRESENTS: AtomicU64 = AtomicU64::new(0);
static PREVIOUS_FILTER: AtomicUsize = AtomicUsize::new(0);
static RECORDED: AtomicBool = AtomicBool::new(false);

/// Log line plus a copy in %LOCALAPPDATA%ixpack\logs\crash_<pid>.log, so a
/// run started without the launcher (no log capture) still leaves the record.
fn record(text: std::fmt::Arguments<'_>) {
    super::log_line(text);
    use std::io::Write;
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return };
    let dir = std::path::Path::new(&base).join("6ixpack").join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("crash_{}.log", std::process::id()));
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{text}");
    }
}

fn enabled() -> bool {
    std::env::var_os("TUXBOX_CRASH_RECORD").is_none_or(|v| v != "0")
}

/// Called on every successful present.
pub(crate) fn presented() {
    PRESENTS.fetch_add(1, Ordering::Relaxed);
}

/// Install once, when the device is created.
pub(crate) fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !enabled() {
        return;
    }
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let text = info.payload().downcast_ref::<&str>().map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
            record_here(&format!("Rust panic at {location}: {text}"));
            previous(info);
        }));
        assert_filter();
        let _ = std::thread::Builder::new().name("TuxBox watchdog".into()).spawn(watchdog);
    });
}

fn assert_filter() {
    let ours = filter as *const () as usize;
    let filter_fn: LPTOP_LEVEL_EXCEPTION_FILTER = Some(filter);
    let previous = unsafe { SetUnhandledExceptionFilter(filter_fn) };
    let previous = previous.map_or(0, |f| f as usize);
    if previous != ours {
        PREVIOUS_FILTER.store(previous, Ordering::Relaxed);
    }
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    if !info.is_null() {
        let info = unsafe { &*info };
        if !info.ExceptionRecord.is_null() && !info.ContextRecord.is_null() {
            let record = unsafe { &*info.ExceptionRecord };
            let context = unsafe { &*info.ContextRecord };
            let mut reason = format!("unhandled exception {:#010x}", record.ExceptionCode.0 as u32);
            if record.ExceptionCode.0 as u32 == 0xc000_0005 && record.NumberParameters >= 2 {
                let op = match record.ExceptionInformation[0] { 0 => "read", 1 => "write", 8 => "execute", _ => "access" };
                reason += &format!(" ({op} at {:#x})", record.ExceptionInformation[1]);
            }
            write(&reason, context, unsafe { GetCurrentThreadId() });
        }
    }
    let previous = PREVIOUS_FILTER.load(Ordering::Relaxed);
    if previous != 0 {
        type F = unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32;
        return unsafe { std::mem::transmute::<usize, F>(previous)(info) };
    }
    0 // EXCEPTION_CONTINUE_SEARCH
}

/// A record of the calling thread, for fail-fast and panics.
pub(crate) fn record_here(reason: &str) {
    if !enabled() {
        return;
    }
    let mut context = Box::new(Aligned(CONTEXT::default()));
    unsafe { RtlCaptureContext(&mut context.0) };
    write(reason, &context.0, unsafe { GetCurrentThreadId() });
}

#[repr(C, align(16))]
struct Aligned(CONTEXT);

fn write(reason: &str, context: &CONTEXT, thread: u32) {
    if RECORDED.swap(true, Ordering::Relaxed) {
        record(format_args!("crash record: {reason} (thread {thread}; a record was already written)"));
        return;
    }
    let list = modules();
    let c = context;
    record(format_args!("crash record: {reason} on thread {thread} at {}", label(&list, c.Rip as usize)));
    super::log_line(format_args!(
        "crash record: rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} rbp={:#x} rsp={:#x} r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        c.Rax, c.Rbx, c.Rcx, c.Rdx, c.Rsi, c.Rdi, c.Rbp, c.Rsp, c.R8, c.R9, c.R10, c.R11, c.R12, c.R13, c.R14, c.R15));
    let stack = unsafe { scan(c.Rsp as usize) };
    record(format_args!("crash record: stack {}", stack_text(&list, c.Rip as usize, &stack)));
}

/// Copy up to SCAN_WORDS of committed stack from rsp.
unsafe fn scan(rsp: usize) -> Vec<usize> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    if !rsp.is_multiple_of(8)
        || unsafe { VirtualQuery(Some(rsp as _), &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0
        || info.State != MEM_COMMIT
        || (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)).0 != 0
    {
        return Vec::new();
    }
    let words = ((info.BaseAddress as usize + info.RegionSize - rsp) / 8).min(SCAN_WORDS);
    let mut out = vec![0usize; words];
    unsafe { std::ptr::copy_nonoverlapping(rsp as *const usize, out.as_mut_ptr(), words) };
    out
}

/// "leaf < caller < caller ..." from return addresses into any loaded module.
fn stack_text(list: &[Module], rip: usize, stack: &[usize]) -> String {
    let mut chain = vec![label(list, rip)];
    let mut last = function_start(rip);
    for &word in stack {
        if chain.len() == FRAMES {
            break;
        }
        if let Some(module) = module_of(list, word)
            && super::sampler::follows_call(module, word)
        {
            let start = function_start(word);
            if start != last {
                last = start;
                chain.push(label(list, word));
            }
        }
    }
    chain.join(" < ")
}

fn watchdog() {
    let limit = std::env::var("TUXBOX_HANG_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(15);
    let (mut last, mut still, mut reported) = (0u64, 0u64, false);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert_filter();
        let now = PRESENTS.load(Ordering::Relaxed);
        if now != last {
            (last, still, reported) = (now, 0, false);
            continue;
        }
        still += 1;
        if now > 0 && still >= limit && !reported && limit > 0 {
            reported = true;
            hang_report(now, still);
        }
    }
}

fn hang_report(presents: u64, seconds: u64) {
    let list = modules();
    let own = unsafe { GetCurrentThreadId() };
    let mut threads = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return };
        let pid = GetCurrentProcessId();
        let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut more = Thread32First(snap, &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid && entry.th32ThreadID != own {
                threads.push(entry.th32ThreadID);
            }
            more = Thread32Next(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    record(format_args!("hang: no frame presented for {seconds} s after present #{presents}; {} threads' stacks follow", threads.len()));
    let mut context = Box::new(Aligned(CONTEXT::default()));
    let mut stack = vec![0usize; SCAN_WORDS];
    for tid in threads {
        let access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;
        let Ok(handle) = (unsafe { OpenThread(access, false, tid) }) else { continue };
        let words = unsafe { capture(handle, &mut context.0, &mut stack) };
        let _ = unsafe { CloseHandle(handle) };
        let Some(words) = words else { continue };
        let name = super::threads::name_of(tid);
        record(format_args!("hang: thread {name}({tid}): {}", stack_text(&list, context.0.Rip as usize, &stack[..words])));
    }
}

/// Suspend, read the context and copy the stack top; nothing allocates while suspended.
unsafe fn capture(handle: HANDLE, context: &mut CONTEXT, stack: &mut [usize]) -> Option<usize> {
    context.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
    if unsafe { SuspendThread(handle) } == u32::MAX {
        return None;
    }
    let mut words = 0;
    let ok = unsafe { GetThreadContext(handle, context) }.is_ok();
    if ok {
        let rsp = context.Rsp as usize;
        let mut info = MEMORY_BASIC_INFORMATION::default();
        if rsp.is_multiple_of(8)
            && unsafe { VirtualQuery(Some(rsp as _), &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0
            && info.State == MEM_COMMIT
            && (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)).0 == 0
        {
            words = ((info.BaseAddress as usize + info.RegionSize - rsp) / 8).min(stack.len());
            unsafe { std::ptr::copy_nonoverlapping(rsp as *const usize, stack.as_mut_ptr(), words) };
        }
    }
    unsafe { ResumeThread(handle) };
    ok.then_some(words)
}
