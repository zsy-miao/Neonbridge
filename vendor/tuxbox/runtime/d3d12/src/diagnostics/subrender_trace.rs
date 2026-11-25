//! Where the Subrender workers went (test173: at the final hang no thread
//! waited on their go events and no stack held a game+0x43c4xxx frame, while
//! the frame thread waited on group 2's event, set only by a worker).
//! TUXBOX_SUBRENDER_TRACE=0 off.
//!
//! - Title thread exits: every thread whose start address is in the title
//!   image is opened when first seen (a 500 ms Toolhelp scan); when its handle
//!   signals, its tid, exit code and start (game+rva) are logged. Works for
//!   a return, ExitThread and TerminateThread alike.
//! - The worker loop game+0x43c4140 waits on its go event through the
//!   WaitForSingleObject thunk (return game+0x43c421e, r15 = worker slot,
//!   slots 0x40 apart from game+0x9c3d670) and sets the job's event after a
//!   job through the SetEvent thunk at game+0x43c4318 (return game+0x43c431d,
//!   r14 = job). The title's two import slots get a naked stub that compares
//!   the return address and otherwise jumps straight on: each worker's tid at
//!   its first go wait is logged once, each SetEvent with its job, worker and
//!   handle (first 64, then every power of two).
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const WAIT_THUNK: usize = 0x1784d5; // mov edx, -1 at 0x1784d0, then jmp [IAT]
const SET_THUNK: usize = 0x5930;
const GO_RETURN: usize = 0x43c421e;
const SET_RETURN: usize = 0x43c431d;
const SLOTS: usize = 0x9c3d670;

static WAIT_REAL: AtomicUsize = AtomicUsize::new(0);
static SET_REAL: AtomicUsize = AtomicUsize::new(0);
static GO_RET: AtomicUsize = AtomicUsize::new(usize::MAX);
static SET_RET: AtomicUsize = AtomicUsize::new(usize::MAX);
static BASE: AtomicUsize = AtomicUsize::new(0);

fn on() -> bool {
    crate::env_toggle!("TUXBOX_SUBRENDER_TRACE") != Some("0")
}

/// Once, after the device is created.
pub(crate) fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !on() {
        return;
    }
    ONCE.call_once(|| {
        let Ok(module) = (unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }) else { return };
        let base = module.0 as usize;
        BASE.store(base, Ordering::Relaxed);
        let _ = std::thread::Builder::new().name("TuxBox thread exits".into()).spawn(move || exits(base));
        unsafe { hook(base) };
    });
}

/// The import slot a `jmp [rip+disp]` thunk goes through, if the bytes are one.
unsafe fn slot_of(thunk: usize) -> Option<*mut usize> {
    let p = thunk as *const u8;
    if unsafe { (*p, *p.add(1)) } != (0xff, 0x25) {
        return None;
    }
    let disp = unsafe { (p.add(2) as *const i32).read_unaligned() };
    Some((thunk as isize + 6 + disp as isize) as *mut usize)
}

/// Whether the call before `ret` is a rel32 call (this build of the title).
unsafe fn call_before(ret: usize) -> bool {
    unsafe { *((ret - 5) as *const u8) == 0xe8 }
}

unsafe fn hook(base: usize) {
    use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
    let (Some(wait), Some(set)) = (unsafe { slot_of(base + WAIT_THUNK) }, unsafe { slot_of(base + SET_THUNK) }) else {
        crate::diagnostics::log_line(format_args!("subrender trace: the import thunks are not where this build has them; not hooked"));
        return;
    };
    if !unsafe { call_before(base + GO_RETURN) && call_before(base + SET_RETURN) } {
        crate::diagnostics::log_line(format_args!("subrender trace: the worker loop is not where this build has it; not hooked"));
        return;
    }
    GO_RET.store(base + GO_RETURN, Ordering::Relaxed);
    SET_RET.store(base + SET_RETURN, Ordering::Relaxed);
    for (slot, real, stub) in [(wait, &WAIT_REAL, wait_stub as *const () as usize), (set, &SET_REAL, set_stub as *const () as usize)] {
        let mut old = PAGE_PROTECTION_FLAGS(0);
        if unsafe { VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old) }.is_err() {
            continue;
        }
        real.store(unsafe { slot.read() }, Ordering::Release);
        unsafe { (slot as *const AtomicUsize).as_ref() }.unwrap().store(stub, Ordering::Release);
        let _ = unsafe { VirtualProtect(slot.cast(), 8, old, &mut old) };
    }
    crate::diagnostics::log_line(format_args!(
        "subrender trace: worker go waits and job SetEvents logged, title thread exits watched (TUXBOX_SUBRENDER_TRACE=0 off)"));
}

#[unsafe(naked)]
unsafe extern "system" fn wait_stub() {
    core::arch::naked_asm!(
        "mov rax, [rsp]",
        "cmp rax, qword ptr [rip + {ret}]",
        "jne 2f",
        "mov r8, r15",
        "jmp {slow}",
        "2:",
        "jmp qword ptr [rip + {real}]",
        ret = sym GO_RET, real = sym WAIT_REAL, slow = sym wait_go,
    )
}

#[unsafe(naked)]
unsafe extern "system" fn set_stub() {
    core::arch::naked_asm!(
        "mov rax, [rsp]",
        "cmp rax, qword ptr [rip + {ret}]",
        "jne 2f",
        "mov rdx, r14",
        "mov r8, r15",
        "jmp {slow}",
        "2:",
        "jmp qword ptr [rip + {real}]",
        ret = sym SET_RET, real = sym SET_REAL, slow = sym set_job,
    )
}

fn worker(slot: usize) -> Option<usize> {
    let first = BASE.load(Ordering::Relaxed) + SLOTS;
    slot.checked_sub(first).filter(|o| o % 0x40 == 0 && o / 0x40 < 16).map(|o| o / 0x40)
}

extern "system" fn wait_go(handle: usize, timeout: u32, slot: usize) -> u32 {
    static SEEN: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    if let Some(i) = worker(slot) && SEEN[i].swap(u64::from(tid), Ordering::Relaxed) != u64::from(tid) {
        crate::diagnostics::log_line(format_args!(
            "subrender trace: worker {i} (slot {slot:#x}) waits on its go event {handle:#x} on thread {tid}"));
    }
    let real: extern "system" fn(usize, u32) -> u32 = unsafe { core::mem::transmute(WAIT_REAL.load(Ordering::Acquire)) };
    real(handle, timeout)
}

extern "system" fn set_job(handle: usize, job: usize, slot: usize) -> i32 {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 64 || n.is_power_of_two() {
        let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        let who = worker(slot).map_or_else(|| format!("slot {slot:#x}"), |i| format!("worker {i}"));
        crate::diagnostics::log_line(format_args!(
            "subrender trace: {who} on thread {tid} sets job {job:#x}'s event {handle:#x} ({n} so far)"));
    }
    let real: extern "system" fn(usize) -> i32 = unsafe { core::mem::transmute(SET_REAL.load(Ordering::Acquire)) };
    real(handle)
}

/// The thread's start address (ThreadQuerySetWin32StartAddress).
fn start_of(thread: windows::Win32::Foundation::HANDLE) -> usize {
    type Query = unsafe extern "system" fn(isize, u32, *mut usize, u32, *mut u32) -> i32;
    static QUERY: std::sync::OnceLock<Option<Query>> = std::sync::OnceLock::new();
    let query = QUERY.get_or_init(|| unsafe {
        let ntdll = windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!("ntdll.dll")).ok()?;
        windows::Win32::System::LibraryLoader::GetProcAddress(ntdll, windows::core::s!("NtQueryInformationThread"))
            .map(|f| core::mem::transmute::<_, Query>(f))
    });
    let mut start = 0usize;
    if let Some(query) = query {
        unsafe { query(thread.0 as isize, 9, &mut start, 8, core::ptr::null_mut()) };
    }
    start
}

fn exits(base: usize) {
    use std::collections::HashMap;
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::*;
    let size = unsafe {
        let dos = base as *const u8;
        let pe = base + (dos.add(0x3c) as *const u32).read_unaligned() as usize;
        ((pe + 0x50) as *const u32).read_unaligned() as usize
    };
    let pid = unsafe { GetCurrentProcessId() };
    let mut watched: HashMap<u32, (windows::Win32::Foundation::HANDLE, usize)> = HashMap::new();
    let mut ignored = std::collections::HashSet::new();
    loop {
        if let Ok(snap) = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) } {
            let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut more = unsafe { Thread32First(snap, &mut entry) }.is_ok();
            while more {
                let tid = entry.th32ThreadID;
                if entry.th32OwnerProcessID == pid && !watched.contains_key(&tid) && !ignored.contains(&tid)
                    && let Ok(h) = unsafe { OpenThread(THREAD_SYNCHRONIZE | THREAD_QUERY_LIMITED_INFORMATION | THREAD_QUERY_INFORMATION, false, tid) }
                {
                    let start = start_of(h);
                    if start.wrapping_sub(base) < size {
                        watched.insert(tid, (h, start));
                    } else {
                        let _ = unsafe { CloseHandle(h) };
                        ignored.insert(tid);
                    }
                }
                more = unsafe { Thread32Next(snap, &mut entry) }.is_ok();
            }
            let _ = unsafe { CloseHandle(snap) };
        }
        watched.retain(|&tid, &mut (h, start)| {
            if unsafe { WaitForSingleObject(h, 0) } != WAIT_OBJECT_0 {
                return true;
            }
            let mut code = 0u32;
            let _ = unsafe { GetExitCodeThread(h, &mut code) };
            crate::diagnostics::log_line(format_args!(
                "subrender trace: title thread {tid} exited with code {code:#x}, started at game+{:#x}", start - base));
            let _ = unsafe { CloseHandle(h) };
            false
        });
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}
