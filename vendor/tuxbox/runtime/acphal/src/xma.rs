//! SHAPE XMA decoding (TUXBOX_XMA=0 off: IAcpHal slot 2 stays an inert counter).
//!
//! The title drives the console XMA decoder through IAcpHal slot 2, the
//! coprocessor command submit (this, command, id, frame, data, notification).
//! Commands 5/6 enable one context / a bitmask of contexts, 7/8 disable them,
//! 18 advances a context's output read block. Enabled contexts are decoded on
//! one thread of ours: 2 KiB XMA2 packets from the current input buffer go
//! through the `xma` crate (LGPL-2.1-or-later, unmodified pinned dependency;
//! see NOTICE) and the s16 PCM fills the output ring in 256-byte blocks until
//! the write block reaches the title's read block.
//!
//! Context fields as (word, shift, bits), from the shared audio notes (facts
//! only): (0,0,12) input 0 packets, (0,12,8) loop count (255 forever),
//! (0,20,2) input valid, (0,22,5) ring blocks, (0,27,5) write block;
//! (1,0,12) input 1 packets, (1,12,2) loop-end subframes kept - 1,
//! (1,17,3) loop-start skip, (1,24,3) first skip (subframes of 128 samples),
//! (1,27,2) rate, (1,29,1) stereo, (1,31,1) output valid; (2,0,26) read bit,
//! (2,26,5) error code, (2,31,1) error flag; (3,0,26) loop start bit;
//! (4,0,26) loop end bit, (4,31,1) current input; words 5/6 input addresses,
//! word 7 ring address (APU addresses); (9,0,5) read block.
//!
//! Decoding is per packet: the read offset moves in whole packets and loop
//! points round down to their packet (the loop restarts the decoder cold).
//! Fallback: a packet that does not decode (or a decoder that cannot be made)
//! yields one frame of silence per packet, so the ring always advances and
//! the title never stalls on a voice.
use core::ffi::c_void;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use xma::{XMA_FRAME_SAMPLES, XMA_PACKET_BYTES, Xma2Stream};

const ENABLE_XMA: u32 = 5;
const ENABLE_XMAS: u32 = 6;
const DISABLE_XMA: u32 = 7;
const DISABLE_XMAS: u32 = 8;
const ADVANCE_XMA_READ: u32 = 18;

const BLOCK_BYTES: usize = 256;
const SAMPLES_PER_BLOCK: usize = BLOCK_BYTES / 2;
const SUBFRAME_SAMPLES: usize = 128;
const PACKET_BITS: u32 = (XMA_PACKET_BYTES * 8) as u32;
/// Error code: the input ran out.
const INPUT_INVALID: u32 = 4;
const RATES: [u32; 4] = [24_000, 32_000, 44_100, 48_000];
/// Packets in a row without output before silence stands in.
const EMPTY_LIMIT: u32 = 3;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("TUXBOX_XMA").is_none_or(|v| v != "0");
        crate::probe::log(format_args!("XMA decoder {}", if on { "on (TUXBOX_XMA=0 turns it off)" } else { "off (TUXBOX_XMA=0)" }));
        on
    })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Voice {
    stream: Option<Xma2Stream>,
    pcm: VecDeque<i16>,
    /// Samples per channel still to drop.
    skip: usize,
    ended: bool,
    empty: u32,
}

struct Contexts {
    array: usize,
    voices: Vec<Mutex<Option<Voice>>>,
}

static CONTEXTS: RwLock<Contexts> = RwLock::new(Contexts { array: 0, voices: Vec::new() });

fn contexts() -> std::sync::RwLockReadGuard<'static, Contexts> {
    CONTEXTS.read().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct Stats {
    enables: AtomicU64,
    disables: AtomicU64,
    advances: AtomicU64,
    packets: AtomicU64,
    bad_packets: AtomicU64,
    silent_frames: AtomicU64,
    blocks: AtomicU64,
    loops: AtomicU64,
    ended: AtomicU64,
    no_decoder: AtomicU64,
}

static STATS: Stats = Stats {
    enables: AtomicU64::new(0),
    disables: AtomicU64::new(0),
    advances: AtomicU64::new(0),
    packets: AtomicU64::new(0),
    bad_packets: AtomicU64::new(0),
    silent_frames: AtomicU64::new(0),
    blocks: AtomicU64::new(0),
    loops: AtomicU64::new(0),
    ended: AtomicU64::new(0),
    no_decoder: AtomicU64::new(0),
};

fn bump(counter: &AtomicU64) -> u64 {
    counter.fetch_add(1, Ordering::Relaxed)
}

/// Called by AcpHalAllocateShapeContexts.
pub(crate) fn contexts_allocated(array: usize, count: usize) {
    if !enabled() {
        return;
    }
    let mut c = CONTEXTS.write().unwrap_or_else(|e| e.into_inner());
    c.array = array;
    c.voices = (0..count).map(|_| Mutex::new(None)).collect();
    crate::probe::log(format_args!("XMA decoder: {count} contexts at {array:#x}"));
}

/// Called by AcpHalReleaseShapeContexts.
pub(crate) fn contexts_released() {
    let mut c = CONTEXTS.write().unwrap_or_else(|e| e.into_inner());
    c.array = 0;
    c.voices.clear();
}

#[derive(Clone, Copy)]
struct Context(*mut u32);

impl Context {
    fn get(self, word: usize) -> u32 {
        // SAFETY: inside the context array, which the title shares with us.
        unsafe { self.0.add(word).read_volatile() }
    }
    fn set(self, word: usize, value: u32) {
        // SAFETY: as above.
        unsafe { self.0.add(word).write_volatile(value) }
    }
    fn field(self, word: usize, shift: u32, bits: u32) -> u32 {
        (self.get(word) >> shift) & ((1 << bits) - 1)
    }
    fn set_field(self, word: usize, shift: u32, bits: u32, value: u32) {
        let mask = ((1u32 << bits) - 1) << shift;
        self.set(word, (self.get(word) & !mask) | ((value << shift) & mask));
    }
    fn channels(self) -> usize {
        1 + self.field(1, 29, 1) as usize
    }
}

impl Contexts {
    fn context(&self, index: u32) -> Option<Context> {
        ((index as usize) < self.voices.len() && self.array != 0).then(|| Context((self.array + index as usize * 64) as *mut u32))
    }

    fn enable(&self, index: u32) {
        let Some(context) = self.context(index) else { return };
        let mut slot = lock(&self.voices[index as usize]);
        context.set_field(2, 26, 6, 0);
        let n = bump(&STATS.enables);
        if n < 4 {
            let words: Vec<String> = (0..10).map(|w| format!("{:08x}", context.get(w))).collect();
            crate::probe::log(format_args!("XMA voice {index} enabled: {}", words.join(" ")));
        }
        *slot = Some(Voice {
            stream: None,
            pcm: VecDeque::new(),
            skip: context.field(1, 24, 3) as usize * SUBFRAME_SAMPLES,
            ended: false,
            empty: 0,
        });
    }

    fn disable(&self, index: u32) {
        if (index as usize) < self.voices.len() && lock(&self.voices[index as usize]).take().is_some() {
            bump(&STATS.disables);
        }
    }

    fn advance_read(&self, index: u32, blocks: u32) {
        let Some(context) = self.context(index) else { return };
        let _slot = lock(&self.voices[index as usize]);
        let size = context.field(0, 22, 5).max(1);
        context.set_field(9, 0, 5, (context.field(9, 0, 5) + blocks) % size);
        if blocks > 0 {
            context.set_field(1, 31, 1, 1);
        }
        bump(&STATS.advances);
    }

    fn pump(&self) {
        for index in 0..self.voices.len() as u32 {
            if let (Some(context), Some(voice)) = (self.context(index), &mut *lock(&self.voices[index as usize])) {
                pump(context, voice);
            }
        }
    }
}

fn pump(context: Context, voice: &mut Voice) {
    let size = context.field(0, 22, 5);
    if size == 0 {
        return;
    }
    while context.field(1, 31, 1) == 1 {
        if voice.pcm.len() < SAMPLES_PER_BLOCK && !decode(context, voice) {
            return;
        }
        let write = context.field(0, 27, 5);
        let Some(out) = crate::heap_host(context.get(7).wrapping_add(write * BLOCK_BYTES as u32), BLOCK_BYTES) else {
            return;
        };
        for i in 0..SAMPLES_PER_BLOCK {
            let sample = voice.pcm.pop_front().unwrap_or(0);
            // SAFETY: `out` covers BLOCK_BYTES of the live APU heap.
            unsafe { (out as *mut u8).add(i * 2).cast::<[u8; 2]>().write_unaligned(sample.to_le_bytes()) };
        }
        bump(&STATS.blocks);
        let write = (write + 1) % size;
        context.set_field(0, 27, 5, write);
        if write == context.field(9, 0, 5) {
            context.set_field(1, 31, 1, 0);
        }
    }
}

fn silence(voice: &mut Voice, channels: usize) {
    bump(&STATS.silent_frames);
    voice.pcm.extend(core::iter::repeat_n(0i16, XMA_FRAME_SAMPLES * channels));
}

/// Decodes the next packet of the current input into `voice.pcm`; false when
/// nothing is left.
fn decode(context: Context, voice: &mut Voice) -> bool {
    if voice.ended {
        return false;
    }
    let channels = context.channels();
    let mut current = context.field(4, 31, 1);
    let valid = context.field(0, 20, 2);
    if valid & (1 << current) == 0 {
        if valid & (1 << (1 - current)) == 0 {
            voice.ended = true;
            bump(&STATS.ended);
            context.set_field(2, 26, 5, INPUT_INVALID);
            context.set_field(2, 31, 1, 1);
            return !voice.pcm.is_empty();
        }
        current = 1 - current;
        context.set_field(4, 31, 1, current);
        context.set_field(2, 0, 26, 0);
    }
    let (address, packets) = match current {
        0 => (context.get(5), context.field(0, 0, 12)),
        _ => (context.get(6), context.field(1, 0, 12)),
    };
    let at = context.field(2, 0, 26);
    let packet = at / PACKET_BITS;
    let data = (packet < packets).then(|| crate::heap_host((address & !0x7ff).wrapping_add(packet * XMA_PACKET_BYTES as u32), XMA_PACKET_BYTES)).flatten();
    let Some(data) = data else {
        // This input is used up (or unreadable).
        context.set_field(0, 20 + current, 1, 0);
        context.set_field(2, 0, 26, 0);
        return true;
    };
    // SAFETY: heap_host checked the packet lies in the live APU heap.
    let bytes = unsafe { core::slice::from_raw_parts(data as *const u8, XMA_PACKET_BYTES) };

    if voice.stream.is_none() {
        let rate = RATES[context.field(1, 27, 2) as usize];
        voice.stream = Xma2Stream::new_hardware(rate, channels).ok();
        if voice.stream.is_none() && bump(&STATS.no_decoder) < 2 {
            crate::probe::log(format_args!("XMA decoder could not start ({rate} Hz, {channels} ch): silence instead"));
        }
    }
    bump(&STATS.packets);
    let mut pcm = Vec::with_capacity(XMA_FRAME_SAMPLES * channels * 4);
    let produced = voice.stream.as_mut().map_or(0, |s| s.decode_packet(bytes, &mut pcm));
    if voice.stream.is_none() {
        silence(voice, channels);
    } else if produced == 0 {
        voice.empty += 1;
        if voice.empty >= EMPTY_LIMIT {
            if bump(&STATS.bad_packets) < 2 {
                crate::probe::log(format_args!("XMA packet gave no samples (header {:02x?}): silence instead", &bytes[..4]));
            }
            silence(voice, channels);
        }
    } else {
        voice.empty = 0;
    }

    let mut next = (packet + 1) * PACKET_BITS;
    let loops = context.field(0, 12, 8);
    let loop_end = context.field(4, 0, 26);
    if loops > 0 && loop_end != 0 && loop_end / PACKET_BITS == packet {
        next = context.field(3, 0, 26) / PACKET_BITS * PACKET_BITS;
        voice.skip = context.field(1, 17, 3) as usize * SUBFRAME_SAMPLES;
        voice.stream = None;
        bump(&STATS.loops);
        if loops != 255 {
            context.set_field(0, 12, 8, loops - 1);
        }
    }
    let skip = (voice.skip * channels).min(pcm.len());
    voice.skip -= skip / channels;
    voice.pcm.extend(&pcm[skip..]);
    if next / PACKET_BITS >= packets {
        context.set_field(0, 20 + current, 1, 0);
        next = 0;
    }
    context.set_field(2, 0, 26, next);
    true
}

fn wake() {
    static THREAD: OnceLock<std::thread::Thread> = OnceLock::new();
    THREAD
        .get_or_init(|| {
            std::thread::Builder::new()
                .name("TuxBox XMA decoder".into())
                .spawn(|| {
                    let mut report = Instant::now();
                    loop {
                        contexts().pump();
                        if report.elapsed() >= Duration::from_secs(10) {
                            report = Instant::now();
                            report_stats();
                        }
                        // A 256-byte block lasts 2.7 ms at 48 kHz mono.
                        std::thread::park_timeout(Duration::from_millis(2));
                    }
                })
                .map_or_else(|_| std::thread::current(), |h| h.thread().clone())
        })
        .unpark();
}

fn report_stats() {
    let s = &STATS;
    let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
    let active = {
        let c = contexts();
        c.voices.iter().filter(|v| lock(v).is_some()).count()
    };
    crate::probe::log(format_args!(
        "XMA: {active} voices active; totals enable {} disable {} advance {} packets {} silent frames {} (bad packets {}) blocks {} loops {} inputs ended {}",
        g(&s.enables),
        g(&s.disables),
        g(&s.advances),
        g(&s.packets),
        g(&s.silent_frames),
        g(&s.bad_packets),
        g(&s.blocks),
        g(&s.loops),
        g(&s.ended),
    ));
}

/// IAcpHal slot 2: the coprocessor command submit.
pub(crate) unsafe extern "system" fn submit(this: *mut c_void, command: u32, id: u64, frame: u32, data: *const c_void, notification: u32) -> i32 {
    // SAFETY: same arguments the inert slot took (the rest stay on the stack).
    let result = unsafe { crate::probe::method::<2>(this, command as usize, id as usize, frame as usize) };
    let word = |at: usize| -> Option<u32> {
        // SAFETY: the command's data starts with these u32 fields.
        (!data.is_null()).then(|| unsafe { data.cast::<u32>().add(at).read_unaligned() })
    };
    crate::messages::command(command, id, &word);
    if !enabled() {
        return if crate::messages::enabled() { 0 } else { result };
    }
    static SEEN: Mutex<u64> = Mutex::new(0);
    if command < 64 {
        let mut seen = lock(&SEEN);
        if *seen & 1 << command == 0 {
            *seen |= 1 << command;
            let words: Vec<String> = (0..4).map(|i| word(i).map_or("-".into(), |w| format!("{w:08x}"))).collect();
            crate::probe::log(format_args!(
                "first coprocessor command {command} (id {id:#x}, frame {frame:#x}, data {data:p} = {}, notification {notification:#x})",
                words.join(" ")
            ));
        }
    }
    let handled = {
        let c = contexts();
        match command {
            ENABLE_XMA => {
                c.enable(word(0).unwrap_or(u32::MAX));
                true
            }
            DISABLE_XMA => {
                c.disable(word(0).unwrap_or(u32::MAX));
                true
            }
            ENABLE_XMAS | DISABLE_XMAS => {
                for i in 0..c.voices.len() as u32 {
                    if word(i as usize / 32).unwrap_or(0) & (1 << (i % 32)) != 0 {
                        if command == ENABLE_XMAS { c.enable(i) } else { c.disable(i) }
                    }
                }
                true
            }
            ADVANCE_XMA_READ => {
                c.advance_read(word(0).unwrap_or(u32::MAX), word(1).unwrap_or(0));
                true
            }
            _ => false,
        }
    };
    if handled {
        wake();
    }
    // The command is done at once: set its notification word.
    if notification != 0
        && let Some(word) = crate::heap_host(notification, 4)
    {
        // SAFETY: a u32 in the live APU heap, which the title reads as volatile.
        unsafe { (word as *mut u32).write_volatile(1) };
    }
    0
}
