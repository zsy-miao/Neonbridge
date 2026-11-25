//! Where the submitting thread's time goes (`VICTORY_PROFILE=<file>`): a
//! thread stops it every millisecond, takes its call stack and counts it;
//! the functions most seen are written to the file every 20 seconds.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C, align(16))]
struct Context([u8; 1232]);

const RSP: usize = 0x98;
const RIP: usize = 0xF8;
const FLAGS: usize = 0x30;
const CONTEXT_FULL: u32 = 0x10_000B;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
    fn SuspendThread(thread: *mut c_void) -> u32;
    fn ResumeThread(thread: *mut c_void) -> u32;
    fn GetThreadContext(thread: *mut c_void, context: *mut Context) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> *mut c_void;
    fn Thread32First(snapshot: *mut c_void, entry: *mut u32) -> i32;
    fn Thread32Next(snapshot: *mut c_void, entry: *mut u32) -> i32;
    fn GetModuleHandleExW(flags: u32, at: u64, module: *mut u64) -> i32;
    fn GetModuleFileNameW(module: u64, name: *mut u16, len: u32) -> u32;
    fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut c_void) -> *mut c_void;
    fn RtlVirtualUnwind(
        kind: u32,
        base: u64,
        pc: u64,
        entry: *mut c_void,
        context: *mut Context,
        data: *mut *mut c_void,
        frame: *mut u64,
        pointers: *mut c_void,
    ) -> *mut c_void;
}

#[link(name = "dbghelp")]
unsafe extern "system" {
    fn SymInitialize(process: *mut c_void, path: *const u8, invade: i32) -> i32;
    fn SymSetOptions(options: u32) -> u32;
    fn SymGetLineFromAddr64(process: *mut c_void, at: u64, displacement: *mut u32, line: *mut u64) -> i32;
    fn SymFromAddr(process: *mut c_void, at: u64, displacement: *mut u64, symbol: *mut u8) -> i32;
}

/// The thread submitting now.
static THREAD: AtomicU32 = AtomicU32::new(0);
/// Whether it is inside a submission.
static INSIDE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Until dropped, the thread is in a submission.
pub(crate) struct Inside;

impl Drop for Inside {
    fn drop(&mut self) {
        INSIDE.store(false, Ordering::Relaxed);
    }
}

/// Called by the thread that submits, at each submission.
pub(crate) fn here() -> Inside {
    static FILE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let file = FILE.get_or_init(|| {
        let file = std::env::var("VICTORY_PROFILE").ok().filter(|f| !f.is_empty());
        if let Some(file) = file.clone() {
            std::thread::spawn(move || sample(&file));
        }
        file
    });
    if file.is_some() {
        // SAFETY: a plain query.
        THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
        INSIDE.store(true, Ordering::Relaxed);
    }
    Inside
}

/// The call stack of the thread `handle`, stopped meanwhile, into `stack`
/// (with room for it: nothing is allocated while a thread is stopped, which
/// may hold the heap's lock).
unsafe fn stack_of(handle: *mut c_void, context: &mut Context, stack: &mut Vec<u64>) -> bool {
    // SAFETY: the caller's handle; a context of the right size and
    // alignment; unwinding of a stopped thread's copy of its registers.
    unsafe {
        if SuspendThread(handle) == u32::MAX {
            return false;
        }
        context.0[FLAGS..FLAGS + 4].copy_from_slice(&CONTEXT_FULL.to_ne_bytes());
        if GetThreadContext(handle, context) != 0 {
            while stack.len() + 2 < stack.capacity() {
                let pc = u64::from_ne_bytes(context.0[RIP..RIP + 8].try_into().expect("8 bytes"));
                if pc == 0 {
                    break;
                }
                stack.push(pc);
                let mut base = 0u64;
                let entry = RtlLookupFunctionEntry(pc, &mut base, std::ptr::null_mut());
                if entry.is_null() {
                    // Title code or a leaf: no further.
                    break;
                }
                let (mut data, mut frame) = (std::ptr::null_mut(), 0u64);
                RtlVirtualUnwind(
                    0,
                    base,
                    pc,
                    entry,
                    context,
                    &mut data,
                    &mut frame,
                    std::ptr::null_mut(),
                );
            }
        }
        ResumeThread(handle);
    }
    true
}

/// Handles of the process's threads but this one and `but`.
fn threads(but: u32) -> Vec<*mut c_void> {
    let mut all = Vec::new();
    // SAFETY: plain calls with a record of the size given.
    unsafe {
        let (process, me) = (GetCurrentProcessId(), GetCurrentThreadId());
        let snapshot = CreateToolhelp32Snapshot(4, 0);
        let mut entry = [0u32; 7];
        entry[0] = 28;
        let mut more = Thread32First(snapshot, entry.as_mut_ptr());
        while more != 0 {
            if entry[3] == process && entry[2] != me && entry[2] != but {
                let handle = OpenThread(0x2 | 0x8 | 0x40, 0, entry[2]);
                if !handle.is_null() {
                    all.push(handle);
                }
            }
            more = Thread32Next(snapshot, entry.as_mut_ptr());
        }
        CloseHandle(snapshot);
    }
    all
}

fn sample(file: &str) {
    const DEPTH: usize = 48;
    let mut stacks: HashMap<Vec<u64>, u32> = HashMap::new();
    let mut others: HashMap<Vec<u64>, u32> = HashMap::new();
    let (mut opened, mut handle) = (0u32, std::ptr::null_mut::<c_void>());
    let mut rest: Vec<*mut c_void> = Vec::new();
    let mut listed = std::time::Instant::now();
    let mut written = std::time::Instant::now();
    let mut context = Box::new(Context([0; 1232]));
    let mut round = 0u32;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1));
        let id = THREAD.load(Ordering::Relaxed);
        if id == 0 {
            continue;
        }
        round += 1;
        // SAFETY: handles of ours.
        unsafe {
            if id != opened {
                if !handle.is_null() {
                    CloseHandle(handle);
                }
                // To suspend and resume, get the context and query.
                handle = OpenThread(0x2 | 0x8 | 0x40, 0, id);
                opened = id;
            }
            if handle.is_null() {
                continue;
            }
            let mut stack = Vec::with_capacity(DEPTH);
            let inside = INSIDE.load(Ordering::Relaxed);
            if stack_of(handle, &mut context, &mut stack) {
                stack.push(u64::from(inside));
                *stacks.entry(stack).or_insert(0) += 1;
            }
            // The other threads, less often.
            if round.is_multiple_of(16) {
                if rest.is_empty() || listed.elapsed() >= std::time::Duration::from_secs(3) {
                    for h in rest.drain(..) {
                        CloseHandle(h);
                    }
                    rest = threads(id);
                    listed = std::time::Instant::now();
                }
                for &h in &rest {
                    let mut stack = Vec::with_capacity(DEPTH);
                    if stack_of(h, &mut context, &mut stack) {
                        *others.entry(stack).or_insert(0) += 1;
                    }
                }
            }
        }
        if written.elapsed() >= std::time::Duration::from_secs(20) {
            written = std::time::Instant::now();
            write(file, &stacks, &others);
            stacks.clear();
            others.clear();
        }
    }
}

fn write(file: &str, stacks: &HashMap<Vec<u64>, u32>, others: &HashMap<Vec<u64>, u32>) {
    static READY: std::sync::Once = std::sync::Once::new();
    // SAFETY: plain calls; the symbol record has room for the name.
    let name = |at: u64| unsafe {
        if at < 2 {
            return ["[between submissions]", "[in a submission]"][at as usize].to_string();
        }
        // The module the address is in, by address and without a reference.
        let mut module = 0u64;
        let mut file = [0u16; 260];
        let module_name = if GetModuleHandleExW(0x4 | 0x2, at, &mut module) != 0 {
            let n = GetModuleFileNameW(module, file.as_mut_ptr(), 260) as usize;
            let path = String::from_utf16_lossy(&file[..n]);
            path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
        } else {
            String::new()
        };
        READY.call_once(|| {
            // Undecorated names, loaded when asked for.
            SymSetOptions(0x2 | 0x4 | 0x10);
            let mut path = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
                .unwrap_or_default()
                .into_bytes();
            path.push(0);
            SymInitialize(GetCurrentProcess(), path.as_ptr(), 1);
        });
        let mut symbol = [0u8; 88 + 512];
        symbol[0..4].copy_from_slice(&88u32.to_ne_bytes());
        symbol[80..84].copy_from_slice(&500u32.to_ne_bytes());
        let mut displacement = 0u64;
        if SymFromAddr(GetCurrentProcess(), at, &mut displacement, symbol.as_mut_ptr()) == 0 {
            return if module == 0 {
                "(title code)".to_string()
            } else {
                format!("{module_name}+{:#x}", (at - module) & !0xfff)
            };
        }
        let len = u32::from_ne_bytes(symbol[76..80].try_into().expect("4 bytes")) as usize;
        let mut line = [0u64; 5];
        line[0] = 40;
        let mut off = 0u32;
        let line = if module_name == "victory.exe"
            && SymGetLineFromAddr64(GetCurrentProcess(), at, &mut off, line.as_mut_ptr()) != 0
        {
            format!(":{}", line[2] as u32)
        } else {
            String::new()
        };
        format!("{module_name}!{}{line}", String::from_utf8_lossy(&symbol[84..84 + len.min(500)]))
    };
    let mut names: HashMap<u64, String> = HashMap::new();
    let mut own: HashMap<String, u32> = HashMap::new();
    let mut with: HashMap<String, u32> = HashMap::new();
    let mut total = 0u32;
    for (stack, &n) in stacks {
        total += n;
        let mut seen = std::collections::HashSet::new();
        for (depth, &at) in stack.iter().enumerate() {
            let text = names.entry(at).or_insert_with(|| name(at)).clone();
            if depth == 0 {
                *own.entry(text.clone()).or_insert(0) += n;
            }
            if seen.insert(text.clone()) {
                *with.entry(text).or_insert(0) += n;
            }
        }
    }
    let mut text = format!("{total} samples\n");
    for (title, counts, most) in [("in itself", own, 40), ("with what it calls", with, 90)] {
        let mut all: Vec<_> = counts.into_iter().collect();
        all.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        text.push_str(&format!("-- {title}\n"));
        for (name, n) in all.into_iter().take(most) {
            let share = f64::from(n) * 100.0 / f64::from(total.max(1));
            text.push_str(&format!("{share:5.1}% {name}\n"));
        }
    }
    // Whole stacks, the most seen first: the submitting thread's, then the
    // others' that are not only waiting.
    for (title, set, most) in [("stacks", stacks, 30), ("stacks of other threads", others, 40)] {
        let mut all: Vec<_> = set.iter().collect();
        all.sort_unstable_by(|a, b| b.1.cmp(a.1));
        text.push_str(&format!("-- {title}\n"));
        let mut shown = 0;
        for (stack, n) in all {
            let frames: Vec<String> = stack
                .iter()
                .take(18)
                .map(|&at| names.entry(at).or_insert_with(|| name(at)).clone())
                .collect();
            let idle = frames.first().is_some_and(|f| {
                ["Wait", "Delay", "RemoveIoCompletion", "WorkerFactory"].iter().any(|w| f.contains(w))
            }) && !frames.iter().any(|f| f.contains("victory_gpu"));
            if idle && !std::ptr::eq(set, stacks) {
                continue;
            }
            text.push_str(&format!("{n:6} {}\n", frames.join(" < ")));
            shown += 1;
            if shown >= most {
                break;
            }
        }
    }
    let _ = std::fs::write(file, text);
}
