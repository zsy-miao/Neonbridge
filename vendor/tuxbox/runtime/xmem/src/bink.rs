//! Diagnostics for the intro movies: the exe's Bink2Scarlett calls, timed.
//! test54: the intro audio has gaps while the loading-screen audio is fine,
//! and Bink makes no XAudio2 voice or XMA context of its own. The exe hands
//! Bink its own sound system (BinkSetSoundSystem; it imports no
//! BinkOpenXAudio2), and Bink decodes a movie's audio with its frames, so
//! intro audio starves when the title's frame loop runs behind the movie.
//! This logs what the sound system is (its address's module and offset) and,
//! every 10 s, how often each call ran, how long the waits took, how many
//! frames were skipped, and the movie's frame advance against its rate.
//! TUXBOX_BINK_TRACE=0 leaves Bink's imports alone.
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::loader::{log, patch_imports};

type F4 = unsafe extern "system" fn(u64, u64, u64, u64) -> u64;

/// The traced exports; the wrapper of each calls REAL[i].
const NAMES: [&str; 9] = [
    "BinkSetSoundSystem", "BinkDoFrame", "BinkNextFrame", "BinkWait", "BinkShouldSkip",
    "BinkDoFrameAsyncMulti", "BinkDoFrameAsyncWait", "BinkOpenWithOptions", "BinkSetSoundTrack",
];
const SOUND_SYSTEM: usize = 0;
const WAIT: usize = 3;
const SHOULD_SKIP: usize = 4;
const OPEN: usize = 7;
static REAL: [AtomicUsize; 9] = [const { AtomicUsize::new(0) }; 9];

#[derive(Default, Clone, Copy)]
struct Call {
    count: u32,
    nonzero: u32,
    total_us: u64,
    max_us: u64,
}

struct Window {
    started: Instant,
    calls: [Call; 9],
    /// The movie handle last seen, and its frame number then.
    movie: u64,
    first_frame: Option<u32>,
    last_frame: u32,
}

static WINDOW: Mutex<Option<Window>> = Mutex::new(None);

fn module_of(address: u64) -> String {
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
    }
    let mut module = std::ptr::null_mut();
    // FROM_ADDRESS | UNCHANGED_REFCOUNT
    if address == 0 || unsafe { GetModuleHandleExW(0x4 | 0x2, address as *const c_void, &mut module) } == 0 {
        return format!("{address:#x}");
    }
    let mut buf = [0u16; 260];
    let n = unsafe { crate::loader::module_file_name(module, &mut buf) };
    let name = String::from_utf16_lossy(&buf[..n]);
    let base = name.rsplit(['\\', '/']).next().unwrap_or(&name).to_string();
    format!("{base}+{:#x}", address - module as u64)
}

/// HBINK starts Width, Height, Frames, FrameNum, LastFrameNum, FrameRate,
/// FrameRateDiv (Bink 2 SDK layout, read as a hint).
fn header(movie: u64) -> Option<[u32; 7]> {
    if movie < 0x10000 {
        return None;
    }
    let mut out = [0u32; 7];
    let mut read = 0usize;
    unsafe extern "system" {
        fn ReadProcessMemory(process: isize, base: *const c_void, buffer: *mut c_void, size: usize, read: *mut usize) -> i32;
    }
    (unsafe { ReadProcessMemory(-1, movie as *const c_void, out.as_mut_ptr().cast(), 28, &mut read) } != 0 && read == 28).then_some(out)
}

fn record(index: usize, movie: u64, result: u64, us: u64) {
    let mut guard = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let window = guard.get_or_insert_with(|| Window { started: now, calls: [Call::default(); 9], movie: 0, first_frame: None, last_frame: 0 });
    let call = &mut window.calls[index];
    call.count += 1;
    call.nonzero += u32::from(result as u32 != 0);
    call.total_us += us;
    call.max_us = call.max_us.max(us);
    if (1..=6).contains(&index) && let Some(h) = header(movie) {
        if window.movie != movie {
            window.movie = movie;
            window.first_frame = None;
        }
        window.first_frame.get_or_insert(h[3]);
        window.last_frame = h[3];
    }
    let seconds = now.duration_since(window.started).as_secs_f64();
    if seconds < 10.0 {
        return;
    }
    let parts: Vec<String> = NAMES.iter().zip(&window.calls).enumerate().filter(|(_, (_, c))| c.count != 0).map(|(i, (name, c))| {
        let what = match i {
            WAIT => " (nonzero: not yet time)",
            SHOULD_SKIP => " (nonzero: skip)",
            _ => " (nonzero)",
        };
        format!("{name} {:.1}/s mean {:.2} ms max {:.1} ms, {}{what}", f64::from(c.count) / seconds,
            c.total_us as f64 / 1e3 / f64::from(c.count), c.max_us as f64 / 1e3, c.nonzero)
    }).collect();
    let movie_line = match (header(window.movie), window.first_frame) {
        (Some(h), Some(first)) => format!("; movie {}x{} frame {} of {}, {:.1} frames/s against {:.2} nominal",
            h[0], h[1], h[3], h[2], f64::from(window.last_frame.wrapping_sub(first)) / seconds,
            if h[6] == 0 { 0.0 } else { f64::from(h[5]) / f64::from(h[6]) }),
        _ => String::new(),
    };
    log(&format!("bink: last {seconds:.1} s: {}{movie_line}; Bink's own waits: {}", if parts.is_empty() { "no calls".into() } else { parts.join(", ") }, take_waits(seconds)));
    *window = Window { started: now, calls: [Call::default(); 9], movie: window.movie, first_frame: None, last_frame: window.last_frame };
}

unsafe fn traced(index: usize, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let real: F4 = unsafe { std::mem::transmute(REAL[index].load(Ordering::Acquire)) };
    let start = Instant::now();
    let result = unsafe { real(a, b, c, d) };
    let us = start.elapsed().as_micros() as u64;
    match index {
        SOUND_SYSTEM => log(&format!("bink: BinkSetSoundSystem(open {}, param {b:#x}) = {result:#x}", module_of(a))),
        OPEN => log(&format!("bink: BinkOpenWithOptions = {result:#x} in {:.1} ms{}", us as f64 / 1e3,
            header(result).map_or(String::new(), |h| format!(", {}x{} {} frames at {}/{}", h[0], h[1], h[2], h[5], h[6])))),
        8 => log(&format!("bink: BinkSetSoundTrack({a}, {b:#x}) = {result:#x}")),
        _ => record(index, a, result, us),
    }
    result
}

macro_rules! wrapper {
    ($name:ident, $i:expr) => {
        unsafe extern "system" fn $name(a: u64, b: u64, c: u64, d: u64) -> u64 {
            unsafe { traced($i, a, b, c, d) }
        }
    };
}
wrapper!(w0, 0);
wrapper!(w1, 1);
wrapper!(w2, 2);
wrapper!(w3, 3);
wrapper!(w4, 4);
wrapper!(w5, 5);
wrapper!(w6, 6);
wrapper!(w7, 7);
wrapper!(w8, 8);
const WRAPPERS: [unsafe extern "system" fn(u64, u64, u64, u64) -> u64; 9] = [w0, w1, w2, w3, w4, w5, w6, w7, w8];

/// Called from loader::install, after the exe's imports are bound.
pub(crate) unsafe fn install() {
    if std::env::var("TUXBOX_BINK_TRACE").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }
    let dll: Vec<u16> = "bink2scarlett.dll".encode_utf16().chain([0]).collect();
    let module = unsafe { GetModuleHandleW(dll.as_ptr()) };
    if module.is_null() {
        log("bink: Bink2Scarlett.dll not loaded at install; calls not traced");
        return;
    }
    let mut hooks = Vec::new();
    for (i, name) in NAMES.iter().enumerate() {
        let c = format!("{name}\0");
        let real = unsafe { GetProcAddress(module, c.as_ptr()) } as usize;
        if real != 0 {
            REAL[i].store(real, Ordering::Release);
            hooks.push((*name, WRAPPERS[i] as usize));
        }
    }
    let n = unsafe { patch_imports(|d| d.starts_with("bink2scarlett"), &hooks) };
    log(&format!("bink: {n} Bink2Scarlett import(s) of the exe traced (TUXBOX_BINK_TRACE=0 skips)"));
    unsafe { trace_waits(module) };
}

// ---- Bink's own waits: Sleep, WaitForSingleObject, SleepConditionVariableCS ----

#[derive(Default, Clone, Copy)]
struct Wait {
    count: u32,
    timed: u32,
    /// Timed waits that returned more than 2 ms after their timeout.
    late: u32,
    asked_us: u64,
    took_us: u64,
    worst_late_us: u64,
}

const WAIT_NAMES: [&str; 3] = ["Sleep", "WaitForSingleObject", "SleepConditionVariableCS"];
static WAITS: Mutex<[Wait; 3]> = Mutex::new([Wait { count: 0, timed: 0, late: 0, asked_us: 0, took_us: 0, worst_late_us: 0 }; 3]);
static REAL_SLEEP: AtomicUsize = AtomicUsize::new(0);
static REAL_WAIT: AtomicUsize = AtomicUsize::new(0);
static REAL_SLEEP_CV: AtomicUsize = AtomicUsize::new(0);
const INFINITE: u32 = u32::MAX;

fn note_wait(index: usize, ms: u32, took: std::time::Duration) {
    let took_us = took.as_micros() as u64;
    let mut waits = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    let w = &mut waits[index];
    w.count += 1;
    w.took_us += took_us;
    if ms != INFINITE {
        w.timed += 1;
        let asked = u64::from(ms) * 1000;
        w.asked_us += asked;
        if took_us > asked + 2000 && (index == 0 || took_us >= asked) {
            w.late += 1;
            w.worst_late_us = w.worst_late_us.max(took_us - asked);
        }
    }
}

fn take_waits(seconds: f64) -> String {
    let waits = std::mem::take(&mut *WAITS.lock().unwrap_or_else(|e| e.into_inner()));
    let parts: Vec<String> = WAIT_NAMES.iter().zip(waits).filter(|(_, w)| w.count != 0).map(|(name, w)| format!(
        "{name} {:.1}/s took {:.2} ms mean, {} timed asking {:.2} ms mean, {} over 2 ms late (worst {:.1} ms)",
        f64::from(w.count) / seconds, w.took_us as f64 / 1e3 / f64::from(w.count), w.timed,
        if w.timed == 0 { 0.0 } else { w.asked_us as f64 / 1e3 / f64::from(w.timed) }, w.late, w.worst_late_us as f64 / 1e3)).collect();
    if parts.is_empty() { "none".into() } else { parts.join(", ") }
}

unsafe extern "system" fn bink_sleep(ms: u32) {
    let real: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(REAL_SLEEP.load(Ordering::Acquire)) };
    let start = Instant::now();
    unsafe { real(ms) };
    note_wait(0, ms, start.elapsed());
}

unsafe extern "system" fn bink_wait(handle: *mut c_void, ms: u32) -> u32 {
    let real: unsafe extern "system" fn(*mut c_void, u32) -> u32 = unsafe { std::mem::transmute(REAL_WAIT.load(Ordering::Acquire)) };
    let start = Instant::now();
    let r = unsafe { real(handle, ms) };
    // Only a timeout says the wait ran its full time.
    note_wait(1, if r == 0x102 { ms } else { INFINITE }, start.elapsed());
    r
}

unsafe extern "system" fn bink_sleep_cv(cv: *mut c_void, cs: *mut c_void, ms: u32) -> i32 {
    let real: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> i32 = unsafe { std::mem::transmute(REAL_SLEEP_CV.load(Ordering::Acquire)) };
    let start = Instant::now();
    let r = unsafe { real(cv, cs, ms) };
    note_wait(2, if r == 0 { ms } else { INFINITE }, start.elapsed());
    r
}

unsafe fn trace_waits(module: *mut c_void) {
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }
    let k: Vec<u16> = "kernelbase.dll".encode_utf16().chain([0]).collect();
    let kb = unsafe { GetModuleHandleW(k.as_ptr()) };
    let find = |n: &[u8]| unsafe { GetProcAddress(kb, n.as_ptr()) } as usize;
    REAL_SLEEP.store(find(b"Sleep\0"), Ordering::Release);
    REAL_WAIT.store(find(b"WaitForSingleObject\0"), Ordering::Release);
    REAL_SLEEP_CV.store(find(b"SleepConditionVariableCS\0"), Ordering::Release);
    if REAL_SLEEP.load(Ordering::Relaxed) == 0 || REAL_WAIT.load(Ordering::Relaxed) == 0 || REAL_SLEEP_CV.load(Ordering::Relaxed) == 0 {
        log("bink: wait functions not found in kernelbase; Bink's waits not traced");
        return;
    }
    let n = unsafe { crate::loader::patch_module_imports(module.cast(), |d| d.starts_with("api-ms-win-core-synch") || d == "kernel32.dll", &[
        ("Sleep", bink_sleep as *const () as usize),
        ("WaitForSingleObject", bink_wait as *const () as usize),
        ("SleepConditionVariableCS", bink_sleep_cv as *const () as usize),
    ], &[]) };
    unsafe extern "system" {
        fn QueryPerformanceFrequency(f: *mut i64) -> i32;
    }
    let mut qpf = 0i64;
    unsafe { QueryPerformanceFrequency(&mut qpf) };
    log(&format!("bink: {n} of Bink2Scarlett's wait imports traced; QPC frequency {qpf} Hz"));
}
