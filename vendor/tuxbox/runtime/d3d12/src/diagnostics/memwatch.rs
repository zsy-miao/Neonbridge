//! Memory and thread watches, each opt-in (re-derived from palmetto 8ca2c44e
//! and rebox 54a989a, d1a9e91, 543125d). An ADDR is `game+0xRVA` or `0xVA`.
//! - TUXBOX_WRITE_WATCH=ADDR[:len]: hardware write breakpoints over up to 32
//!   bytes (four 8-byte debug registers) on every thread, re-armed every 2 s
//!   for new threads. Each write logs the writer as game+RVA, the registers and
//!   the bytes after the write (TUXBOX_WRITE_WATCH_HITS, default 64).
//! - TUXBOX_PAGE_CHECK=ADDR:len: polls up to 4 KiB every millisecond and logs
//!   each change as offset old -> new (200 lines at most). No writer, but any
//!   size and no debug registers.
//! - TUXBOX_THREAD_INFO=1: 20 s in, each thread's priority, ideal processor
//!   and group affinity, once.
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, CONTEXT_DEBUG_REGISTERS_AMD64, EXCEPTION_POINTERS, GetThreadContext, SetThreadContext,
};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, GetThreadPriority, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
    THREAD_QUERY_INFORMATION, THREAD_SET_CONTEXT, THREAD_SUSPEND_RESUME,
};

static BASE: AtomicUsize = AtomicUsize::new(0);
static WATCH: AtomicUsize = AtomicUsize::new(0);
static WATCH_LEN: AtomicUsize = AtomicUsize::new(0);
static HITS: AtomicU32 = AtomicU32::new(0);
static HIT_CAP: AtomicU32 = AtomicU32::new(64);
/// The default credit watch: filter the taker and repeat writers.
static CREDITS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WRITERS: std::sync::Mutex<Vec<(usize, u32)>> = std::sync::Mutex::new(Vec::new());
static TAKES: AtomicU32 = AtomicU32::new(0);

pub(crate) fn start() {
    let base = unsafe { GetModuleHandleW(None) }.map_or(0, |m| m.0 as usize);
    BASE.store(base, Ordering::Relaxed);
    // Default (user order, default-on): the loading-screen render's GPU
    // channel ring credits, game+0x75e5ed8, which 0x2a42d0 waits for at
    // 0x2a442d and whose releaser is reached only through a pointer. Writes
    // from 0x2a42d0 itself (the taker) are counted, not logged, and each
    // other writer RIP is logged at most 4 times, so the releaser shows.
    // TUXBOX_WRITE_WATCH=0 off.
    match std::env::var("TUXBOX_WRITE_WATCH") {
        Ok(value) if value.trim() == "0" => {}
        Ok(value) => write_watch(&value),
        Err(_) => {
            CREDITS.store(true, Ordering::Relaxed);
            write_watch("game+0x75e5ed8:8");
        }
    }
    if let Ok(value) = std::env::var("TUXBOX_PAGE_CHECK") {
        page_check(&value);
    }
    if std::env::var_os("TUXBOX_THREAD_INFO").is_some_and(|v| v == "1") {
        let _ = std::thread::Builder::new().name("TuxBox thread info".into()).spawn(|| {
            std::thread::sleep(Duration::from_secs(20));
            thread_info();
        });
    }
}

/// `game+0xRVA`, `0xVA` or a bare hex VA, with an optional `:len` (hex or decimal).
pub(crate) fn parse_address(text: &str) -> Option<(usize, Option<usize>)> {
    let (at, len) = match text.trim().split_once(':') {
        Some((a, l)) => (a, Some(l)),
        None => (text.trim(), None),
    };
    let hex = |s: &str| usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
    let address = match at.trim().strip_prefix("game+") {
        Some(rva) => BASE.load(Ordering::Relaxed).checked_add(hex(rva)?)?,
        None => hex(at)?,
    };
    let len = match len {
        Some(l) => Some(l.trim().strip_prefix("0x").map_or_else(|| l.trim().parse().ok(), |h| usize::from_str_radix(h, 16).ok())?),
        None => None,
    };
    Some((address, len))
}

pub(crate) fn label(address: usize) -> String {
    let base = BASE.load(Ordering::Relaxed);
    let size = if base == 0 { 0 } else {
        super::crash_detail::read::<u32>(base + 0x3c)
            .and_then(|nt| super::crash_detail::read::<u32>(base + nt as usize + 24 + 56)).unwrap_or(0) as usize
    };
    if address >= base && address < base + size { format!("game+{:#x}", address - base) } else { format!("{address:#x}") }
}

fn threads() -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return out };
        let pid = GetCurrentProcessId();
        let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut more = Thread32First(snap, &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid {
                out.push(entry.th32ThreadID);
            }
            more = Thread32Next(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[repr(C, align(16))]
struct Aligned(CONTEXT);

/// Debug register slots covering [address, address+len): (aligned start, DR7 LEN bits).
fn slots(address: usize, len: usize) -> Vec<(usize, u64)> {
    let mut out = Vec::new();
    let mut at = address & !7;
    // Saturate at the top of the address space (VIctory ec0d155).
    while at < address.saturating_add(len) && out.len() < 4 {
        out.push((at, 0b10)); // 8 bytes
        at += 8;
    }
    out
}

fn write_watch(value: &str) {
    let Some((address, len)) = parse_address(value) else {
        super::log_line(format_args!("write watch: cannot read {value:?} (game+0xRVA or 0xVA, optional :len)"));
        return;
    };
    let len = len.unwrap_or(8).clamp(1, 32);
    if let Some(cap) = std::env::var("TUXBOX_WRITE_WATCH_HITS").ok().and_then(|v| v.trim().parse().ok()) {
        HIT_CAP.store(cap, Ordering::Relaxed);
    }
    WATCH.store(address, Ordering::Relaxed);
    WATCH_LEN.store(len, Ordering::Relaxed);
    if unsafe { AddVectoredExceptionHandler(1, Some(handler)) }.is_null() {
        return;
    }
    let slots = slots(address, len);
    let covered = slots.last().map_or(0, |s| s.0 + 8 - address.min(slots[0].0));
    super::log_line(format_args!("write watch: {} +{len} ({} debug registers, {covered} bytes covered); bytes now {}",
        label(address), slots.len(), super::crash_detail::bytes(address, len)));
    let _ = std::thread::Builder::new().name("TuxBox write watch".into()).spawn(move || {
        let own = unsafe { GetCurrentThreadId() };
        let mut armed = HashSet::new();
        loop {
            for tid in threads() {
                if tid != own && !armed.contains(&tid) && arm(tid, &slots) {
                    armed.insert(tid);
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

fn arm(tid: u32, slots: &[(usize, u64)]) -> bool {
    let access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION;
    let Ok(handle) = (unsafe { OpenThread(access, false, tid) }) else { return false };
    let mut ok = false;
    unsafe {
        if SuspendThread(handle) != u32::MAX {
            let mut c = Box::new(Aligned(CONTEXT::default()));
            c.0.ContextFlags = CONTEXT_DEBUG_REGISTERS_AMD64;
            if GetThreadContext(handle, &mut c.0).is_ok() {
                let mut dr7 = c.0.Dr7;
                for (i, &(at, len)) in slots.iter().enumerate() {
                    match i {
                        0 => c.0.Dr0 = at as u64,
                        1 => c.0.Dr1 = at as u64,
                        2 => c.0.Dr2 = at as u64,
                        _ => c.0.Dr3 = at as u64,
                    }
                    dr7 &= !(0b11 << (2 * i) | 0b1111 << (16 + 4 * i));
                    dr7 |= 1 << (2 * i) | (0b01 | len << 2) << (16 + 4 * i); // local enable, write, length
                }
                c.0.Dr7 = dr7;
                ok = SetThreadContext(handle, &c.0).is_ok();
            }
            ResumeThread(handle);
        }
        let _ = CloseHandle(handle);
    }
    ok
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let Some(info) = (unsafe { info.as_ref() }) else { return CONTINUE_SEARCH };
    let (Some(record), Some(c)) = (unsafe { info.ExceptionRecord.as_ref() }, unsafe { info.ContextRecord.as_mut() }) else {
        return CONTINUE_SEARCH;
    };
    // Single step with one of our four DR6 bits set: a watched write just retired.
    if record.ExceptionCode.0 as u32 != 0x8000_0004 || c.Dr6 & 0xf == 0 {
        return CONTINUE_SEARCH;
    }
    if CREDITS.load(Ordering::Relaxed) {
        let rva = (c.Rip as usize).wrapping_sub(BASE.load(Ordering::Relaxed));
        if (0x2a_42d0..0x2a_4700).contains(&rva) {
            TAKES.fetch_add(1, Ordering::Relaxed);
            c.Dr6 = 0;
            return CONTINUE_EXECUTION;
        }
        // try_lock: a writer interrupted while logging must not deadlock.
        let Ok(mut writers) = WRITERS.try_lock() else {
            c.Dr6 = 0;
            return CONTINUE_EXECUTION;
        };
        let seen = match writers.iter().position(|w| w.0 == rva) {
            Some(i) => { writers[i].1 += 1; writers[i].1 }
            None if writers.len() < 64 => { writers.push((rva, 1)); 1 }
            None => u32::MAX,
        };
        drop(writers);
        if seen > 4 {
            c.Dr6 = 0;
            return CONTINUE_EXECUTION;
        }
        let stack: Vec<String> = (0..48)
            .filter_map(|i| super::crash_detail::read::<usize>(c.Rsp as usize + i * 8))
            .map(label).filter(|l| l.starts_with("game+")).take(8).collect();
        super::log_line(format_args!(
            "credit watch: {} written by {} ({seen} of 4 logged for it; {} takes by 0x2a42d0 so far), stack game returns {}",
            label(WATCH.load(Ordering::Relaxed)), label(c.Rip as usize), TAKES.load(Ordering::Relaxed), stack.join(" < ")));
    }
    let hit = HITS.fetch_add(1, Ordering::Relaxed) + 1;
    if hit <= HIT_CAP.load(Ordering::Relaxed) {
        let address = WATCH.load(Ordering::Relaxed);
        let len = WATCH_LEN.load(Ordering::Relaxed);
        super::log_line(format_args!(
            "write watch hit {hit}: {} +{len} now {} written before {} (thread {}, dr6 {:#x}): rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} \
             rsi={:#x} rdi={:#x} rbp={:#x} rsp={:#x} r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}; return {}",
            label(address), super::crash_detail::bytes(address, len), label(c.Rip as usize), unsafe { GetCurrentThreadId() }, c.Dr6,
            c.Rax, c.Rbx, c.Rcx, c.Rdx, c.Rsi, c.Rdi, c.Rbp, c.Rsp, c.R8, c.R9, c.R10, c.R11, c.R12, c.R13, c.R14, c.R15,
            super::crash_detail::read::<usize>(c.Rsp as usize).map_or("?".into(), label),
        ));
    }
    c.Dr6 = 0;
    CONTINUE_EXECUTION
}

fn page_check(value: &str) {
    let Some((address, Some(len))) = parse_address(value) else {
        super::log_line(format_args!("page check: cannot read {value:?} (ADDR:len)"));
        return;
    };
    let len = len.clamp(1, 4096);
    let _ = std::thread::Builder::new().name("TuxBox page check".into()).spawn(move || {
        let snapshot = || -> Option<Vec<u8>> {
            super::crash_detail::read::<u8>(address)?;
            super::crash_detail::read::<u8>(address + len - 1)?;
            Some(unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec())
        };
        let mut last = snapshot();
        super::log_line(format_args!("page check: {} +{len:#x} {}", label(address), if last.is_some() { "readable" } else { "not readable yet" }));
        let mut lines = 0;
        while lines < 200 {
            std::thread::sleep(Duration::from_millis(1));
            let now = snapshot();
            if now == last {
                continue;
            }
            let text = match (&last, &now) {
                (Some(a), Some(b)) => {
                    let changed: Vec<String> = (0..len).filter(|&i| a[i] != b[i]).take(16)
                        .map(|i| format!("+{i:#x} {:02x}->{:02x}", a[i], b[i])).collect();
                    let count = (0..len).filter(|&i| a[i] != b[i]).count();
                    format!("{count} bytes changed: {}", changed.join(" "))
                }
                (None, Some(_)) => "became readable".into(),
                _ => "no longer readable".into(),
            };
            super::log_line(format_args!("page check: {} {text}", label(address)));
            lines += 1;
            last = now;
        }
        super::log_line(format_args!("page check: {} 200 lines logged; stopping", label(address)));
    });
}

fn thread_info() {
    use windows::Win32::System::Kernel::PROCESSOR_NUMBER;
    use windows::Win32::System::SystemInformation::GROUP_AFFINITY;
    use windows::Win32::System::Threading::{GetThreadGroupAffinity, GetThreadIdealProcessorEx};
    let mut lines = Vec::new();
    for tid in threads() {
        let Ok(handle) = (unsafe { OpenThread(THREAD_QUERY_INFORMATION, false, tid) }) else { continue };
        let priority = unsafe { GetThreadPriority(handle) };
        let mut ideal = PROCESSOR_NUMBER::default();
        let ideal = if unsafe { GetThreadIdealProcessorEx(handle, &mut ideal) }.is_ok() { format!("{}:{}", ideal.Group, ideal.Number) } else { "?".into() };
        let mut affinity = GROUP_AFFINITY::default();
        let affinity = if unsafe { GetThreadGroupAffinity(handle, &mut affinity) }.as_bool() { format!("{}:{:#x}", affinity.Group, affinity.Mask) } else { "?".into() };
        let _ = unsafe { CloseHandle(handle) };
        lines.push(format!("{}({tid}) priority {priority} ideal {ideal} affinity {affinity}", super::threads::name_of(tid)));
    }
    super::log_line(format_args!("thread info: {} threads", lines.len()));
    for line in lines {
        super::log_line(format_args!("thread info: {line}"));
    }
}
