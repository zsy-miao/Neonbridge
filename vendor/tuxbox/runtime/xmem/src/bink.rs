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
//!
//! test56: the 4K intro played 14 of its 30 frames/s, held back by the
//! title's frame loop (BinkDoFrameAsyncWait took 3-5 ms), so its audio came
//! at half rate. After the title's BinkNextFrame, a movie behind its clock
//! decodes and advances extra frames (the title's last BinkDoFrameAsyncMulti
//! arguments replayed and waited on, else BinkDoFrame), at most
//! TUXBOX_BINK_CATCHUP (default 2) per call, so audio keeps the movie's rate
//! while fewer frames are shown. TUXBOX_BINK_CATCHUP=0 decodes none.
//!
//! The play log at b7f0f56 still ran 27.0 of 30 frames/s, with 1 s audio
//! gaps: catching up only whole frames behind, two at a time, after
//! BinkNextFrame left the movie up to a frame behind its clock all the time.
//! Now it catches up as soon as it is any fraction behind, to TUXBOX_BINK_LEAD
//! frames (default 1) ahead of the clock, at most TUXBOX_BINK_CATCHUP (now 4)
//! per call. The catch-up also runs when the title asks BinkWait, unless the
//! title's own asynchronous decode is in flight. The 10 s line logs the
//! smallest lead over the clock.
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
const DO_FRAME: usize = 1;
const NEXT_FRAME: usize = 2;
const WAIT: usize = 3;
const ASYNC_MULTI: usize = 5;
const ASYNC_WAIT: usize = 6;
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
    caught_up: u32,
}

static WINDOW: Mutex<Option<Window>> = Mutex::new(None);

pub(crate) fn module_of(address: u64) -> String {
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
    let window = guard.get_or_insert_with(|| Window { started: now, calls: [Call::default(); 9], movie: 0, first_frame: None, last_frame: 0, caught_up: 0 });
    window.caught_up += std::mem::take(&mut *CAUGHT_UP.lock().unwrap_or_else(|e| e.into_inner()));
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
        (Some(h), Some(first)) => format!("; movie {}x{} frame {} of {}, {:.1} frames/s ({} extra decoded to catch up, TUXBOX_BINK_CATCHUP; smallest lead over the clock {}, TUXBOX_BINK_LEAD) against {:.2} nominal",
            h[0], h[1], h[3], h[2], f64::from(window.last_frame.wrapping_sub(first)) / seconds, window.caught_up,
            MIN_LEAD.lock().unwrap_or_else(|e| e.into_inner()).take().map_or("-".into(), |ms| format!("{ms:.0} ms")),
            if h[6] == 0 { 0.0 } else { f64::from(h[5]) / f64::from(h[6]) }),
        _ => String::new(),
    };
    log(&format!("bink: last {seconds:.1} s: {}{movie_line}; Bink's own waits: {}", if parts.is_empty() { "no calls".into() } else { parts.join(", ") }, take_waits(seconds)));
    *window = Window { started: now, calls: [Call::default(); 9], movie: window.movie, first_frame: None, last_frame: window.last_frame, caught_up: 0 };
}

/// Extra frames decoded since the last record.
static CAUGHT_UP: Mutex<u32> = Mutex::new(0);

struct Clock {
    movie: u64,
    started: Instant,
    first_frame: u32,
    /// The title's last BinkDoFrameAsyncMulti arguments after the handle.
    multi: Option<(u64, u64, u64)>,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);

/// Default 0, as every reference plays these intros: VIctory-fxceless and
/// VIctory have no Bink hooks at all (fxceless files.rs only routes Bink's
/// CreateFile2), rebox bink_trace.c and xcl hle/bink.rs pass the title's
/// calls through (xcl's only change is its opt-in XCL_SKIP_BINK skip). The
/// title's own BinkWait/BinkShouldSkip pace a slow title. Our extra decodes
/// ran between the title's decode and its use of the planes (test146: 166
/// catch-up frames, garbled intros 2 and 3). TUXBOX_BINK_CATCHUP=4 restores
/// the old catch-up.
fn catch_up_cap() -> u32 {
    use std::sync::OnceLock;
    static CAP: OnceLock<u32> = OnceLock::new();
    *CAP.get_or_init(|| std::env::var("TUXBOX_BINK_CATCHUP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0))
}

/// Frames ahead of its clock a movie is decoded to: TUXBOX_BINK_LEAD (default 1).
fn lead_frames() -> f64 {
    use std::sync::OnceLock;
    static LEAD: OnceLock<f64> = OnceLock::new();
    *LEAD.get_or_init(|| std::env::var("TUXBOX_BINK_LEAD").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(1.0))
}

/// The title's BinkDoFrameAsyncMulti not yet completed by its BinkDoFrameAsyncWait.
static IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Synchronous decodes running (BinkDoFrame, the catch-up's decodes).
static SYNC_DECODES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Decodes completed so far.
static DECODED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// test112: the intro showed Bink macroblock residue. d3d12 reads the video
/// planes back from title memory each submission, unordered against Bink's
/// decode, so a plane could be read half-decoded. d3d12 asks this before
/// reading a plane: the decodes in flight (0: none), and the completed count.
#[unsafe(export_name = "TuxBoxBinkState")]
pub unsafe extern "system" fn tuxbox_bink_state(decoded: *mut u64) -> u32 {
    if !decoded.is_null() {
        unsafe { *decoded = DECODED.load(Ordering::Acquire) };
    }
    SYNC_DECODES.load(Ordering::Acquire) + u32::from(IN_FLIGHT.load(Ordering::Acquire))
}

/// The first calls of the decode cycle, in order, with frame and thread
/// (TUXBOX_BINK_CALL_TRACE, default 48; 0 none).
fn trace_call(index: usize, movie: u64, phase: &str) {
    static LEFT: std::sync::OnceLock<AtomicUsize> = std::sync::OnceLock::new();
    let left = LEFT.get_or_init(|| AtomicUsize::new(std::env::var("TUXBOX_BINK_CALL_TRACE").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(48)));
    if left.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_err() {
        return;
    }
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    let frame = header(movie).map_or(String::from("-"), |h| h[3].to_string());
    log(&format!("bink call: {} {phase} (frame {frame}, thread {}, decodes done {}, in flight {})",
        NAMES[index], unsafe { GetCurrentThreadId() }, DECODED.load(Ordering::Relaxed),
        SYNC_DECODES.load(Ordering::Relaxed) + u32::from(IN_FLIGHT.load(Ordering::Relaxed))));
}

/// The smallest lead of the movie over its clock since the last record, in ms
/// (negative: behind), measured after each catch-up.
static MIN_LEAD: Mutex<Option<f64>> = Mutex::new(None);

fn note_multi(movie: u64, b: u64, c: u64, d: u64) {
    let mut clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(clock) = clock.as_mut().filter(|c| c.movie == movie) {
        clock.multi = Some((b, c, d));
    }
}

/// After the title's BinkNextFrame: decode and advance the frames the movie
/// is behind its clock, at most catch_up_cap().
unsafe fn catch_up(movie: u64) {
    let cap = catch_up_cap();
    let Some(h) = header(movie).filter(|_| cap != 0) else { return };
    let (frames, frame, rate) = (h[2], h[3], if h[6] == 0 { 0.0 } else { f64::from(h[5]) / f64::from(h[6]) });
    if !(1.0..=240.0).contains(&rate) {
        return;
    }
    let (behind, multi) = {
        let mut guard = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let restart = guard.as_ref().is_none_or(|c| c.movie != movie || frame < c.first_frame);
        if restart {
            let multi = guard.as_ref().filter(|c| c.movie == movie).and_then(|c| c.multi);
            *guard = Some(Clock { movie, started: now, first_frame: frame, multi });
            return;
        }
        let clock = guard.as_mut().unwrap();
        let target = f64::from(clock.first_frame) + now.duration_since(clock.started).as_secs_f64() * rate;
        let behind = target - f64::from(frame);
        // A stall over 2 s (loading, pause) restarts the clock instead of racing.
        if behind > rate * 2.0 {
            clock.started = now;
            clock.first_frame = frame;
            return;
        }
        // Any fraction behind counts: decode up to the lead past the clock.
        let wanted = behind + lead_frames();
        (if behind > 0.0 { wanted.ceil().max(0.0) as u32 } else { 0 }, clock.multi)
    };
    let real = |i: usize| -> F4 { unsafe { std::mem::transmute(REAL[i].load(Ordering::Acquire)) } };
    let mut done = 0;
    for _ in 0..behind.min(cap) {
        if header(movie).is_none_or(|h| h[3] >= frames) {
            break;
        }
        match multi {
            Some((b, c, d)) if REAL[ASYNC_MULTI].load(Ordering::Relaxed) != 0 && REAL[ASYNC_WAIT].load(Ordering::Relaxed) != 0 => unsafe {
                SYNC_DECODES.fetch_add(1, Ordering::AcqRel);
                real(ASYNC_MULTI)(movie, b, c, d);
                real(ASYNC_WAIT)(movie, u64::from(u32::MAX), 0, 0);
                DECODED.fetch_add(1, Ordering::AcqRel);
                SYNC_DECODES.fetch_sub(1, Ordering::AcqRel);
            },
            _ => unsafe {
                SYNC_DECODES.fetch_add(1, Ordering::AcqRel);
                real(DO_FRAME)(movie, 0, 0, 0);
                DECODED.fetch_add(1, Ordering::AcqRel);
                SYNC_DECODES.fetch_sub(1, Ordering::AcqRel);
            },
        }
        unsafe { real(NEXT_FRAME)(movie, 0, 0, 0) };
        done += 1;
    }
    if done != 0 {
        *CAUGHT_UP.lock().unwrap_or_else(|e| e.into_inner()) += done;
    }
    if let Some(h) = header(movie)
        && let Some(clock) = CLOCK.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|c| c.movie == movie)
    {
        let target = f64::from(clock.first_frame) + clock.started.elapsed().as_secs_f64() * rate;
        let lead_ms = (f64::from(h[3]) - target) / rate * 1000.0;
        let mut min = MIN_LEAD.lock().unwrap_or_else(|e| e.into_inner());
        *min = Some(min.map_or(lead_ms, |m| m.min(lead_ms)));
    }
}

unsafe fn traced(index: usize, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let real: F4 = unsafe { std::mem::transmute(REAL[index].load(Ordering::Acquire)) };
    // The decode is in flight from before Bink starts it until it completes.
    match index {
        DO_FRAME => { SYNC_DECODES.fetch_add(1, Ordering::AcqRel); }
        ASYNC_MULTI => IN_FLIGHT.store(true, Ordering::Release),
        _ => {}
    }
    if matches!(index, DO_FRAME | NEXT_FRAME | ASYNC_MULTI | ASYNC_WAIT) {
        trace_call(index, a, "start");
    }
    let start = Instant::now();
    let result = unsafe { real(a, b, c, d) };
    let us = start.elapsed().as_micros() as u64;
    match index {
        DO_FRAME => {
            DECODED.fetch_add(1, Ordering::AcqRel);
            SYNC_DECODES.fetch_sub(1, Ordering::AcqRel);
        }
        ASYNC_WAIT if result as u32 != 0 && IN_FLIGHT.swap(false, Ordering::AcqRel) => {
            DECODED.fetch_add(1, Ordering::AcqRel);
        }
        _ => {}
    }
    if matches!(index, DO_FRAME | ASYNC_WAIT) {
        trace_call(index, a, if index == ASYNC_WAIT && result as u32 == 0 { "returned 0 (not done)" } else { "done" });
    }
    match index {
        SOUND_SYSTEM => log(&format!("bink: BinkSetSoundSystem(open {}, param {b:#x}) = {result:#x}", module_of(a))),
        OPEN => log(&format!("bink: BinkOpenWithOptions = {result:#x} in {:.1} ms{}", us as f64 / 1e3,
            header(result).map_or(String::new(), |h| format!(", {}x{} {} frames at {}/{}", h[0], h[1], h[2], h[5], h[6])))),
        8 => log(&format!("bink: BinkSetSoundTrack({a}, {b:#x}) = {result:#x}")),
        _ => record(index, a, result, us),
    }
    match index {
        ASYNC_MULTI => note_multi(a, b, c, d),
        NEXT_FRAME => unsafe { catch_up(a) },
        WAIT if !IN_FLIGHT.load(Ordering::Relaxed) && std::env::var_os("TUXBOX_BINK_WAIT_CATCHUP").is_none_or(|v| v != "0") => unsafe { catch_up(a) },
        _ => {}
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
