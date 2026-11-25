//! Thread notices for title images. The prepared copies have no entry point
//! and no TLS callbacks, so the Windows loader tells them nothing when a
//! thread starts or ends. Our own TLS callback, which Windows calls on every
//! thread of the process whoever created it (title, host CRT or our
//! runtime), passes each notice on to every attached image: its TLS
//! callbacks, then a DLL's `DllMain`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{DLL_PROCESS_DETACH, LoadedImage};

const DLL_THREAD_ATTACH: u32 = 2;
const DLL_THREAD_DETACH: u32 = 3;

struct Mapped {
    image: Arc<LoadedImage>,
    /// Its `DLL_PROCESS_ATTACH` is done; only then does it get thread notices.
    attached: bool,
    /// `DisableThreadLibraryCalls`: no more `DllMain` thread notices.
    quiet: bool,
}

/// Every title image we mapped, in load order.
static IMAGES: Mutex<Vec<Mapped>> = Mutex::new(Vec::new());

fn images() -> std::sync::MutexGuard<'static, Vec<Mapped>> {
    IMAGES.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn mapped(image: &LoadedImage) {
    // SAFETY: reads our own static, so the linker keeps it (and its section).
    unsafe { std::ptr::from_ref(&ON_THREAD).read_volatile() };
    let image = Arc::new(image.clone());
    let mut images = images();
    images.retain(|m| m.image.base() != image.base());
    images.push(Mapped {
        image,
        attached: false,
        quiet: false,
    });
}

/// Its `DLL_PROCESS_ATTACH` is done. The list keeps attach order, which is
/// the Windows loader's initialisation order.
pub(crate) fn attached(base: usize) {
    let mut images = images();
    if let Some(i) = images.iter().position(|m| m.image.base() == base) {
        let mut m = images.remove(i);
        m.attached = true;
        images.push(m);
    }
    static AT_EXIT: std::sync::Once = std::sync::Once::new();
    // Registered before the title's own exit functions, so it runs after
    // them, as `ExitProcess` follows `exit`'s functions on Windows.
    // SAFETY: registers a function with the host CRT's exit table.
    AT_EXIT.call_once(|| unsafe {
        atexit(detach_all);
    });
}

pub(crate) fn detached(base: usize) {
    if let Some(m) = images().iter_mut().find(|m| m.image.base() == base) {
        m.attached = false;
    }
}

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

/// Set once the process is ending: no more thread notices.
static EXITING: AtomicBool = AtomicBool::new(false);

/// How long title images may take to detach before the process ends anyway
/// (a thread ended while holding a lock their detach code wants).
const DETACH_DEADLINE: Duration = Duration::from_secs(5);

/// Process exit (the CRT's `exit`, which titles call), in `ExitProcess`'s
/// order: the title's exit functions have run; every other thread ends
/// (title threads and ours); every attached image gets
/// `DLL_PROCESS_DETACH`, DLLs in reverse initialisation order, then the
/// exe's TLS callbacks; then the CRT ends the process.
extern "C" fn detach_all() {
    let attached: Vec<_> = images()
        .iter_mut()
        .filter(|m| m.attached)
        .map(|m| {
            m.attached = false;
            m.image.clone()
        })
        .collect();
    let (dlls, exes): (Vec<_>, Vec<_>) =
        attached.into_iter().rev().partition(|i| i.info().is_dll());
    tracing::info!(
        images = dlls.len() + exes.len(),
        "process exit: ending other threads, detaching title images"
    );
    EXITING.store(true, Ordering::SeqCst);
    // As on Windows, a lock a thread held when it ended stays taken; detach
    // code that waits for one would hang the exit. The watchdog runs before
    // the other threads end: a thread that ends can hold the heap lock or
    // the loader lock, and a new thread needs both to start. After the
    // deadline it only ends the process; it does not log, because logging
    // needs the heap and the log writer's lock.
    // Its thread id, set when it runs its own code; 0 until then.
    static WATCHDOG: AtomicU32 = AtomicU32::new(0);
    drop(std::thread::spawn(|| {
        // SAFETY: a plain query of the calling thread.
        let id = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        WATCHDOG.store(id, Ordering::SeqCst);
        std::thread::sleep(DETACH_DEADLINE);
        end_process();
    }));
    // Its start (loader notices, Rust's thread set-up) is done only when
    // it runs its own code. The loader notices need the loader lock. This
    // thread can hold it, or a thread that waits for this thread can hold
    // it. Then the watchdog does not start, so this wait has the same
    // deadline.
    let waiting = Instant::now();
    let watchdog = loop {
        match WATCHDOG.load(Ordering::SeqCst) {
            0 if waiting.elapsed() >= DETACH_DEADLINE => end_process(),
            0 => std::thread::yield_now(),
            id => break id,
        }
    };
    end_other_threads(&[watchdog]);
    for image in dlls.iter().chain(&exes) {
        // SAFETY: attached images are bound. A non-null third argument
        // tells the entry points the process is ending.
        unsafe {
            image.notify(
                DLL_PROCESS_DETACH,
                true,
                std::ptr::without_provenance_mut(1),
            )
        };
    }
}

/// Ends this process when the title images do not detach in time. It does
/// not use the heap or any lock: a thread that ended can hold them.
fn end_process() {
    // SAFETY: ends this process, which is exiting anyway.
    unsafe {
        windows_sys::Win32::System::Threading::TerminateProcess(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            1,
        )
    };
}

/// Ends every thread of the process but this one and those in `keep` (and
/// the ones started while doing so), as `ExitProcess` does first; except
/// the system's thread-pool workers, which `TerminateThread` refuses (the
/// process fails fast) and which run no title code (no title image uses
/// that pool).
fn end_other_threads(keep: &[u32]) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetCurrentThreadId, OpenThread, THREAD_TERMINATE, TerminateThread,
        THREAD_QUERY_INFORMATION,
    };
    // SAFETY: plain queries of the calling process and thread.
    let (process, me) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
    let mut ended = std::collections::HashSet::new();
    loop {
        // SAFETY: a snapshot of every thread, walked with a sized entry.
        let others: Vec<u32> = unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                break;
            }
            let mut entry = THREADENTRY32 {
                dwSize: size_of::<THREADENTRY32>() as u32,
                ..std::mem::zeroed()
            };
            let mut ids = Vec::new();
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == process
                    && entry.th32ThreadID != me
                    && !keep.contains(&entry.th32ThreadID)
                {
                    ids.push(entry.th32ThreadID);
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            ids
        };
        let new: Vec<u32> = others.into_iter().filter(|id| ended.insert(*id)).collect();
        if new.is_empty() {
            break;
        }
        for id in new {
            // SAFETY: a thread of this process, which is exiting.
            unsafe {
                let thread = OpenThread(THREAD_TERMINATE | THREAD_QUERY_INFORMATION, 0, id);
                if !thread.is_null() {
                    if !starts_in(thread, c"ntdll.dll") {
                        TerminateThread(thread, 0);
                    }
                    CloseHandle(thread);
                }
            }
        }
    }
}

pub(crate) fn unmapped(base: usize) {
    images().retain(|m| m.image.base() != base);
}

/// `DisableThreadLibraryCalls` for a title image; `None` if `base` is not
/// one. Fails, as on Windows, for an image with static TLS.
pub fn disable_thread_calls(base: usize) -> Option<bool> {
    let mut images = images();
    let m = images.iter_mut().find(|m| m.image.base() == base)?;
    if m.image.info().tls.is_some() {
        return Some(false);
    }
    m.quiet = true;
    Some(true)
}

/// Windows calls every pointer between the CRT's markers `.CRT$XLA` and
/// `.CRT$XLZ`; `XLAV` sorts ahead of the usual `XLB`, so ours runs before
/// any other callback linked into the exe.
#[used]
#[unsafe(link_section = ".CRT$XLAV")]
static ON_THREAD: unsafe extern "system" fn(*mut c_void, u32, *mut c_void) = on_thread;

unsafe extern "system" fn on_thread(_: *mut c_void, reason: u32, _: *mut c_void) {
    if reason != DLL_THREAD_ATTACH && reason != DLL_THREAD_DETACH {
        return;
    }
    if EXITING.load(Ordering::SeqCst) {
        return; // our exit watchdog, or threads ending with the process
    }
    // A snapshot: title code may map and attach another DLL from here.
    let notify: Vec<_> = images()
        .iter()
        .filter(|m| m.attached)
        .map(|m| (m.image.clone(), !m.quiet))
        .collect();
    if notify.is_empty() {
        return;
    }
    // Rust's std sets its threads up after this callback and aborts the
    // process if their per-thread state was touched first (the current
    // thread's id, which our logging asks for when title code calls us).
    // That state is put back as the thread started.
    let saved = (reason == DLL_THREAD_ATTACH && started_by_rust()).then(OwnTls::save);
    // Starts in load order, ends in reverse, as the Windows loader does.
    let notify_one = |(image, dll_main): &(Arc<LoadedImage>, bool)| {
        // SAFETY: attached images have their imports bound.
        unsafe { image.notify(reason, *dll_main, std::ptr::null_mut()) };
    };
    if reason == DLL_THREAD_ATTACH {
        notify.iter().for_each(notify_one);
    } else {
        notify.iter().rev().for_each(notify_one);
    }
    if let Some(saved) = saved {
        saved.restore();
    }
}

/// Our own image's static TLS block on this thread (Rust's std keeps its
/// per-thread state there), as copied before title code ran.
struct OwnTls {
    block: *mut u8,
    saved: Vec<u8>,
}

/// The TLS directory the linker emits for our image (the CRT's symbol).
#[repr(C)]
struct TlsDirectory {
    start: usize,
    end: usize,
    index: *const u32,
    callbacks: usize,
    zero_fill: u32,
    characteristics: u32,
}

unsafe extern "C" {
    static _tls_used: TlsDirectory;
}

impl OwnTls {
    fn save() -> OwnTls {
        let array: *const *mut u8;
        // SAFETY: reads TEB.ThreadLocalStoragePointer; the directory is our
        // image's, and Windows set up its block on this thread before
        // calling TLS callbacks.
        let (block, len) = unsafe {
            std::arch::asm!("mov {}, gs:[0x58]", out(reg) array, options(nostack, readonly));
            let dir = &raw const _tls_used;
            let block = *array.add((*dir).index.read() as usize);
            (block, (*dir).end - (*dir).start + (*dir).zero_fill as usize)
        };
        // SAFETY: the block is `len` bytes, live for the thread's life.
        let saved = unsafe { std::slice::from_raw_parts(block, len) }.to_vec();
        OwnTls { block, saved }
    }

    fn restore(self) {
        // SAFETY: the same block on the same thread, no reference to it held.
        unsafe { std::ptr::copy_nonoverlapping(self.saved.as_ptr(), self.block, self.saved.len()) };
    }
}

/// Whether Rust's std started this thread (its start routine is in our
/// exe): the title's main thread and our runtime's workers, which run
/// title callbacks.
fn started_by_rust() -> bool {
    // SAFETY: the calling thread's pseudo handle.
    starts_in(unsafe { windows_sys::Win32::System::Threading::GetCurrentThread() }, c"")
}

/// Whether `thread`'s start routine lies in the module named `module`
/// (empty: our exe).
fn starts_in(thread: windows_sys::Win32::Foundation::HANDLE, module: &std::ffi::CStr) -> bool {
    use windows_sys::Wdk::System::Threading::{
        NtQueryInformationThread, ThreadQuerySetWin32StartAddress,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleA, GetModuleHandleExW,
    };

    let mut start = 0usize;
    // SAFETY: writes one pointer-sized start address.
    let status = unsafe {
        NtQueryInformationThread(
            thread,
            ThreadQuerySetWin32StartAddress,
            (&raw mut start).cast(),
            size_of::<usize>() as u32,
            std::ptr::null_mut(),
        )
    };
    let mut found = std::ptr::null_mut();
    let name = if module.is_empty() {
        std::ptr::null()
    } else {
        module.as_ptr().cast()
    };
    // SAFETY: looks up the module containing an address, taking no reference.
    status == 0
        && unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                start as *const u16,
                &mut found,
            )
        } != 0
        // SAFETY: a module handle by name (or our exe's); no reference taken.
        && found == unsafe { GetModuleHandleA(name) }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::time::Duration;

    /// A thread that counts in `ticks` until it ends; returns its thread id
    /// when it has counted once.
    fn busy(ticks: &'static AtomicU64) -> u32 {
        static ID: AtomicU32 = AtomicU32::new(0);
        ID.store(0, Ordering::SeqCst);
        std::thread::spawn(move || {
            // SAFETY: a plain query of the calling thread.
            let id = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
            ID.store(id, Ordering::SeqCst);
            loop {
                ticks.fetch_add(1, Ordering::SeqCst);
                std::thread::yield_now();
            }
        });
        while ticks.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        ID.load(Ordering::SeqCst)
    }

    /// Whether `ticks` stays the same for a short time.
    fn stopped(ticks: &AtomicU64) -> bool {
        let at = ticks.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        ticks.load(Ordering::SeqCst) == at
    }

    /// In a child process of the next test: ends the other threads but one
    /// kept thread, and checks that a busy thread stopped and the kept one
    /// did not.
    #[test]
    fn exit_child() {
        if std::env::var_os("VICTORY_EXIT_CHILD").is_none() {
            return;
        }
        static ENDS: AtomicU64 = AtomicU64::new(0);
        static KEPT: AtomicU64 = AtomicU64::new(0);
        busy(&ENDS);
        let kept = busy(&KEPT);
        super::end_other_threads(&[kept]);
        let ended = stopped(&ENDS);
        let still_running = !stopped(&KEPT);
        // The test harness's threads are gone too: leave at once.
        println!(
            "{} {}",
            if ended { "ENDED" } else { "STILL RUNNING" },
            if still_running { "KEPT" } else { "NOT KEPT" }
        );
        std::process::exit(0);
    }

    #[test]
    fn exit_ends_the_other_threads_first() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "threads::tests::exit_child", "--nocapture"])
            .env("VICTORY_EXIT_CHILD", "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && text.contains("ENDED KEPT"),
            "{text}"
        );
    }
}
