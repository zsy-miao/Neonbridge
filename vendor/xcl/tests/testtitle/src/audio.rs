//! XAudio2 the way a console title uses it: engine with console-only flags, mastering
//! voice with a console device id, an XMA2 voice (decoded to PCM16 by XCL), an Opus voice
//! (decoded via MSOpusDecoder) and a plain PCM voice. Buffer callbacks must come back with
//! the title's contexts and the streams must end on time.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[link(name = "xaudio2_9", kind = "raw-dylib")]
extern "system" {
    fn XAudio2Create(pp: *mut *mut c_void, flags: u32, processor: u32) -> i32;
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct WaveFormatEx {
    tag: u16,
    channels: u16,
    rate: u32,
    avg_bytes: u32,
    block_align: u16,
    bits: u16,
    cb_size: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct Xma2Format {
    wfx: WaveFormatEx,
    num_streams: u16,
    channel_mask: u32,
    samples_encoded: u32,
    bytes_per_block: u32,
    play_begin: u32,
    play_length: u32,
    loop_begin: u32,
    loop_length: u32,
    loop_count: u8,
    encoder_version: u8,
    block_count: u16,
}

#[repr(C, packed)]
struct Buffer {
    flags: u32,
    audio_bytes: u32,
    audio_data: *const u8,
    play_begin: u32,
    play_length: u32,
    loop_begin: u32,
    loop_length: u32,
    loop_count: u32,
    context: *mut c_void,
}

#[repr(C, packed)]
#[derive(Default)]
struct VoiceDetails {
    creation_flags: u32,
    active_flags: u32,
    channels: u32,
    rate: u32,
}

#[repr(C, packed)]
#[derive(Default)]
struct VoiceState {
    current_buffer_context: *mut c_void,
    buffers_queued: u32,
    samples_played: u64,
}

const END_OF_STREAM: u32 = 0x40;
const XMA_CONTEXT: usize = 0x100;
const PCM_CONTEXT: usize = 0x2_0000;
const OPUS_CONTEXT: usize = 0x4000;

// Real 48 kHz mono XMA2 packet prefix (zero-padded to 2048 B; 12 frames = 6080 samples).
const XMA2_MONO_48K_PREFIX: &[u8] = &[
    0x30, 0x00, 0x00, 0x00, 0x00, 0xe7, 0x00, 0x0c, 0x13, 0xe3, 0xff, 0xff, 0xfb, 0x39, 0x46, 0x44,
    0xf6, 0x55, 0xa0, 0x1c, 0xe0, 0x01, 0x82, 0x7c, 0x7f, 0xff, 0xff, 0x67, 0x28, 0xc8, 0x9d, 0xca,
    0xb4, 0x03, 0x9c, 0x00, 0x30, 0x4f, 0x8f, 0xff, 0xff, 0xec, 0xe5, 0x19, 0x13, 0xd9, 0x56, 0x80,
    0x73, 0x80, 0x06, 0x09, 0xf1, 0xff, 0xff, 0xfd, 0x9c, 0xa3, 0x22, 0x77, 0x2a, 0xd0, 0x0e, 0x70,
    0x00, 0xc1, 0x3e, 0x3f, 0xff, 0xff, 0xb3, 0x94, 0x64, 0x4f, 0x65, 0x5a, 0x01, 0xce, 0x00, 0x18,
    0x27, 0xc7, 0xff, 0xff, 0xf6, 0x72, 0x8c, 0x89, 0xdc, 0xab, 0x40, 0x39, 0xc0, 0x03, 0x04, 0xf8,
    0xff, 0xff, 0xfe, 0xce, 0x51, 0x91, 0x3d, 0x95, 0x68, 0x07, 0x38, 0x00, 0x60, 0x9f, 0x1f, 0xff,
    0xff, 0xd9, 0xca, 0x32, 0x27, 0x72, 0xad, 0x00, 0xe7, 0x00, 0x0c, 0x13, 0xe3, 0xff, 0xff, 0xfb,
    0x39, 0x46, 0x44, 0xf6, 0x55, 0xa0, 0x1c, 0xe0, 0x01, 0x82, 0x7c, 0x7f, 0xff, 0xff, 0x67, 0x28,
    0xc8, 0x9d, 0xca, 0xb4, 0x03, 0x9c, 0x00, 0x30, 0x4f, 0x8f, 0xff, 0xff, 0xec, 0xe5, 0x19, 0x13,
    0xd9, 0x56, 0x80, 0x73, 0x80, 0x06, 0x09, 0xf1, 0xff, 0xff, 0xfd, 0x9c, 0xa3, 0x22, 0x77, 0x2a,
    0xc0,
];

// Five real 20 ms (960-sample) Hybrid FB mono Opus packets (100 ms total at 48 kHz).
const OPUS_PACKETS: &[&[u8]] = &[
    &[0x78, 0x81, 0x7b, 0xc6, 0x11, 0x76, 0xf4, 0x75, 0x00, 0x00, 0x07, 0x8e, 0xca, 0x69, 0xdb, 0xbb, 0xcc, 0x48, 0xdc, 0x16, 0xbd, 0x9b, 0xd3, 0x58, 0x27, 0x78, 0x93, 0xbc, 0x37, 0x44, 0x5e, 0xcf, 0x2d, 0x97, 0x00, 0x69, 0x80, 0x11, 0x80, 0x2c, 0x03, 0x38, 0xab, 0x45, 0x61, 0xfd, 0x89, 0xb7, 0x7e, 0x0d, 0x17, 0xbf, 0x20, 0xa5, 0x2b, 0xb5, 0xea, 0x2b, 0x5e, 0xd3, 0xa7, 0x48, 0xd1, 0x18, 0xf2, 0x0b, 0x5c, 0x01, 0x4f, 0x5f, 0xa1, 0x96, 0x21, 0xae, 0xde, 0xe0, 0x53, 0x00, 0xd6, 0x65, 0x65, 0x3f, 0x1f, 0xa6, 0xcb, 0xb9, 0x77, 0xbf, 0x97, 0x62, 0xc1, 0x12, 0xcf, 0x8d, 0xc2, 0x7d, 0xfd, 0x00, 0x80, 0xff, 0xfa, 0xc6, 0x88, 0xb6, 0xde, 0x10, 0xb8, 0x4f, 0xc6, 0x23, 0x6b, 0xea, 0x87, 0xc9, 0x05],
    &[0x78, 0x9e, 0x1e, 0xa1, 0xe7, 0xfc, 0x95, 0x4d, 0x4e, 0xaa, 0x18, 0x0a, 0xca, 0x5d, 0x07, 0x64, 0x44, 0x34, 0x94, 0x45, 0x3e, 0x66, 0x66, 0xca, 0x44, 0xed, 0x1f, 0x0d, 0x44, 0x6b, 0xf3, 0x4f, 0xcc, 0xa5, 0x55, 0x45, 0x41, 0x9e, 0xb4, 0xf3, 0xfc, 0x13, 0x38, 0x65, 0x79, 0xed, 0x99, 0xf0, 0x1b, 0x7f, 0xbd, 0x3c, 0x92, 0x73, 0xd5, 0xee, 0x8f, 0xeb, 0xab, 0x76, 0x32, 0xa5, 0xf4, 0xa1, 0x0c, 0x35, 0xbe, 0xab, 0xbb, 0x61, 0x99, 0xbf, 0xbd],
    &[0x78, 0x99, 0xc2, 0x5f, 0x75, 0x9c, 0xfc, 0x4b, 0x3a, 0x45, 0xab, 0x5a, 0x7e, 0x7d, 0x7b, 0x37, 0x7c, 0x02, 0xb4, 0x4a, 0x61, 0xf0, 0x89, 0x27, 0x87, 0xce, 0x50, 0xfb, 0xe3, 0xc3, 0x04, 0xcb, 0xaf, 0x99, 0x64, 0x7c, 0x17, 0x1a, 0x33, 0x8d, 0xeb, 0xae, 0xba, 0x19, 0x9b, 0xdb, 0xe1, 0xd7, 0x19, 0xb5, 0xc8, 0xa1, 0x79, 0xd6, 0x1c, 0xb3, 0x68, 0xe5, 0x2b, 0x25, 0x98, 0x0c, 0x5c, 0xbd, 0x64, 0x08, 0x11, 0xbb, 0x14, 0xd0, 0xbb, 0x08, 0x0d, 0xf9, 0xff, 0xdb, 0x05],
    &[0x78, 0x99, 0xc2, 0x5f, 0x73, 0x2d, 0xd2, 0x2d, 0xed, 0xd7, 0x7f, 0xc4, 0x72, 0x9e, 0xe4, 0xeb, 0x27, 0x57, 0xa3, 0x16, 0xf3, 0xb3, 0xb6, 0x2c, 0x03, 0xfc, 0x49, 0x19, 0x83, 0x78, 0x64, 0x9d, 0x41, 0x4c, 0x6a, 0x16, 0xfc, 0x80, 0xea, 0x13, 0xc6, 0x03, 0x49, 0xbf, 0xa5, 0x19, 0x46, 0x91, 0x46, 0x8c, 0xc6, 0x1e, 0x3d, 0x86, 0xd9, 0x6e, 0x57, 0xa8, 0x95, 0x55, 0x90, 0x38, 0x9d, 0xf3, 0x03, 0x88, 0x71, 0x90, 0x83, 0x6f, 0x80, 0x8a, 0x96, 0x45, 0xb9, 0x64, 0xc3, 0xb6, 0x4d],
    &[0x78, 0x99, 0xc2, 0x5f, 0x73, 0x2d, 0xd2, 0x31, 0x33, 0xbf, 0x7b, 0x34, 0x8a, 0x71, 0x0f, 0x77, 0xd5, 0xd5, 0x1b, 0xda, 0x78, 0xa4, 0x47, 0xbb, 0x8c, 0x85, 0x53, 0xbe, 0x21, 0xd6, 0x5f, 0x9f, 0x2e, 0xba, 0x38, 0xfa, 0x2d, 0x32, 0xbc, 0xa1, 0xd7, 0xce, 0xb0, 0x1b, 0x08, 0x44, 0x28, 0x21, 0xcb, 0x80, 0x40, 0xaf, 0xc4, 0x08, 0xdc, 0x96, 0xfd, 0xb6, 0x95, 0x96, 0xda, 0xf3, 0xae, 0x6c, 0x2b, 0x04, 0xc4, 0x23, 0xb8, 0x12, 0x45, 0x51, 0xf9, 0x5f, 0x9b, 0xb0, 0xc6, 0x86, 0xed, 0x08, 0x0c],
];

static BUFFER_END: AtomicUsize = AtomicUsize::new(0);
static STREAM_END: AtomicUsize = AtomicUsize::new(0);
static VOICE_ERRORS: AtomicUsize = AtomicUsize::new(0);
static STARTED: OnceLock<Instant> = OnceLock::new();
/// ms from Start to the end of the XMA2 buffer
static XMA_END_MS: AtomicU64 = AtomicU64::new(u64::MAX);
/// ms from Start to the end of the Opus buffer
static OPUS_END_MS: AtomicU64 = AtomicU64::new(u64::MAX);

// IXAudio2VoiceCallback (no IUnknown)
#[repr(C)]
struct CallbackVtbl {
    pass_start: unsafe extern "system" fn(*mut c_void, u32),
    pass_end: unsafe extern "system" fn(*mut c_void),
    stream_end: unsafe extern "system" fn(*mut c_void),
    buffer_start: unsafe extern "system" fn(*mut c_void, *mut c_void),
    buffer_end: unsafe extern "system" fn(*mut c_void, *mut c_void),
    loop_end: unsafe extern "system" fn(*mut c_void, *mut c_void),
    voice_error: unsafe extern "system" fn(*mut c_void, *mut c_void, i32),
}

unsafe extern "system" fn on_pass_start(_: *mut c_void, _: u32) {}
unsafe extern "system" fn on_nothing(_: *mut c_void) {}
unsafe extern "system" fn on_stream_end(_: *mut c_void) {
    STREAM_END.fetch_add(1, Ordering::SeqCst);
}
unsafe extern "system" fn on_ctx(_: *mut c_void, _: *mut c_void) {}
unsafe extern "system" fn on_buffer_end(_: *mut c_void, ctx: *mut c_void) {
    if ctx as usize == XMA_CONTEXT {
        if let Some(t0) = STARTED.get() {
            XMA_END_MS.store(t0.elapsed().as_millis() as u64, Ordering::SeqCst);
        }
    } else if ctx as usize == OPUS_CONTEXT {
        if let Some(t0) = STARTED.get() {
            OPUS_END_MS.store(t0.elapsed().as_millis() as u64, Ordering::SeqCst);
        }
    }
    BUFFER_END.fetch_add(ctx as usize, Ordering::SeqCst);
}
unsafe extern "system" fn on_error(_: *mut c_void, _: *mut c_void, _: i32) {
    VOICE_ERRORS.fetch_add(1, Ordering::SeqCst);
}

static VTBL: CallbackVtbl = CallbackVtbl {
    pass_start: on_pass_start,
    pass_end: on_nothing,
    stream_end: on_stream_end,
    buffer_start: on_ctx,
    buffer_end: on_buffer_end,
    loop_end: on_ctx,
    voice_error: on_error,
};

#[repr(C)]
struct Callback {
    vtbl: &'static CallbackVtbl,
}

static CALLBACK: Callback = Callback { vtbl: &VTBL };

unsafe fn method<F: Copy>(obj: *mut c_void, slot: usize) -> F {
    let vt = *(obj as *const *const usize);
    std::mem::transmute_copy::<usize, F>(&*vt.add(slot))
}

type CreateSourceVoice =
    unsafe extern "system" fn(*mut c_void, *mut *mut c_void, *const c_void, u32, f32, *const c_void, *const c_void, *const c_void) -> i32;
type CreateMasteringVoice = unsafe extern "system" fn(*mut c_void, *mut *mut c_void, u32, u32, u32, *const u16, *const c_void, u32) -> i32;
type GetVoiceDetails = unsafe extern "system" fn(*mut c_void, *mut VoiceDetails);
type SetVolume = unsafe extern "system" fn(*mut c_void, f32, u32) -> i32;
type Submit = unsafe extern "system" fn(*mut c_void, *const Buffer, *const c_void) -> i32;
type GetState = unsafe extern "system" fn(*mut c_void, *mut VoiceState, u32);
type Start = unsafe extern "system" fn(*mut c_void, u32, u32) -> i32;
type Destroy = unsafe extern "system" fn(*mut c_void);
type Release = unsafe extern "system" fn(*mut c_void) -> u32;

pub fn xaudio2_console_voices() -> Result<(), String> {
    unsafe {
        let mut eng: *mut c_void = std::ptr::null_mut();
        // 0x1 / 0x40: flags the Windows engine does not accept; 0x3c: a console core mask
        let hr = XAudio2Create(&mut eng, 0x41, 0x3c);
        if hr != 0 || eng.is_null() {
            return Err(format!("XAudio2Create → {:#x}", hr as u32));
        }
        let release: Release = method(eng, 2);
        let device = super::wide("console-audio-endpoint-0");
        let mut master: *mut c_void = std::ptr::null_mut();
        let hr = method::<CreateMasteringVoice>(eng, 7)(eng, &mut master, 2, 48_000, 0x4, device.as_ptr(), std::ptr::null(), 6);
        if hr < 0 {
            println!("  (no audio device: CreateMasteringVoice → {:#x}; skipping voices)", hr as u32);
            release(eng);
            return Ok(());
        }

        // XMA2 mono 48 kHz – decoded to PCM16 by XCL (Windows XAudio2 cannot play it natively)
        let xma = Xma2Format {
            wfx: WaveFormatEx { tag: 0x166, channels: 1, rate: 48_000, avg_bytes: 6000, block_align: 1, bits: 16, cb_size: 34 },
            num_streams: 1,
            channel_mask: 4,
            samples_encoded: 6080,
            bytes_per_block: 2048,
            encoder_version: 4,
            block_count: 1,
            ..Default::default()
        };
        let create_source: CreateSourceVoice = method(eng, 5);
        let mut xv: *mut c_void = std::ptr::null_mut();
        let cb = &CALLBACK as *const Callback as *const c_void;
        let hr = create_source(eng, &mut xv, &xma as *const Xma2Format as *const c_void, 0x2, 2.0, cb, std::ptr::null(), std::ptr::null());
        if hr != 0 || xv.is_null() {
            return Err(format!("CreateSourceVoice(XMA2) → {:#x}", hr as u32));
        }
        let mut det = VoiceDetails::default();
        method::<GetVoiceDetails>(xv, 0)(xv, &mut det);
        let (ch, rate) = (det.channels, det.rate);
        if (ch, rate) != (1, 48_000) {
            return Err(format!("XMA2 voice: {ch} ch., {rate} Hz"));
        }
        let _ = method::<SetVolume>(xv, 12)(xv, 0.0, 0); // stay silent during automated tests
        let mut packet = vec![0u8; 2048];
        packet[..XMA2_MONO_48K_PREFIX.len()].copy_from_slice(XMA2_MONO_48K_PREFIX);
        let xb = Buffer {
            flags: END_OF_STREAM,
            audio_bytes: 2048,
            audio_data: packet.as_ptr(),
            play_begin: 0,
            play_length: 4800, // 0.1 s slice of the 6080 decoded samples
            loop_begin: 0,
            loop_length: 0,
            loop_count: 0,
            context: XMA_CONTEXT as *mut c_void,
        };
        let hr = method::<Submit>(xv, 21)(xv, &xb, std::ptr::null());
        if hr != 0 {
            return Err(format!("SubmitSourceBuffer(XMA2) → {:#x}", hr as u32));
        }
        let mut xst = VoiceState::default();
        method::<GetState>(xv, 25)(xv, &mut xst, 0);
        if { xst.current_buffer_context } as usize != XMA_CONTEXT || { xst.buffers_queued } != 1 {
            return Err(format!(
                "XMA2 GetState: ctx={:#x}, queued={}",
                { xst.current_buffer_context } as usize,
                { xst.buffers_queued }
            ));
        }

        // plain PCM16 voice must pass through untouched
        let pcm = WaveFormatEx { tag: 1, channels: 1, rate: 48_000, avg_bytes: 96_000, block_align: 2, bits: 16, cb_size: 0 };
        let mut pv: *mut c_void = std::ptr::null_mut();
        let hr = create_source(eng, &mut pv, &pcm as *const WaveFormatEx as *const c_void, 0, 2.0, cb, std::ptr::null(), std::ptr::null());
        if hr != 0 || pv.is_null() {
            return Err(format!("CreateSourceVoice(PCM) → {:#x}", hr as u32));
        }
        let samples = vec![0i16; 2400]; // 0.05 s
        let pb = Buffer {
            flags: END_OF_STREAM,
            audio_bytes: 4800,
            audio_data: samples.as_ptr() as *const u8,
            play_begin: 0,
            play_length: 0,
            loop_begin: 0,
            loop_length: 0,
            loop_count: 0,
            context: PCM_CONTEXT as *mut c_void,
        };
        if method::<Submit>(pv, 21)(pv, &pb, std::ptr::null()) != 0 {
            return Err("SubmitSourceBuffer(PCM)".into());
        }

        // Opus mono 48 kHz (0x704F) in a 2048 B AWC-framed block – decoded to PCM16 by XCL
        let opus_fmt = WaveFormatEx { tag: 0x704F, channels: 1, rate: 48_000, avg_bytes: 4000, block_align: 2048, bits: 0, cb_size: 0 };
        let mut ov: *mut c_void = std::ptr::null_mut();
        let hr = create_source(eng, &mut ov, &opus_fmt as *const WaveFormatEx as *const c_void, 0, 2.0, cb, std::ptr::null(), std::ptr::null());
        if hr != 0 || ov.is_null() {
            return Err(format!("CreateSourceVoice(Opus) → {:#x}", hr as u32));
        }
        let mut odet = VoiceDetails::default();
        method::<GetVoiceDetails>(ov, 0)(ov, &mut odet);
        if (odet.channels, odet.rate) != (1, 48_000) {
            return Err(format!("Opus voice: {} ch., {} Hz", { odet.channels }, { odet.rate }));
        }
        let _ = method::<SetVolume>(ov, 12)(ov, 0.0, 0); // stay silent during automated tests
        let mut opus_block = vec![0u8; 2048];
        let mut pos = 0usize;
        for &pkt in OPUS_PACKETS {
            opus_block[pos..pos + 2].copy_from_slice(&(pkt.len() as u16).to_le_bytes());
            opus_block[pos + 2..pos + 2 + pkt.len()].copy_from_slice(pkt);
            pos += 2 + pkt.len();
        }
        let ob = Buffer {
            flags: END_OF_STREAM,
            audio_bytes: opus_block.len() as u32,
            audio_data: opus_block.as_ptr(),
            play_begin: 0,
            play_length: 0, // decoded length: 5 * 960 = 4800 samples = 0.1 s
            loop_begin: 0,
            loop_length: 0,
            loop_count: 0,
            context: OPUS_CONTEXT as *mut c_void,
        };
        let hr = method::<Submit>(ov, 21)(ov, &ob, std::ptr::null());
        if hr != 0 {
            return Err(format!("SubmitSourceBuffer(Opus) → {:#x}", hr as u32));
        }
        let mut ost = VoiceState::default();
        method::<GetState>(ov, 25)(ov, &mut ost, 0);
        if { ost.current_buffer_context } as usize != OPUS_CONTEXT || { ost.buffers_queued } != 1 {
            return Err(format!(
                "Opus GetState: ctx={:#x}, queued={}",
                { ost.current_buffer_context } as usize,
                { ost.buffers_queued }
            ));
        }

        STARTED.get_or_init(Instant::now);
        for v in [xv, pv, ov] {
            let hr = method::<Start>(v, 19)(v, 0, 0);
            if hr != 0 {
                return Err(format!("Start → {:#x}", hr as u32));
            }
        }
        let expected_ctx = XMA_CONTEXT + PCM_CONTEXT + OPUS_CONTEXT;
        let t0 = Instant::now();
        while (BUFFER_END.load(Ordering::SeqCst) != expected_ctx || STREAM_END.load(Ordering::SeqCst) < 3)
            && t0.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let ends = BUFFER_END.load(Ordering::SeqCst);
        if ends != expected_ctx || STREAM_END.load(Ordering::SeqCst) < 3 {
            return Err(format!("callbacks: OnBufferEnd context sum {ends:#x}, OnStreamEnd {}", STREAM_END.load(Ordering::SeqCst)));
        }
        if VOICE_ERRORS.load(Ordering::SeqCst) != 0 {
            return Err("OnVoiceError".into());
        }
        // the XMA2 buffer (PlayLength 4800 = 0.1 s) must take as long as the title expects
        // (± one 10 ms engine quantum: the clock starts mid-quantum; 79 ms was seen)
        let ms = XMA_END_MS.load(Ordering::SeqCst);
        if !(70..=2000).contains(&ms) {
            return Err(format!("XMA2 buffer ended after {ms} ms (expected ≈100)"));
        }
        let oms = OPUS_END_MS.load(Ordering::SeqCst);
        if !(70..=2000).contains(&oms) {
            return Err(format!("Opus buffer ended after {oms} ms (expected ≈100)"));
        }
        println!("  (XMA2 buffer ended after {ms} ms; Opus buffer ended after {oms} ms)");
        for v in [xv, pv, ov, master] {
            method::<Destroy>(v, 18)(v);
        }
        release(eng);
        drop((packet, samples, opus_block));
        Ok(())
    }
}
