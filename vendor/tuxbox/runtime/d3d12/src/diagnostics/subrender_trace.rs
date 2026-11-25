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

const GO_RETURN: usize = 0x43c421e;
const SET_RETURN: usize = 0x43c431d;
/// The render's wait on a group's event (game+0x43c3d58, through the thunk
/// at 0x1784d0); returns to game+0x43c3d5d.
const GROUP_WAIT_RETURN: usize = 0x43c3d5d;
/// Group events: CreateEventA (auto-reset) at game+0x43c39dc fills
/// game+0x9c3be78 + 0x30 * g for 128 groups (0x18..0x1818 step 0x30).
const GROUP_EVENTS: usize = 0x9c3be78;
const GROUPS: usize = 128;
const SLOTS: usize = 0x9c3d670;

static WAIT_REAL: AtomicUsize = AtomicUsize::new(0);
static SET_REAL: AtomicUsize = AtomicUsize::new(0);
static GO_RET: AtomicUsize = AtomicUsize::new(usize::MAX);
static SET_RET: AtomicUsize = AtomicUsize::new(usize::MAX);
static GROUP_WAIT_RET: AtomicUsize = AtomicUsize::new(usize::MAX);
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

/// The title's import address slot for `name`, found through its import
/// directory by name. test175: matching the thunks' bytes failed (they are
/// `48 ff 25`, REX.W jmp), so nothing was hooked.
unsafe fn import_slot(base: usize, name: &[u8]) -> Option<*mut usize> {
    let rd32 = |at: usize| unsafe { (at as *const u32).read_unaligned() } as usize;
    let pe = base + rd32(base + 0x3c);
    let (dir, size) = (rd32(pe + 0x90), rd32(pe + 0x94)); // optional header data directory 1
    if dir == 0 || size == 0 {
        return None;
    }
    let mut desc = base + dir;
    loop {
        let (names, thunks) = (rd32(desc), rd32(desc + 16));
        if thunks == 0 {
            return None;
        }
        let lookup = if names != 0 { names } else { thunks };
        let mut i = 0;
        loop {
            let entry = unsafe { ((base + lookup + i * 8) as *const u64).read_unaligned() };
            if entry == 0 {
                break;
            }
            if entry >> 63 == 0 {
                let text = base + (entry as usize & 0x7fff_ffff) + 2;
                let found = unsafe { std::ffi::CStr::from_ptr(text as *const core::ffi::c_char) }.to_bytes();
                if found == name {
                    return Some((base + thunks + i * 8) as *mut usize);
                }
            }
            i += 1;
        }
        desc += 20;
    }
}

/// Whether the call before `ret` is a rel32 call (this build of the title).
unsafe fn call_before(ret: usize) -> bool {
    unsafe { *((ret - 5) as *const u8) == 0xe8 }
}

unsafe fn hook(base: usize) {
    use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
    let (Some(wait), Some(set)) = (unsafe { import_slot(base, b"WaitForSingleObject") }, unsafe { import_slot(base, b"SetEvent") }) else {
        crate::diagnostics::log_line(format_args!("subrender trace: the title imports no WaitForSingleObject or SetEvent by name; not hooked"));
        return;
    };
    if !unsafe { call_before(base + GO_RETURN) && call_before(base + SET_RETURN) } {
        crate::diagnostics::log_line(format_args!("subrender trace: the worker loop is not where this build has it; not hooked"));
        return;
    }
    GO_RET.store(base + GO_RETURN, Ordering::Relaxed);
    SET_RET.store(base + SET_RETURN, Ordering::Relaxed);
    if unsafe { call_before(base + GROUP_WAIT_RETURN) } {
        GROUP_WAIT_RET.store(base + GROUP_WAIT_RETURN, Ordering::Relaxed);
    }
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
        "subrender trace: worker go waits and job SetEvents logged (import slots game+{:#x} was {:#x}, game+{:#x} was {:#x}), title thread exits watched (TUXBOX_SUBRENDER_TRACE=0 off)",
        wait as usize - base, WAIT_REAL.load(Ordering::Relaxed), set as usize - base, SET_REAL.load(Ordering::Relaxed)));
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
        "cmp rax, qword ptr [rip + {gret}]",
        "jne 3f",
        "jmp {group}",
        "3:",
        "jmp qword ptr [rip + {real}]",
        ret = sym GO_RET, real = sym WAIT_REAL, slow = sym wait_go, gret = sym GROUP_WAIT_RET, group = sym group_wait,
    )
}

#[unsafe(naked)]
unsafe extern "system" fn set_stub() {
    core::arch::naked_asm!(
        "mov rdx, r14",
        "mov r8, r15",
        "mov r9, [rsp]",
        "jmp {slow}",
        slow = sym set_job,
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

/// test176: no SetEvent came from game+0x43c4318 in 330 s while the loading
/// render's groups kept completing, so every SetEvent of a group event
/// (game+0x9c3be78 + 0x30 * group, 128 groups) is logged with its caller too: the first
/// 8 per (group, caller), then every power of two.
fn group_of(handle: usize) -> Option<usize> {
    let base = BASE.load(Ordering::Relaxed);
    (0..GROUPS).find(|g| unsafe { ((base + GROUP_EVENTS + g * 0x30) as *const usize).read_volatile() } == handle)
}

/// The render thread's wait on a group's event: the first 8 per group, then
/// every power of two, and every wait that took 2 s or more when it returns.
/// Static: the event (auto-reset) is set only by a Subrender worker after it
/// ran that group (game+0x43c4318 SetEvent([group+0x18]); the worker ring's
/// job is the group itself), so a wait logged here with no worker SetEvent
/// after it is the test173 deadlock.
extern "system" fn group_wait(handle: usize, timeout: u32) -> u32 {
    static N: AtomicU64 = AtomicU64::new(0);
    let real: extern "system" fn(usize, u32) -> u32 = unsafe { core::mem::transmute(WAIT_REAL.load(Ordering::Acquire)) };
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    let group = group_of(handle).map_or_else(|| "?".to_string(), |g| g.to_string());
    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "subrender trace: render waits on group {group}'s event {handle:#x} on thread {tid} ({n} group waits so far)"));
    }
    let start = std::time::Instant::now();
    let result = real(handle, timeout);
    let took = start.elapsed();
    if took.as_secs() >= 2 {
        crate::diagnostics::log_line(format_args!(
            "subrender trace: render's wait on group {group}'s event {handle:#x} took {:.1} s (wait {n}, result {result:#x})", took.as_secs_f64()));
    }
    result
}

fn group_set(handle: usize, ret: usize) {
    let base = BASE.load(Ordering::Relaxed);
    let Some(group) = group_of(handle) else { return };
    static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<(usize, usize), u64>>> = std::sync::LazyLock::new(Default::default);
    let n = {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let n = seen.entry((group, ret)).or_insert(0);
        *n += 1;
        *n
    };
    if n <= 8 || n.is_power_of_two() {
        let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        let at = ret.checked_sub(base).filter(|&r| r < 0x2000_0000).map_or(format!("{ret:#x}"), |r| format!("game+{r:#x}"));
        crate::diagnostics::log_line(format_args!(
            "subrender trace: group {group}'s event {handle:#x} set from {at} on thread {tid} ({n} from there)"));
    }
}

extern "system" fn set_job(handle: usize, job: usize, slot: usize, ret: usize) -> i32 {
    let real: extern "system" fn(usize) -> i32 = unsafe { core::mem::transmute(SET_REAL.load(Ordering::Acquire)) };
    if ret != SET_RET.load(Ordering::Relaxed) {
        group_set(handle, ret);
        return real(handle);
    }
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 64 || n.is_power_of_two() {
        let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        let who = worker(slot).map_or_else(|| format!("slot {slot:#x}"), |i| format!("worker {i}"));
        crate::diagnostics::log_line(format_args!(
            "subrender trace: {who} on thread {tid} sets job {job:#x}'s event {handle:#x}, group {} ({n} so far)",
            group_of(handle).map_or_else(|| "?".to_string(), |g| g.to_string())));
    }
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
    let mut credit = Credit::default();
    loop {
        credit.poll(base);
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

/// The frame thread's ring credit (game+0x75e5ed8, count in the high 32 bits):
/// taken one at a time in game+0x2a42d0 (it waits at 0x2a442d when none is
/// left) and given back by the submit thread game+0x2481e90 at 0x24825a2,
/// after its ExecuteCommandListsX (queue slot 35) and PresentX (slot 32).
/// Polled every 500 ms: each stretch of 2 s or more with no credit left is
/// logged once, with the word, and when credit comes back.
#[derive(Default)]
struct Credit {
    empty_since: Option<std::time::Instant>,
    told: bool,
}

const CREDIT: usize = 0x75e5ed8;

impl Credit {
    fn poll(&mut self, base: usize) {
        let word = unsafe { ((base + CREDIT) as *const AtomicU64).as_ref() }.map_or(0, |w| w.load(Ordering::Relaxed));
        let count = (word as i64) >> 32;
        if count > 0 {
            if self.told {
                crate::diagnostics::log_line(format_args!(
                    "subrender trace: ring credit back ({count} free, word {word:#x}) after {} ms with none",
                    self.empty_since.map_or(0, |t| t.elapsed().as_millis())));
            }
            *self = Self::default();
            return;
        }
        let since = *self.empty_since.get_or_insert_with(std::time::Instant::now);
        if !self.told && since.elapsed().as_secs() >= 2 {
            self.told = true;
            crate::diagnostics::log_line(format_args!(
                "subrender trace: no ring credit for 2 s (word game+{CREDIT:#x} = {word:#x}): the frame thread waits in game+0x2a42d0 until the submit thread game+0x2481e90 gives one back"));
        }
    }
}
