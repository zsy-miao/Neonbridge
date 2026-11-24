//! XMA context probe (TUXBOX_AUDIO_PROBE=0 off). Diagnostics only.
//!
//! The title drives the console XMA decoder by writing the 64-byte contexts in
//! the SHAPE XMA array and calling IAcpHal methods. This module counts the
//! IAcpHal calls per slot and polls the contexts every 5 ms for changes.
//! SHAPE context fields as (word, shift, bits), from the shared audio notes
//! (facts only); raw words are logged as well:
//!   (0,20,2) input valid, (0,22,5) output ring blocks, (0,27,5) output write block
//!   (1,27,2) rate index, (1,29,1) stereo, (1,31,1) output valid
//!   (2,0,26) input read offset in bits, (2,26,5)/(2,31,1) error code/flag
//!   words 5/6 input buffer 0/1 address, word 7 output ring, (9,0,5) output read block
use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const WORDS: usize = 16;
const SLOTS: usize = super::HAL_VTABLE_SLOTS;

pub(crate) fn enabled() -> bool {
    std::env::var_os("TUXBOX_AUDIO_PROBE").is_none_or(|v| v != "0")
}

pub(crate) fn log(text: std::fmt::Arguments<'_>) {
    let line = format!("TuxBox acphal: {text}");
    if std::env::var_os("TUXBOX_LOG").is_some_and(|v| v == "stderr") {
        eprintln!("{line}");
    }
    let message = format!("{line}\n\0");
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(message.as_ptr()) };
}

// ---- IAcpHal call counts ----

static CALLS: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
static FIRST_ARGS: Mutex<Vec<(usize, [usize; 3])>> = Mutex::new(Vec::new());

pub(crate) unsafe extern "system" fn method<const N: usize>(_this: *mut c_void, a: usize, b: usize, c: usize) -> i32 {
    if enabled() && CALLS[N].fetch_add(1, Ordering::Relaxed) < 3 {
        let mut first = FIRST_ARGS.lock().unwrap_or_else(|e| e.into_inner());
        if first.len() < 96 {
            first.push((N, [a, b, c]));
        }
        drop(first);
        log(format_args!("IAcpHal slot {N} called ({a:#x}, {b:#x}, {c:#x})"));
    }
    start_watch();
    0
}

// ---- XMA context watch ----

static ARRAY: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn contexts_allocated(array: usize, count: usize) {
    if !enabled() {
        return;
    }
    ARRAY.store(array, Ordering::Relaxed);
    COUNT.store(count, Ordering::Relaxed);
    log(format_args!("SHAPE XMA array: {count} contexts at {array:#x}"));
    start_watch();
}

fn start_watch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !enabled() || ARRAY.load(Ordering::Relaxed) == 0 {
        return;
    }
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("TuxBox XMA probe".into()).spawn(watch);
    });
}

#[derive(Default)]
struct Window {
    enables: u32,
    disables: u32,
    output_valid_on: u32,
    read_moves: u32,
    write_moves: u32,
    out_read_moves: u32,
    errors: u32,
    changed_words: [u32; WORDS],
    active: std::collections::BTreeSet<usize>,
}

fn watch() {
    let array = ARRAY.load(Ordering::Relaxed);
    let count = COUNT.load(Ordering::Relaxed).min(4096);
    let mut previous = vec![[0u32; WORDS]; count];
    let mut dumped = 0;
    let mut window = Window::default();
    let mut started = Instant::now();
    let mut ever_active = 0usize;
    loop {
        std::thread::sleep(Duration::from_millis(5));
        for (i, old) in previous.iter_mut().enumerate() {
            let base = (array + i * super::SHAPE_XMA_CONTEXT_SIZE) as *const u32;
            let mut now = [0u32; WORDS];
            for (w, slot) in now.iter_mut().enumerate() {
                // SAFETY: the array lives in the APU heap for the process lifetime.
                *slot = unsafe { base.add(w).read_volatile() };
            }
            if now == *old {
                continue;
            }
            if *old == [0; WORDS] && dumped < 4 {
                dumped += 1;
                log(format_args!("XMA context {i} first written: {:08x?}", now));
            }
            let bit = |v: u32, b: u32| (v >> b) & 1;
            for b in [20, 21] {
                match (bit(old[0], b), bit(now[0], b)) {
                    (0, 1) => window.enables += 1,
                    (1, 0) => window.disables += 1,
                    _ => {}
                }
            }
            window.output_valid_on += u32::from(bit(old[1], 31) == 0 && bit(now[1], 31) == 1);
            window.read_moves += u32::from(old[2] & 0x3ff_ffff != now[2] & 0x3ff_ffff);
            window.write_moves += u32::from(old[0] >> 27 != now[0] >> 27);
            window.out_read_moves += u32::from(old[9] & 0x1f != now[9] & 0x1f);
            window.errors += u32::from(bit(old[2], 31) == 0 && bit(now[2], 31) == 1);
            for w in 0..WORDS {
                window.changed_words[w] += u32::from(old[w] != now[w]);
            }
            if window.active.len() < 4096 {
                window.active.insert(i);
            }
            *old = now;
        }
        let seconds = started.elapsed().as_secs_f64();
        if seconds >= 10.0 {
            ever_active = ever_active.max(previous.iter().filter(|c| **c != [0; WORDS]).count());
            let calls: Vec<String> = CALLS.iter().enumerate()
                .filter_map(|(n, c)| { let c = c.load(Ordering::Relaxed); (c > 0).then(|| format!("{n}:{c}")) })
                .collect();
            let sample = window.active.iter().next().map(|&i| {
                let c = &previous[i];
                const RATES: [u32; 4] = [24000, 32000, 44100, 48000];
                format!("ctx {i}: in valid {:02b}, {} Hz {} ch, ring {} blocks write {} read {} (out valid {}), input read bit {:#x}, error {}/{}, in0 {:#x} in1 {:#x} ring {:#x}",
                    (c[0] >> 20) & 3, RATES[((c[1] >> 27) & 3) as usize], 1 + ((c[1] >> 29) & 1), (c[0] >> 22) & 0x1f,
                    c[0] >> 27, c[9] & 0x1f, c[1] >> 31, c[2] & 0x3ff_ffff, c[2] >> 31, (c[2] >> 26) & 0x1f, c[5], c[6], c[7])
            }).unwrap_or_else(|| "none".into());
            log(format_args!(
                "XMA last {seconds:.1} s: {} of {count} contexts changed ({ever_active} non-zero), input valid set {} / cleared {}, output valid set {}, input read moved {}, output write moved {}, output read moved {}, errors raised {}; dword changes {:?}; {sample}; IAcpHal calls {}",
                window.active.len(), window.enables, window.disables, window.output_valid_on, window.read_moves,
                window.write_moves, window.out_read_moves, window.errors, window.changed_words,
                if calls.is_empty() { "none".into() } else { calls.join(" ") }));
            window = Window::default();
            started = Instant::now();
        }
    }
}
