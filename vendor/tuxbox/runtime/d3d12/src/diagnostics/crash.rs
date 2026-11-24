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

/// Log line plus a copy in %LOCALAPPDATA%\6ixpack\logs\crash_<pid>.log, so a
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
            let mut fault_address = None;
            if record.ExceptionCode.0 as u32 == 0xc000_0005 && record.NumberParameters >= 2 {
                let op = match record.ExceptionInformation[0] { 0 => "read", 1 => "write", 8 => "execute", _ => "access" };
                reason += &format!(" ({op} at {:#x})", record.ExceptionInformation[1]);
                fault_address = Some(record.ExceptionInformation[1]);
            }
            if write(&reason, context, unsafe { GetCurrentThreadId() }) {
                detail(context, fault_address);
                self::record(format_args!("crash record: unwound stack {}", unwind(context)));
                if super::crash_detail::enabled() {
                    // The other threads at the fault (VIctory-style), and a small dump.
                    let lines = all_stacks();
                    self::record(format_args!("crash record: {} other threads' stacks follow", lines.len()));
                    for line in lines {
                        self::record(format_args!("crash record: {line}"));
                    }
                    minidump(info);
                }
            }
        }
    }
    let previous = PREVIOUS_FILTER.load(Ordering::Relaxed);
    if previous != 0 {
        type F = unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32;
        return unsafe { std::mem::transmute::<usize, F>(previous)(info) };
    }
    0 // EXCEPTION_CONTINUE_SEARCH
}

/// %LOCALAPPDATA%\6ixpack\logs\crash_<pid>.dmp: threads, modules, the
/// faulting context and memory the stacks point at (TUXBOX_CRASH_DUMP=0 off).
fn minidump(info: &EXCEPTION_POINTERS) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::System::Diagnostics::Debug::{
        MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE, MiniDumpScanMemory, MiniDumpWithIndirectlyReferencedMemory, MiniDumpWriteDump,
    };
    if std::env::var_os("TUXBOX_CRASH_DUMP").is_some_and(|v| v == "0") {
        return;
    }
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return };
    let path = std::path::Path::new(&base).join("6ixpack").join("logs").join(format!("crash_{}.dmp", std::process::id()));
    let Ok(file) = std::fs::File::create(&path) else { return };
    let exception = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: unsafe { GetCurrentThreadId() },
        ExceptionPointers: info as *const _ as *mut _,
        ClientPointers: false.into(),
    };
    let kind = MINIDUMP_TYPE(MiniDumpWithIndirectlyReferencedMemory.0 | MiniDumpScanMemory.0);
    let written = unsafe {
        MiniDumpWriteDump(windows::Win32::System::Threading::GetCurrentProcess(), GetCurrentProcessId(), HANDLE(file.as_raw_handle()), kind, Some(&exception), None, None)
    };
    record(format_args!("crash record: minidump {} {}", path.display(), if written.is_ok() { "written" } else { "failed" }));
}

/// The faulting thread's call chain from the images' own unwind tables
/// (.pdata via RtlLookupFunctionEntry/RtlVirtualUnwind), as module+RVA, so
/// stale return addresses left on the stack are not mistaken for frames
/// (re-derived from palmetto 7eab981a; TUXBOX_CRASH_UNWIND=0 off).
fn unwind(start: &CONTEXT) -> String {
    use windows::Win32::System::Diagnostics::Debug::{RTL_VIRTUAL_UNWIND_HANDLER_TYPE, RtlLookupFunctionEntry, RtlVirtualUnwind};
    if std::env::var_os("TUXBOX_CRASH_UNWIND").is_some_and(|v| v == "0") {
        return "off".into();
    }
    let list = modules();
    let mut c = Box::new(Aligned(*start));
    let mut chain = Vec::new();
    for _ in 0..24 {
        let rip = c.0.Rip;
        if rip == 0 {
            break;
        }
        chain.push(label(&list, rip as usize));
        let mut image = 0u64;
        let entry = unsafe { RtlLookupFunctionEntry(rip, &mut image, None) };
        if entry.is_null() {
            // A leaf function (or code without unwind data): the return address is at rsp.
            let Some(ret) = super::crash_detail::read::<u64>(c.0.Rsp as usize) else { break };
            c.0.Rip = ret;
            c.0.Rsp += 8;
        } else {
            // Unwinding reads the frame; stop rather than fault inside the filter.
            if super::crash_detail::read::<u64>(c.0.Rsp as usize).is_none() {
                break;
            }
            let mut data = core::ptr::null_mut();
            let mut frame = 0u64;
            unsafe { RtlVirtualUnwind(RTL_VIRTUAL_UNWIND_HANDLER_TYPE(0), image, rip, entry, &mut c.0, &mut data, &mut frame, None) };
        }
        if c.0.Rip == rip && chain.len() > 1 {
            break;
        }
    }
    chain.join(" < ")
}

/// A record of the calling thread, for fail-fast and panics.
pub(crate) fn record_here(reason: &str) {
    if !enabled() {
        return;
    }
    let mut context = Box::new(Aligned(CONTEXT::default()));
    unsafe { RtlCaptureContext(&mut context.0) };
    if write(reason, &context.0, unsafe { GetCurrentThreadId() }) {
        detail(&context.0, None);
    }
}

#[repr(C, align(16))]
struct Aligned(CONTEXT);

/// The faulting thread's extra detail (runs on that thread, so TEB reads are its own).
fn detail(c: &CONTEXT, fault_address: Option<usize>) {
    use super::crash_detail as d;
    if !d::enabled() {
        return;
    }
    let list = modules();
    let rip = c.Rip as usize;
    record(format_args!("crash record: bytes at rip: {}", d::instruction_bytes(rip)));
    if let Some(address) = fault_address {
        record(format_args!("crash record: fault address {address:#x} is {}", d::address_kind(&list, address)));
    }
    if let Some(export) = d::export_near(&list, rip) {
        record(format_args!("crash record: inside export {export}"));
    }
    // A call through an unresolved or stubbed import: the return address is on top
    // of the stack (execute fault) or a few words down (fault inside the stub).
    for word in unsafe { scan(c.Rsp as usize) }.into_iter().take(8) {
        if let Some(import) = d::import_called_before(&list, word) {
            record(format_args!("crash record: called through import {import}"));
            break;
        }
    }
    record(format_args!("crash record: tls {}; rax={:#x} r10={:#x} r11={:#x}", d::tls_cells(), c.Rax, c.R10, c.R11));
}

/// Writes the record; false when one was already written.
fn write(reason: &str, context: &CONTEXT, thread: u32) -> bool {
    if RECORDED.swap(true, Ordering::Relaxed) {
        record(format_args!("crash record: {reason} (thread {thread}; a record was already written)"));
        return false;
    }
    let list = modules();
    let c = context;
    record(format_args!("crash record: {reason} on thread {thread} at {}", label(&list, c.Rip as usize)));
    super::log_line(format_args!(
        "crash record: rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} rbp={:#x} rsp={:#x} r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        c.Rax, c.Rbx, c.Rcx, c.Rdx, c.Rsi, c.Rdi, c.Rbp, c.Rsp, c.R8, c.R9, c.R10, c.R11, c.R12, c.R13, c.R14, c.R15));
    let stack = unsafe { scan(c.Rsp as usize) };
    record(format_args!("crash record: stack {}", stack_text(&list, c.Rip as usize, &stack)));
    true
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
    let lines = all_stacks();
    record(format_args!("hang: no frame presented for {seconds} s after present #{presents}; {} threads' stacks follow", lines.len()));
    for line in lines {
        record(format_args!("hang: {line}"));
    }
    for line in crate::device::fence_hang_lines() {
        record(format_args!("hang: {line}"));
    }
    for line in peek_lines() {
        record(format_args!("hang: {line}"));
    }
}

/// TUXBOX_HANG_PEEK="0x9c3d660:4,0x9c3be58:8": the hang report also prints these
/// main-image offsets, so a stall in the title's own job system shows the counters
/// that decide it (unset or bad entries are skipped; read only if committed).
fn peek_lines() -> Vec<String> {
    let Ok(spec) = std::env::var("TUXBOX_HANG_PEEK") else { return Vec::new() };
    let Ok(base) = (unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }) else { return Vec::new() };
    let base = base.0 as usize;
    let mut out = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (off, len) = item.split_once(':').unwrap_or((item, "8"));
        let off = usize::from_str_radix(off.trim_start_matches("0x"), 16).ok();
        let len = len.parse::<usize>().ok().filter(|&n| n > 0 && n <= 256);
        let (Some(off), Some(len)) = (off, len) else { continue };
        let at = base + off;
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let readable = unsafe { VirtualQuery(Some(at as _), &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0
            && info.State == MEM_COMMIT
            && (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)).0 == 0
            && at + len <= info.BaseAddress as usize + info.RegionSize;
        if !readable {
            out.push(format!("peek game+{off:#x}: not readable"));
            continue;
        }
        let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, len) };
        let words: Vec<String> = bytes
            .chunks(if len % 8 == 0 { 8 } else { 4.min(len) })
            .map(|c| {
                let mut v = [0u8; 8];
                v[..c.len()].copy_from_slice(c);
                format!("{:#x}", u64::from_le_bytes(v))
            })
            .collect();
        out.push(format!("peek game+{off:#x} ({len} B): {}", words.join(" ")));
    }
    out
}

/// Every thread's stack now, one line each (for `neonbridge ctl stack`).
pub(crate) fn stacks_now() -> Vec<String> {
    let lines = all_stacks();
    super::log_line(format_args!("ctl: stack dump of {} threads", lines.len()));
    lines
}

/// Every thread's stack while a GPU memory wait is pending (five-star-recomp
/// ad6e007, GPL-3, ported): whoever should write the word is among them.
pub(crate) fn wait_stacks(address: u64, target: u64, queue: usize, waited_ms: u128, n: u32) {
    let lines = all_stacks();
    super::log_line(format_args!(
        "wait stacks #{n}: GPU memory wait {address:#x} >= {target:#x} on queue {queue:#x} pending {waited_ms} ms; {} threads' stacks follow (TUXBOX_WAIT_STACKS=0 off)",
        lines.len()));
    for line in lines {
        super::log_line(format_args!("wait stacks #{n}: {line}"));
    }
}

fn all_stacks() -> Vec<String> {
    let list = modules();
    let own = unsafe { GetCurrentThreadId() };
    let mut threads = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return Vec::new() };
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
    let mut out = Vec::new();
    let mut context = Box::new(Aligned(CONTEXT::default()));
    let mut stack = vec![0usize; SCAN_WORDS];
    for tid in threads {
        let access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;
        let Ok(handle) = (unsafe { OpenThread(access, false, tid) }) else { continue };
        let words = unsafe { capture(handle, &mut context.0, &mut stack) };
        let _ = unsafe { CloseHandle(handle) };
        let Some(words) = words else { continue };
        let name = super::threads::name_of(tid);
        let wait = waits::describe(&context.0);
        out.push(format!("thread {name}({tid}){wait}: {}", stack_text(&list, context.0.Rip as usize, &stack[..words])));
    }
    out
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

/// Names the wait a thread sits in and the objects it waits on, from its RIP and
/// argument registers: inside an ntdll wait stub the first argument is in R10
/// (the stub moves RCX there before `syscall`), the second in RDX. Object types
/// come from NtQueryObject(ObjectTypeInformation); an Event also shows its
/// signalled state. Idea from palmetto 122075ee (Widmer, GPL-3.0), whose
/// LEONIDA_WAIT_SAMPLE names waits the same way. Part of the hang stacks
/// (TUXBOX_CRASH_RECORD=0 off); TUXBOX_HANG_WAITS=0 leaves the wait out.
mod waits {
    use std::sync::OnceLock;
    use windows::Win32::System::Diagnostics::Debug::CONTEXT;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{PCSTR, w};

    type QueryObject = unsafe extern "system" fn(isize, u32, *mut u8, u32, *mut u32) -> i32;
    type QueryEvent = unsafe extern "system" fn(isize, u32, *mut u8, u32, *mut u32) -> i32;

    struct Ntdll {
        single: usize,
        multiple: usize,
        query_object: Option<QueryObject>,
        query_event: Option<QueryEvent>,
    }

    fn ntdll() -> &'static Ntdll {
        static N: OnceLock<Ntdll> = OnceLock::new();
        N.get_or_init(|| unsafe {
            let Ok(module) = GetModuleHandleW(w!("ntdll.dll")) else {
                return Ntdll { single: 0, multiple: 0, query_object: None, query_event: None };
            };
            let at = |name: &[u8]| GetProcAddress(module, PCSTR(name.as_ptr())).map_or(0, |f| f as usize);
            Ntdll {
                single: at(b"NtWaitForSingleObject\0"),
                multiple: at(b"NtWaitForMultipleObjects\0"),
                query_object: GetProcAddress(module, PCSTR(b"NtQueryObject\0".as_ptr())).map(|f| std::mem::transmute(f)),
                query_event: GetProcAddress(module, PCSTR(b"NtQueryEvent\0".as_ptr())).map(|f| std::mem::transmute(f)),
            }
        })
    }

    fn object(handle: isize) -> String {
        let n = ntdll();
        let Some(query) = n.query_object else { return format!("{handle:#x}") };
        // PUBLIC_OBJECT_TYPE_INFORMATION: UNICODE_STRING TypeName, then the name's characters.
        let mut buf = [0u64; 128];
        let mut len = 0u32;
        let status = unsafe { query(handle, 2, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, &mut len) };
        if status < 0 {
            return format!("{handle:#x} (type unknown, status {status:#x})");
        }
        let bytes = (buf[0] & 0xffff) as usize;
        let chars = buf[1] as *const u16;
        let base = buf.as_ptr() as usize;
        let inside = (chars as usize) >= base && (chars as usize) + bytes <= base + size_of_val(&buf);
        let kind = if inside { String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(chars, bytes / 2) }) } else { "?".into() };
        let mut out = format!("{handle:#x} {kind}");
        if kind == "Event" && let Some(event) = n.query_event {
            // EVENT_BASIC_INFORMATION: EventType (0 notification, 1 synchronization), EventState.
            let mut info = [0i32; 2];
            if unsafe { event(handle, 0, info.as_mut_ptr().cast(), 8, &mut len) } >= 0 {
                out.push_str(&format!(" {}{}", if info[0] == 0 { "manual" } else { "auto" }, if info[1] != 0 { " set" } else { " unset" }));
            }
        }
        out
    }

    pub(super) fn describe(c: &CONTEXT) -> String {
        if crate::env_toggle!("TUXBOX_HANG_WAITS") == Some("0") {
            return String::new();
        }
        let n = ntdll();
        let rip = c.Rip as usize;
        let near = |stub: usize| stub != 0 && rip >= stub && rip < stub + 0x40;
        if near(n.single) {
            return format!(" [NtWaitForSingleObject {}]", object(c.R10 as isize));
        }
        if near(n.multiple) {
            let count = (c.R10 as usize).min(64);
            let array = c.Rdx as *const isize;
            let mut info = windows::Win32::System::Memory::MEMORY_BASIC_INFORMATION::default();
            let readable = !array.is_null()
                && unsafe { windows::Win32::System::Memory::VirtualQuery(Some(array.cast()), &mut info, size_of_val(&info)) } != 0
                && info.State == windows::Win32::System::Memory::MEM_COMMIT
                && (array as usize) + count * 8 <= info.BaseAddress as usize + info.RegionSize;
            if !readable {
                return format!(" [NtWaitForMultipleObjects {count} objects]");
            }
            let handles = unsafe { std::slice::from_raw_parts(array, count) };
            let names: Vec<String> = handles.iter().map(|&h| object(h)).collect();
            return format!(" [NtWaitForMultipleObjects {}: {}]", if c.R8 == 0 { "all" } else { "any" }, names.join(", "));
        }
        String::new()
    }
}
