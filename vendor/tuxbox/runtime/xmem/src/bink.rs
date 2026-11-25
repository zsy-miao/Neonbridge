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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

/// Bink buffer watch (test165: Bink2Scarlett+0x624a, the bitstream reader's
/// `mov r8d,[r15]`, faulted reading 0x5d0000000 after r15 passed its own
/// buffer bounds, so a page inside the buffer Bink was given was gone).
/// At each open this logs every allocation the HBINK points into (base,
/// size, state, type, protect) and the watched page with the newest xmem
/// unmap over it; each decode logs the watched page again when its state
/// changes. TUXBOX_BINK_BUF_WATCH=<hex> picks the page (default
/// 0x5d0000000), =0 off.
mod buf_watch {
    use super::log;
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};

    fn page() -> usize {
        static PAGE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *PAGE.get_or_init(|| match std::env::var("TUXBOX_BINK_BUF_WATCH") {
            Ok(v) => usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).unwrap_or(0x5d0000000),
            Err(_) => 0x5d0000000,
        } & !0xfff)
    }

    fn query(at: usize) -> Option<MEMORY_BASIC_INFORMATION> {
        let mut m: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        (unsafe { VirtualQuery(at as *const _, &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0).then_some(m)
    }

    fn describe(m: &MEMORY_BASIC_INFORMATION) -> String {
        format!("region {:#x}+{:#x} base {:#x} state {:#x} type {:#x} protect {:#x}",
            m.BaseAddress as usize, m.RegionSize, m.AllocationBase as usize, m.State, m.Type, m.Protect)
    }

    fn unmapped_over(at: usize) -> String {
        let mut out = [0u64; 4];
        if unsafe { crate::unmap_trace::newest(at, 0x1000, &mut out) } {
            format!("; newest xmem unmap over it: site {} {:#x}+{:#x} {} ms ago", out[0], out[1], out[2], out[3])
        } else {
            "; no xmem unmap recorded over it".into()
        }
    }

    fn watched(tag: &str) -> String {
        let p = page();
        match query(p) {
            Some(m) => format!("bink: buffer watch {tag}: page {p:#x} {}{}", describe(&m), unmapped_over(p)),
            None => format!("bink: buffer watch {tag}: page {p:#x} query failed"),
        }
    }

    pub(super) fn at_open(movie: u64) {
        if page() == 0 || movie < 0x10000 {
            return;
        }
        // The HBINK's own pointers name the buffers Bink reads from.
        let mut words = [0u64; 0x100];
        let mut read = 0usize;
        unsafe extern "system" {
            fn ReadProcessMemory(process: isize, base: *const core::ffi::c_void, buffer: *mut core::ffi::c_void, size: usize, read: *mut usize) -> i32;
        }
        unsafe { ReadProcessMemory(-1, movie as *const _, words.as_mut_ptr().cast(), size_of_val(&words), &mut read) };
        let mut seen: Vec<usize> = Vec::new();
        let mut lines = Vec::new();
        for (i, &w) in words[..read / 8].iter().enumerate() {
            if w < 0x1_0000_0000 || w >= 0x8000_0000_0000 {
                continue;
            }
            let Some(m) = query(w as usize) else { continue };
            if m.State == 0x10000 || seen.contains(&(m.AllocationBase as usize)) {
                continue;
            }
            seen.push(m.AllocationBase as usize);
            lines.push(format!("+{:#x} -> {w:#x} {}", i * 8, describe(&m)));
            if lines.len() == 12 {
                break;
            }
        }
        log(&format!("bink: buffer watch open {movie:#x}: HBINK points into {} allocation(s): {} (TUXBOX_BINK_BUF_WATCH=0 off)", lines.len(), lines.join("; ")));
        log(&watched("open"));
    }

    pub(super) fn at_decode() {
        if page() == 0 {
            return;
        }
        static LAST: AtomicU64 = AtomicU64::new(u64::MAX);
        let key = query(page()).map_or(0, |m| (m.State as u64) << 32 | (m.Protect as u64) << 8 | m.Type as u64 >> 16);
        if LAST.swap(key, Ordering::Relaxed) != key {
            log(&watched("decode (state changed)"));
        }
    }
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
        (Some(h), Some(first)) => format!("; movie {}x{} frame {} of {}, {:.1} frames/s ({} extra decoded to catch up, {:.1}/s, {:.1} ms/s on the title's thread, TUXBOX_BINK_CATCHUP; smallest lead over the clock {}, TUXBOX_BINK_LEAD) against {:.2} nominal",
            h[0], h[1], h[3], h[2], f64::from(window.last_frame.wrapping_sub(first)) / seconds, window.caught_up, f64::from(window.caught_up) / seconds,
            CATCH_UP_US.swap(0, Ordering::Relaxed) as f64 / 1e3 / seconds,
            MIN_LEAD.lock().unwrap_or_else(|e| e.into_inner()).take().map_or("-".into(), |ms| format!("{ms:.0} ms")),
            if h[6] == 0 { 0.0 } else { f64::from(h[5]) / f64::from(h[6]) }),
        _ => String::new(),
    };
    log(&format!("bink: last {seconds:.1} s: {}{movie_line}; Bink's own waits: {}", if parts.is_empty() { "no calls".into() } else { parts.join(", ") }, take_waits(seconds)));
    *window = Window { started: now, calls: [Call::default(); 9], movie: window.movie, first_frame: None, last_frame: window.last_frame, caught_up: 0 };
}

/// Extra frames decoded since the last record.
static CAUGHT_UP: Mutex<u32> = Mutex::new(0);
/// Microseconds the title's thread spent in catch-up decodes since the last
/// 10 s line (test155: 23 extra 4K decodes/s on the thread whose movie loop
/// ran 7/s; how much of its frame they take was not logged).
static CATCH_UP_US: AtomicU64 = AtomicU64::new(0);

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
/// the old catch-up. Now default 4 again, at the before-decode site that
/// fixed the garble: a movie keeps its real-time rate (and Bink's soundtrack,
/// fed per decoded frame, its rate: test152 played the 4K intro at 12 of 30
/// frames/s, 3 s audio gaps) whatever the title's fps. TUXBOX_BINK_CATCHUP=0 off.
/// Where the catch-up decodes. After the title's BinkNextFrame (the
/// old place) its extra decodes land in the registered plane ring while the
/// frame just decoded has not yet been submitted, so d3d12's read of that
/// frame's planes (at submission, from title memory) took a later frame's
/// slot contents: the garble. Just before the title's own next decode
/// (BinkDoFrame / BinkDoFrameAsyncMulti), the previous frame's submission is
/// behind us and the title is about to reuse the ring itself; the extra
/// frames decode and advance in order there, and the frame the title shows
/// is its own decode, made after them. TUXBOX_BINK_CATCHUP_AT=next restores
/// the old place.
fn catch_up_before_decode() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| catch_up_cap() != 0 && std::env::var("TUXBOX_BINK_CATCHUP_AT").map_or(true, |v| v.trim() != "next"))
}

fn catch_up_cap() -> u32 {
    use std::sync::OnceLock;
    static CAP: OnceLock<u32> = OnceLock::new();
    *CAP.get_or_init(|| std::env::var("TUXBOX_BINK_CATCHUP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(4))
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

/// The highest completed-decode count (TuxBoxBinkState's) d3d12 has read the
/// video planes after: d3d12 calls this once it has read a plane texture
/// while Bink was idle, passing the count it saw then.
static PLANES_READ: AtomicU64 = AtomicU64::new(0);
/// Whether d3d12 has ever reported a plane read (an older d3d12 never does).
static PLANES_REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[unsafe(export_name = "TuxBoxBinkPlanesRead")]
pub extern "system" fn tuxbox_bink_planes_read(decoded: u64) {
    PLANES_REPORTED.store(true, Ordering::Release);
    PLANES_READ.fetch_max(decoded, Ordering::AcqRel);
}

/// Catch-up's fence: wait until d3d12 has read the planes of the latest
/// decode (the frame the title shows), so the extra decodes cannot overwrite
/// a ring slot before it is read. TUXBOX_BINK_PLANE_FENCE_MS (default 8, short since the read may come from the same thread after this returns; 0
/// off) bounds the wait; a timeout skips this call's catch-up.
fn planes_fenced() -> bool {
    use std::sync::OnceLock;
    static MS: OnceLock<u64> = OnceLock::new();
    let ms = *MS.get_or_init(|| std::env::var("TUXBOX_BINK_PLANE_FENCE_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(8));
    if ms == 0 || !PLANES_REPORTED.load(Ordering::Acquire) {
        return true;
    }
    let want = DECODED.load(Ordering::Acquire);
    let until = Instant::now() + std::time::Duration::from_millis(ms);
    while PLANES_READ.load(Ordering::Acquire) < want {
        if Instant::now() >= until {
            FENCE_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
    true
}
static FENCE_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

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
/// Frames the movie is behind its clock (fractional), starting or restarting
/// the clock as needed; None while it (re)starts.
fn clock_behind(movie: u64, frame: u32, rate: f64) -> Option<(f64, Option<(u64, u64, u64)>)> {
    let mut guard = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let restart = guard.as_ref().is_none_or(|c| c.movie != movie || frame < c.first_frame);
    if restart {
        let multi = guard.as_ref().filter(|c| c.movie == movie).and_then(|c| c.multi);
        *guard = Some(Clock { movie, started: now, first_frame: frame, multi });
        return None;
    }
    let clock = guard.as_mut().unwrap();
    let target = f64::from(clock.first_frame) + now.duration_since(clock.started).as_secs_f64() * rate;
    let behind = target - f64::from(frame);
    if behind > rate * 2.0 {
        clock.started = now;
        clock.first_frame = frame;
        return None;
    }
    Some((behind, clock.multi))
}

fn movie_rate(h: &[u32; 7]) -> Option<f64> {
    let rate = if h[6] == 0 { 0.0 } else { f64::from(h[5]) / f64::from(h[6]) };
    (1.0..=240.0).contains(&rate).then_some(rate)
}

/// TUXBOX_BINK_SKIP_SIGNAL (default on): BinkShouldSkip says skip while the
/// movie is a whole frame or more behind its clock. The title's own loop then
/// decodes and advances the next frame in order, before showing one, with
/// the frame buffers under its own GPU sync, which is what Bink's skip
/// contract is for (RAD's loop: DoFrame, while ShouldSkip { NextFrame;
/// DoFrame }, NextFrame). Our catch-up decoded the same frames from outside
/// the title's loop into the registered plane ring while the title's copy of
/// the shown frame was still reading it: the garble. Audio is fed from the
/// decoded frames, so it keeps the clock. TUXBOX_BINK_SKIP_SIGNAL=0 passes
/// Bink's own answer through.
fn skip_signal() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_BINK_SKIP_SIGNAL").map_or(true, |v| v.trim() != "0"))
}

static SKIPS_SIGNALLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn should_skip(movie: u64, real: u64) -> u64 {
    if real != 0 || !skip_signal() || catch_up_cap() != 0 {
        return real;
    }
    let Some(h) = header(movie) else { return real };
    let Some(rate) = movie_rate(&h) else { return real };
    if h[3] >= h[2] {
        return real;
    }
    match clock_behind(movie, h[3], rate) {
        Some((behind, _)) if behind >= 1.0 => {
            SKIPS_SIGNALLED.fetch_add(1, Ordering::Relaxed);
            1
        }
        _ => real,
    }
}

unsafe fn catch_up(movie: u64) {
    let cap = catch_up_cap();
    let Some(h) = header(movie) else { return };
    let (frames, frame) = (h[2], h[3]);
    let Some(rate) = movie_rate(&h) else { return };
    if cap == 0 {
        // Keeps the clock (and the smallest-lead log) running for should_skip.
        let _ = clock_behind(movie, frame, rate);
        return;
    }
    let (behind, multi) = {
        let Some((behind, multi)) = clock_behind(movie, frame, rate) else { return };
        // Any fraction behind counts: decode up to the lead past the clock.
        let wanted = behind + lead_frames();
        (if behind > 0.0 { wanted.ceil().max(0.0) as u32 } else { 0 }, multi)
    };
    let real = |i: usize| -> F4 { unsafe { std::mem::transmute(REAL[i].load(Ordering::Acquire)) } };
    let mut done = 0;
    let began = Instant::now();
    for _ in 0..behind.min(cap) {
        if header(movie).is_none_or(|h| h[3] >= frames) || !planes_fenced() {
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
    CATCH_UP_US.fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
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
    // Catch-up before the title's own decode: see catch_up_before_decode.
    if matches!(index, DO_FRAME | ASYNC_MULTI) && catch_up_before_decode() && !IN_FLIGHT.load(Ordering::Relaxed) {
        unsafe { catch_up(a) };
    }
    // The decode is in flight from before Bink starts it until it completes.
    match index {
        DO_FRAME => { SYNC_DECODES.fetch_add(1, Ordering::AcqRel); }
        ASYNC_MULTI => IN_FLIGHT.store(true, Ordering::Release),
        _ => {}
    }
    if matches!(index, DO_FRAME | NEXT_FRAME | ASYNC_MULTI | ASYNC_WAIT) {
        trace_call(index, a, "start");
    }
    if matches!(index, DO_FRAME | ASYNC_MULTI) {
        buf_watch::at_decode();
    }
    let start = Instant::now();
    let given = if index == SOUND_SYSTEM { sound::wrap_provider(a) } else { a };
    let mut result = unsafe { real(given, b, c, d) };
    if index == SHOULD_SKIP {
        result = should_skip(a, result);
    }
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
    if index == OPEN {
        buf_watch::at_open(result);
    }
    match index {
        ASYNC_MULTI => note_multi(a, b, c, d),
        NEXT_FRAME if !catch_up_before_decode() => unsafe { catch_up(a) },
        WAIT if !catch_up_before_decode() && !IN_FLIGHT.load(Ordering::Relaxed) && std::env::var_os("TUXBOX_BINK_WAIT_CATCHUP").is_none_or(|v| v != "0") => unsafe { catch_up(a) },
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

/// Bink sound counters: every BINKSND callback the title's open fills
/// (+0x170..+0x1b0: +0x178 Lock-like, 0 when no buffer is free; +0x180
/// Unlock-like, adds the bytes to [+0x8c]) is wrapped after the open, the
/// first call of each is logged, and every 10 s each slot's calls/s and
/// zero results plus the bytes committed, so a silent movie shows which
/// callback Bink never reaches. TUXBOX_BINK_SOUND_STATS=0 off.
mod sound {
    use super::*;

    const SLOTS: usize = 9;
    const FIRST: u64 = 0x170;
    const UNLOCK: usize = 2;
    static PROVIDER: AtomicUsize = AtomicUsize::new(0);
    static OPEN: AtomicUsize = AtomicUsize::new(0);
    static ORIG: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
    static CALLS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    static ZERO: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    static SEEN: [std::sync::atomic::AtomicBool; SLOTS] = [const { std::sync::atomic::AtomicBool::new(false) }; SLOTS];
    static BYTES: AtomicU64 = AtomicU64::new(0);
    static MAX_GAP_US: AtomicU64 = AtomicU64::new(0);
    static STATE: Mutex<Option<(Instant, Option<Instant>)>> = Mutex::new(None);

    fn on() -> bool {
        std::env::var_os("TUXBOX_BINK_SOUND_STATS").is_none_or(|v| v != "0")
    }

    pub(super) fn wrap_provider(provider: u64) -> u64 {
        if !on() || provider == 0 {
            return provider;
        }
        PROVIDER.store(provider as usize, Ordering::Release);
        provider_wrapper as F4 as usize as u64
    }

    unsafe extern "system" fn provider_wrapper(a: u64, b: u64, c: u64, d: u64) -> u64 {
        let real: F4 = unsafe { std::mem::transmute(PROVIDER.load(Ordering::Acquire)) };
        let open = unsafe { real(a, b, c, d) };
        if open == 0 {
            return 0;
        }
        OPEN.store(open as usize, Ordering::Release);
        open_wrapper as F4 as usize as u64
    }

    unsafe extern "system" fn open_wrapper(snd: u64, b: u64, c: u64, d: u64) -> u64 {
        let real: F4 = unsafe { std::mem::transmute(OPEN.load(Ordering::Acquire)) };
        let result = unsafe { real(snd, b, c, d) };
        if snd != 0 {
            let mut names = Vec::new();
            // test165 crashed in Bink after the first +0x1a0 call: only
            // Ready/Lock/Unlock (+0x170..+0x180, integer arguments, test164's
            // Lock/Unlock wrap ran clean) are wrapped; the others may take
            // float or stack arguments a 4-integer wrapper does not carry.
            for (i, wrapper) in WRAPPERS.iter().enumerate().take(3) {
                let slot = (snd + FIRST + 8 * i as u64) as *mut usize;
                let old = unsafe { slot.read_unaligned() };
                let wrapper = *wrapper as usize;
                if old != 0 && old != wrapper {
                    ORIG[i].store(old, Ordering::Release);
                    unsafe { slot.write_unaligned(wrapper) };
                }
                names.push(format!("+{:#x} {}", FIRST + 8 * i as u64, module_of(ORIG[i].load(Ordering::Relaxed) as u64)));
            }
            if result == 0 {
                // The open's buffer comes from the allocator at
                // game+0x9dc62d8 (thunk game+0x95eb60 at +0x18):
                // [+0x98] * 16 bytes, 16-aligned, kept at [+0xa4].
                let game = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(core::ptr::null()) } as u64;
                let words: Vec<String> = (0..4).map(|i| {
                    let w = unsafe { ((game + 0x9dc_62d8 + 8 * i) as *const u64).read_unaligned() };
                    format!("{w:#x} ({})", module_of(w))
                }).collect();
                let field = |off: u64| unsafe { ((snd + off) as *const u32).read_unaligned() };
                log(&format!("bink: sound open failed: buffer {} x 16 bytes asked ([+0x98]), [+0xa4] {:#x}, channels {} rate {}; allocator game+0x9dc62d8 = {}",
                    field(0x98), unsafe { ((snd + 0xa4) as *const u64).read_unaligned() }, field(0x38), field(0x34), words.join(", ")));
            }
            log(&format!("bink: sound open {snd:#x}({b:#x}, {c:#x}) = {result:#x}; callbacks {} counted (TUXBOX_BINK_SOUND_STATS=0 off)", names.join(", ")));
        }
        result
    }

    unsafe fn call(i: usize, snd: u64, b: u64, c: u64, d: u64) -> u64 {
        let real: F4 = unsafe { std::mem::transmute(ORIG[i].load(Ordering::Acquire)) };
        let total = || unsafe { ((snd + 0x8c) as *const u32).read_unaligned() };
        let before = if i == UNLOCK && snd != 0 { total() } else { 0 };
        let result = unsafe { real(snd, b, c, d) };
        CALLS[i].fetch_add(1, Ordering::Relaxed);
        if result == 0 {
            ZERO[i].fetch_add(1, Ordering::Relaxed);
        }
        if !SEEN[i].swap(true, Ordering::Relaxed) {
            log(&format!("bink: sound callback +{:#x} first call ({snd:#x}, {b:#x}, {c:#x}, {d:#x}) = {result:#x} on thread {}",
                FIRST + 8 * i as u64, unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }));
        }
        let now = Instant::now();
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let (started, last) = state.get_or_insert((now, None));
        if i == UNLOCK && snd != 0 {
            BYTES.fetch_add(u64::from(total().wrapping_sub(before)), Ordering::Relaxed);
            if let Some(last) = *last {
                MAX_GAP_US.fetch_max(now.duration_since(last).as_micros() as u64, Ordering::Relaxed);
            }
            *last = Some(now);
        }
        let seconds = now.duration_since(*started).as_secs_f64();
        if seconds >= 10.0 {
            *started = now;
            let slots: Vec<String> = (0..SLOTS).filter_map(|i| {
                let n = CALLS[i].swap(0, Ordering::Relaxed);
                let z = ZERO[i].swap(0, Ordering::Relaxed);
                (n != 0).then(|| format!("+{:#x} {:.1}/s ({z} returned 0)", FIRST + 8 * i as u64, n as f64 / seconds))
            }).collect();
            log(&format!("bink: sound last {seconds:.1} s: {}; {:.0} bytes/s committed, longest gap between commits {:.0} ms; {}",
                slots.join(", "), BYTES.swap(0, Ordering::Relaxed) as f64 / seconds, MAX_GAP_US.swap(0, Ordering::Relaxed) as f64 / 1e3,
                unsafe { consumer(snd) }));
        }
        result
    }

    /// The title's side of the stream: Ready (+0x170, game+0x20eeb40) says
    /// free = queue [+0xc] - the head entry's size, queue = [snd+0x6c]
    /// (entries [+0x38], 16 bytes, count [+0x34]); the open registers the
    /// stream [snd+0x74] in the 8-slot table at game+0x9dc6320 (0x18 apart),
    /// which the mixer callback game+0x9efd90 looks up (0xa104b0) and drains
    /// only while the byte at game+0x9dd6ee8 is set.
    unsafe fn consumer(snd: u64) -> String {
        if snd == 0 {
            return String::new();
        }
        let game = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(core::ptr::null()) } as u64;
        let r64 = |a: u64| unsafe { (a as *const u64).read_unaligned() };
        let r32 = |a: u64| unsafe { (a as *const u32).read_unaligned() };
        let queue = r64(snd + 0x6c);
        let stream = r64(snd + 0x74);
        let q = if queue == 0 { "queue 0".to_string() } else {
            let count = r32(queue + 0x34);
            let entries = r64(queue + 0x38);
            let head = if count != 0 && entries != 0 { r32(entries + 8) } else { 0 };
            format!("queue {queue:#x} capacity {} entries {count} head {head} bytes", r32(queue + 0xc))
        };
        let table: Vec<String> = (0..8).map(|i| format!("{:#x}", r64(game + 0x9dc_6320 + 0x18 * i))).collect();
        format!("{q}; stream {stream:#x}, mixer table {}, mixer gate byte {}", table.join(" "), unsafe { *((game + 0x9dd_6ee8) as *const u8) })
    }

    macro_rules! slot {
        ($name:ident, $i:expr) => {
            unsafe extern "system" fn $name(a: u64, b: u64, c: u64, d: u64) -> u64 {
                unsafe { call($i, a, b, c, d) }
            }
        };
    }
    slot!(s0, 0);
    slot!(s1, 1);
    slot!(s2, 2);
    slot!(s3, 3);
    slot!(s4, 4);
    slot!(s5, 5);
    slot!(s6, 6);
    slot!(s7, 7);
    slot!(s8, 8);
    static WRAPPERS: [F4; SLOTS] = [s0, s1, s2, s3, s4, s5, s6, s7, s8];
}
