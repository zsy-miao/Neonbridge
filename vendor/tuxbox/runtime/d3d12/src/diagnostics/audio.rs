//! Diagnostics: is the title's host XAudio2 output running dry? Once
//! xaudio2_9.dll is loaded, the source-voice vtable (shared by every source
//! voice, so voices the title made earlier are covered) has SubmitSourceBuffer
//! and DestroyVoice wrapped. Each submission records how many buffers were
//! still queued on that voice: 0 means the voice had played everything and
//! went silent before the title's next buffer (an audible gap). One line per
//! 10 s. TUXBOX_AUDIO_PROBE=0 turns it off.
//!
//! The vtable is found through a private engine with a mastering voice and
//! one source voice, destroyed again right after.
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
use windows::core::{s, w};

type Submit = unsafe extern "system" fn(*mut c_void, *const u8, *const c_void) -> i32;
type Destroy = unsafe extern "system" fn(*mut c_void);
type GetState = unsafe extern "system" fn(*mut c_void, *mut VoiceState, u32);

#[repr(C)]
struct VoiceState {
    context: *mut c_void,
    queued: u32,
    samples: u64,
}

const DESTROY: usize = 18;
const SUBMIT: usize = 21;
const GET_STATE: usize = 25;
const NO_SAMPLES_PLAYED: u32 = 0x100;

static ORIGINAL_SUBMIT: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_DESTROY: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_STATE: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct Voice {
    last: Option<Instant>,
    submits: u32,
    bytes: u64,
    longest_gap_ms: f64,
}

#[derive(Default)]
struct Window {
    started: Option<Instant>,
    voices: HashMap<usize, Voice>,
    submits: u32,
    dry: u32,
    one_left: u32,
    queued_total: u64,
    destroyed: u32,
    threads: Vec<u32>,
}

static WINDOW: Mutex<Option<Window>> = Mutex::new(None);

unsafe extern "system" fn submit(voice: *mut c_void, buffer: *const u8, wma: *const c_void) -> i32 {
    let mut state = VoiceState { context: std::ptr::null_mut(), queued: u32::MAX, samples: 0 };
    let get = ORIGINAL_STATE.load(Ordering::Relaxed);
    if get != 0 {
        // SAFETY: the voice is live for the duration of the title's call.
        unsafe { std::mem::transmute::<usize, GetState>(get)(voice, &mut state, NO_SAMPLES_PLAYED) };
    }
    let bytes = if buffer.is_null() { 0 } else { unsafe { buffer.add(4).cast::<u32>().read_unaligned() } };
    note(voice as usize, state.queued, bytes);
    let original = ORIGINAL_SUBMIT.load(Ordering::Relaxed);
    unsafe { std::mem::transmute::<usize, Submit>(original)(voice, buffer, wma) }
}

unsafe extern "system" fn destroy(voice: *mut c_void) {
    {
        let mut guard = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
        let window = guard.get_or_insert_with(Window::default);
        if window.voices.remove(&(voice as usize)).is_some() {
            window.destroyed += 1;
        }
    }
    let original = ORIGINAL_DESTROY.load(Ordering::Relaxed);
    unsafe { std::mem::transmute::<usize, Destroy>(original)(voice) }
}

fn note(voice: usize, queued: u32, bytes: u32) {
    let now = Instant::now();
    let mut guard = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let window = guard.get_or_insert_with(Window::default);
    let started = *window.started.get_or_insert(now);
    let entry = window.voices.entry(voice).or_default();
    if let Some(last) = entry.last.replace(now) {
        entry.longest_gap_ms = entry.longest_gap_ms.max(now.duration_since(last).as_secs_f64() * 1e3);
    }
    entry.submits += 1;
    entry.bytes += u64::from(bytes);
    window.submits += 1;
    if queued != u32::MAX {
        window.dry += u32::from(queued == 0);
        window.one_left += u32::from(queued == 1);
        window.queued_total += u64::from(queued);
    }
    let thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    if !window.threads.contains(&thread) && window.threads.len() < 8 {
        window.threads.push(thread);
    }
    let seconds = now.duration_since(started).as_secs_f64();
    if seconds < 10.0 {
        return;
    }
    let mut busiest: Vec<&Voice> = window.voices.values().filter(|v| v.submits > 0).collect();
    busiest.sort_unstable_by(|a, b| b.submits.cmp(&a.submits));
    let top = busiest.iter().take(3).map(|v| format!("{:.0}/s {:.0} KB/s gap {:.0} ms",
        f64::from(v.submits) / seconds, v.bytes as f64 / 1024.0 / seconds, v.longest_gap_ms))
        .collect::<Vec<_>>().join(", ");
    super::log_line(format_args!(
        "xaudio2 last {seconds:.1} s: {} submits on {} voices ({} destroyed); queue at submit: {} empty (voice ran dry), {} one left, mean {:.1}; busiest voices: {top}; threads {:?}",
        window.submits, busiest.len(), window.destroyed, window.dry, window.one_left,
        window.queued_total as f64 / f64::from(window.submits.max(1)), window.threads));
    let voices = std::mem::take(&mut window.voices).into_keys().map(|k| (k, Voice::default())).collect();
    *window = Window { started: Some(now), voices, ..Default::default() };
}

/// Patch one vtable slot; returns the previous function.
unsafe fn patch(vtable: *mut usize, slot: usize, function: usize) -> Option<usize> {
    let at = unsafe { vtable.add(slot) };
    let mut old = PAGE_PROTECTION_FLAGS(0);
    unsafe { VirtualProtect(at.cast(), 8, PAGE_READWRITE, &mut old).ok()? };
    let previous = unsafe { at.read() };
    unsafe { at.write(function) };
    let mut ignored = PAGE_PROTECTION_FLAGS(0);
    let _ = unsafe { VirtualProtect(at.cast(), 8, old, &mut ignored) };
    Some(previous)
}

/// Call from a thread that holds no locks, once xaudio2_9.dll is loaded.
pub(crate) fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if std::env::var_os("TUXBOX_AUDIO_PROBE").is_some_and(|v| v == "0") {
        return;
    }
    ONCE.call_once(|| {
        let result = unsafe { install_now() };
        super::log_line(format_args!("xaudio2 probe: {result}"));
    });
}

unsafe fn install_now() -> String {
    type Create = unsafe extern "system" fn(*mut *mut c_void, u32, u32) -> i32;
    type CreateMaster = unsafe extern "system" fn(*mut c_void, *mut *mut c_void, u32, u32, u32, *const u16, *const c_void, u32) -> i32;
    type CreateSource = unsafe extern "system" fn(*mut c_void, *mut *mut c_void, *const u8, u32, f32, *const c_void, *const c_void, *const c_void) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    let Ok(module) = (unsafe { GetModuleHandleW(w!("xaudio2_9.dll")) }) else { return "xaudio2_9.dll not loaded".into() };
    let Some(create) = (unsafe { GetProcAddress(module, s!("XAudio2Create")) }) else { return "no XAudio2Create export".into() };
    let create: Create = unsafe { std::mem::transmute(create) };
    let mut engine = std::ptr::null_mut();
    let hr = unsafe { create(&mut engine, 0, 1) };
    if hr < 0 || engine.is_null() {
        return format!("XAudio2Create failed {hr:#x}");
    }
    let vtable = |object: *mut c_void| unsafe { *object.cast::<*mut usize>() };
    let slot = |object: *mut c_void, i: usize| unsafe { *vtable(object).add(i) };
    let mut master = std::ptr::null_mut();
    let hr = unsafe { std::mem::transmute::<usize, CreateMaster>(slot(engine, 7))(engine, &mut master, 2, 48_000, 0, std::ptr::null(), std::ptr::null(), 6) };
    let outcome = if hr < 0 || master.is_null() {
        format!("private mastering voice failed {hr:#x}")
    } else {
        // 48 kHz stereo 16-bit PCM WAVEFORMATEX.
        let mut format = [0u8; 18];
        format[0..2].copy_from_slice(&1u16.to_le_bytes());
        format[2..4].copy_from_slice(&2u16.to_le_bytes());
        format[4..8].copy_from_slice(&48_000u32.to_le_bytes());
        format[8..12].copy_from_slice(&192_000u32.to_le_bytes());
        format[12..14].copy_from_slice(&4u16.to_le_bytes());
        format[14..16].copy_from_slice(&16u16.to_le_bytes());
        let mut source = std::ptr::null_mut();
        let hr = unsafe { std::mem::transmute::<usize, CreateSource>(slot(engine, 5))(engine, &mut source, format.as_ptr(), 0, 2.0, std::ptr::null(), std::ptr::null(), std::ptr::null()) };
        let outcome = if hr < 0 || source.is_null() {
            format!("private source voice failed {hr:#x}")
        } else {
            let table = vtable(source);
            ORIGINAL_STATE.store(slot(source, GET_STATE), Ordering::Relaxed);
            // Originals are stored before the slot changes, so a title call
            // racing the patch always finds them.
            ORIGINAL_SUBMIT.store(slot(source, SUBMIT), Ordering::Relaxed);
            ORIGINAL_DESTROY.store(slot(source, DESTROY), Ordering::Relaxed);
            let ok = unsafe { patch(table, SUBMIT, submit as *const () as usize).is_some() && patch(table, DESTROY, destroy as *const () as usize).is_some() };
            unsafe { std::mem::transmute::<usize, Destroy>(slot(source, DESTROY))(source) };
            // IXAudio2 calls the title makes while playing tell us its engine.
            let engine_table = vtable(engine);
            for (i, f) in [(5usize, engine_create_source as *const () as usize), (6, engine_create_submix as *const () as usize),
                (10, engine_commit as *const () as usize), (11, engine_performance as *const () as usize)] {
                ENGINE_ORIGINALS[ENGINE_SLOTS.iter().position(|&x| x == i).unwrap()].store(slot(engine, i), Ordering::Relaxed);
                let _ = unsafe { patch(engine_table, i, f) };
            }
            ENGINE_PERFORMANCE.store(slot(engine, 11), Ordering::Relaxed);
            if ok { format!("source voice vtable {table:p} wrapped (SubmitSourceBuffer, DestroyVoice); engine vtable {engine_table:p} wrapped (CreateSourceVoice, CreateSubmixVoice, CommitChanges, GetPerformanceData)") } else { "VirtualProtect on the vtable failed".into() }
        };
        unsafe { std::mem::transmute::<usize, Destroy>(slot(master, DESTROY))(master) };
        outcome
    };
    unsafe { std::mem::transmute::<usize, Release>(slot(engine, 2))(engine) };
    outcome
}

// ---- WASAPI: the device buffer the title's own mixer fills ----
//
// IAudioClient::GetCurrentPadding is wrapped (frames still queued for the
// device when the title checks; 0 = the device played everything: an audible
// gap) and IAudioRenderClient::ReleaseBuffer (frames written, silent flag).
// The vtables are shared by every client of the class audioses.dll hands out.

const PADDING: usize = 6;
const RELEASE: usize = 4;
static ORIGINAL_PADDING: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_RELEASE: AtomicUsize = AtomicUsize::new(0);
static BUFFER_FRAMES: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct Device {
    started: Option<Instant>,
    last_release: Option<Instant>,
    releases: u32,
    frames: u64,
    silent: u32,
    longest_gap_ms: f64,
    gaps_over_20: u32,
    paddings: u32,
    empty: u32,
    under_10ms: u32,
    padding_sum: u64,
    padding_min: u32,
    threads: Vec<u32>,
}

static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

fn thread_id() -> u32 {
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

unsafe extern "system" fn padding(client: *mut c_void, out: *mut u32) -> windows::core::HRESULT {
    type F = unsafe extern "system" fn(*mut c_void, *mut u32) -> windows::core::HRESULT;
    let hr = unsafe { std::mem::transmute::<usize, F>(ORIGINAL_PADDING.load(Ordering::Relaxed))(client, out) };
    if hr.is_ok() && !out.is_null() {
        let frames = unsafe { out.read() };
        let mut guard = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
        let d = guard.get_or_insert_with(|| Device { padding_min: u32::MAX, ..Default::default() });
        d.paddings += 1;
        d.empty += u32::from(frames == 0);
        d.under_10ms += u32::from(frames < 480);
        d.padding_sum += u64::from(frames);
        d.padding_min = d.padding_min.min(frames);
    }
    hr
}

unsafe extern "system" fn release(render: *mut c_void, frames: u32, flags: u32) -> windows::core::HRESULT {
    type F = unsafe extern "system" fn(*mut c_void, u32, u32) -> windows::core::HRESULT;
    let now = Instant::now();
    {
        let mut guard = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
        let d = guard.get_or_insert_with(|| Device { padding_min: u32::MAX, ..Default::default() });
        let started = *d.started.get_or_insert(now);
        if let Some(last) = d.last_release.replace(now) {
            let gap = now.duration_since(last).as_secs_f64() * 1e3;
            d.longest_gap_ms = d.longest_gap_ms.max(gap);
            d.gaps_over_20 += u32::from(gap > 20.0);
        }
        d.releases += 1;
        d.frames += u64::from(frames);
        d.silent += u32::from(flags & 2 != 0);
        let thread = thread_id();
        if !d.threads.contains(&thread) && d.threads.len() < 8 {
            d.threads.push(thread);
        }
        let seconds = now.duration_since(started).as_secs_f64();
        if seconds >= 10.0 {
            super::log_line(format_args!(
                "wasapi last {seconds:.1} s: {} ReleaseBuffer, {:.0} frames/s written ({} silent), longest gap {:.0} ms, {} gaps over 20 ms; padding checks {}: {} empty (device ran dry), {} under 10 ms, min {}, mean {:.0} frames of a {}-frame buffer; threads {:?}",
                d.releases, d.frames as f64 / seconds, d.silent, d.longest_gap_ms, d.gaps_over_20, d.paddings, d.empty,
                d.under_10ms, if d.padding_min == u32::MAX { 0 } else { d.padding_min },
                d.padding_sum as f64 / f64::from(d.paddings.max(1)), BUFFER_FRAMES.load(Ordering::Relaxed), d.threads));
            *d = Device { started: Some(now), last_release: Some(now), padding_min: u32::MAX, ..Default::default() };
        }
    }
    unsafe { std::mem::transmute::<usize, F>(ORIGINAL_RELEASE.load(Ordering::Relaxed))(render, frames, flags) }
}

/// Call from a thread that holds no locks, once audioses.dll is loaded.
pub(crate) fn install_wasapi() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if std::env::var_os("TUXBOX_AUDIO_PROBE").is_some_and(|v| v == "0") {
        return;
    }
    ONCE.call_once(|| {
        let result = unsafe { install_wasapi_now() }.unwrap_or_else(|e| format!("failed: {e:?}"));
        super::log_line(format_args!("wasapi probe: {result}"));
    });
}

unsafe fn install_wasapi_now() -> windows::core::Result<String> {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;
    use windows::core::Interface;
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole)? };
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
    let format = unsafe { client.GetMixFormat()? };
    let initialized = unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None) };
    unsafe { CoTaskMemFree(Some(format.cast())) };
    initialized?;
    let render: IAudioRenderClient = unsafe { client.GetService()? };
    let slot = |object: *mut c_void, i: usize| unsafe { (*object.cast::<*mut usize>()).add(i) };
    let (client_table, render_table) = (slot(client.as_raw(), 0), slot(render.as_raw(), 0));
    // Who opens an endpoint, and with what format and buffer: IMMDevice::Activate and IAudioClient::Initialize.
    let device_table = slot(device.as_raw(), 0);
    ORIGINAL_ACTIVATE.store(unsafe { *slot(device.as_raw(), 3) }, Ordering::Relaxed);
    ORIGINAL_INITIALIZE.store(unsafe { *slot(client.as_raw(), 3) }, Ordering::Relaxed);
    let _ = unsafe { patch(device_table, 3, activate as *const () as usize) };
    let _ = unsafe { patch(client_table, 3, initialize as *const () as usize) };
    ORIGINAL_PADDING.store(unsafe { *slot(client.as_raw(), PADDING) }, Ordering::Relaxed);
    ORIGINAL_RELEASE.store(unsafe { *slot(render.as_raw(), RELEASE) }, Ordering::Relaxed);
    let ok = unsafe {
        patch(client_table, PADDING, padding as *const () as usize).is_some()
            && patch(render_table, RELEASE, release as *const () as usize).is_some()
    };
    Ok(if ok {
        format!("IAudioClient vtable {client_table:p} and IAudioRenderClient vtable {render_table:p} wrapped (GetCurrentPadding, ReleaseBuffer)")
    } else {
        "VirtualProtect on the vtables failed".into()
    })
}

// ---- the title's XAudio2 engine: glitches and voice counts ----

const ENGINE_SLOTS: [usize; 4] = [5, 6, 10, 11];
static ENGINE_ORIGINALS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
static ENGINE_PERFORMANCE: AtomicUsize = AtomicUsize::new(0);
static TITLE_ENGINE: AtomicUsize = AtomicUsize::new(0);
static ENGINE_CALLS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

fn seen(engine: *mut c_void, index: usize) -> usize {
    TITLE_ENGINE.store(engine as usize, Ordering::Relaxed);
    ENGINE_CALLS[index].fetch_add(1, Ordering::Relaxed);
    ENGINE_ORIGINALS[index].load(Ordering::Relaxed)
}

unsafe extern "system" fn engine_create_source(e: *mut c_void, a: *mut c_void, b: *const c_void, c: u32, d: f32, f: *const c_void, g: *const c_void, h: *const c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void, *const c_void, u32, f32, *const c_void, *const c_void, *const c_void) -> i32;
    unsafe { std::mem::transmute::<usize, F>(seen(e, 0))(e, a, b, c, d, f, g, h) }
}
unsafe extern "system" fn engine_create_submix(e: *mut c_void, a: *mut c_void, b: u32, c: u32, d: u32, f: u32, g: *const c_void, h: *const c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32, u32, *const c_void, *const c_void) -> i32;
    unsafe { std::mem::transmute::<usize, F>(seen(e, 1))(e, a, b, c, d, f, g, h) }
}
unsafe extern "system" fn engine_commit(e: *mut c_void, a: u32) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, u32) -> i32;
    unsafe { std::mem::transmute::<usize, F>(seen(e, 2))(e, a) }
}
unsafe extern "system" fn engine_performance(e: *mut c_void, a: *mut u8) {
    type F = unsafe extern "system" fn(*mut c_void, *mut u8);
    unsafe { std::mem::transmute::<usize, F>(seen(e, 3))(e, a) }
}

/// For the heartbeat: the title engine's XAUDIO2_PERFORMANCE_DATA, if one was seen.
pub(crate) fn engine_report() -> Option<String> {
    use std::sync::atomic::AtomicU32;
    static LAST_GLITCHES: AtomicU32 = AtomicU32::new(0);
    let calls: Vec<usize> = ENGINE_CALLS.iter().map(|c| c.swap(0, Ordering::Relaxed)).collect();
    let engine = TITLE_ENGINE.load(Ordering::Relaxed);
    let get = ENGINE_PERFORMANCE.load(Ordering::Relaxed);
    if get == 0 {
        return None;
    }
    if engine == 0 {
        return Some("xaudio2 engine: the title made no CreateSourceVoice/CreateSubmixVoice/CommitChanges/GetPerformanceData call since the probe".into());
    }
    // XAUDIO2_PERFORMANCE_DATA is 64 bytes.
    let mut data = [0u8; 64];
    type F = unsafe extern "system" fn(*mut c_void, *mut u8);
    unsafe { std::mem::transmute::<usize, F>(get)(engine as *mut c_void, data.as_mut_ptr()) };
    let u64_at = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
    let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    let glitches = u32_at(32);
    let new = glitches.wrapping_sub(LAST_GLITCHES.swap(glitches, Ordering::Relaxed));
    let (audio, total) = (u64_at(0), u64_at(8));
    Some(format!(
        "xaudio2 engine: {new} new glitches ({glitches} since start), audio thread busy {:.0}% of its cycles, latency {} samples, source voices {} active / {} total, submix {} active; title calls since last line: create source {}, create submix {}, commit {}, performance {}",
        if total == 0 { 0.0 } else { audio as f64 * 100.0 / total as f64 }, u32_at(28),
        u32_at(36), u32_at(40), u32_at(44), calls[0], calls[1], calls[2], calls[3]))
}

static ORIGINAL_ACTIVATE: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_INITIALIZE: AtomicUsize = AtomicUsize::new(0);

fn callers() -> String {
    let mut frames = [std::ptr::null_mut(); 8];
    let n = unsafe { windows::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace(2, &mut frames, None) } as usize;
    let list = super::sampler::modules();
    frames[..n].iter().map(|&f| super::sampler::label(&list, f as usize)).collect::<Vec<_>>().join(" < ")
}

unsafe extern "system" fn activate(device: *mut c_void, iid: *const windows::core::GUID, context: u32, params: *const c_void, out: *mut *mut c_void) -> windows::core::HRESULT {
    type F = unsafe extern "system" fn(*mut c_void, *const windows::core::GUID, u32, *const c_void, *mut *mut c_void) -> windows::core::HRESULT;
    let hr = unsafe { std::mem::transmute::<usize, F>(ORIGINAL_ACTIVATE.load(Ordering::Relaxed))(device, iid, context, params, out) };
    let iid = if iid.is_null() { "null".into() } else { format!("{:?}", unsafe { *iid }) };
    super::log_line(format_args!("audio endpoint: IMMDevice::Activate {iid} -> {:#x}; callers {}", hr.0, callers()));
    hr
}

unsafe extern "system" fn initialize(client: *mut c_void, mode: i32, flags: u32, duration: i64, period: i64, format: *const u8, session: *const c_void) -> windows::core::HRESULT {
    type F = unsafe extern "system" fn(*mut c_void, i32, u32, i64, i64, *const u8, *const c_void) -> windows::core::HRESULT;
    let hr = unsafe { std::mem::transmute::<usize, F>(ORIGINAL_INITIALIZE.load(Ordering::Relaxed))(client, mode, flags, duration, period, format, session) };
    let fmt = if format.is_null() { "null".into() } else {
        let w = |o: usize| u16::from_le_bytes(unsafe { [*format.add(o), *format.add(o + 1)] });
        let d = |o: usize| u32::from_le_bytes(unsafe { [*format.add(o), *format.add(o + 1), *format.add(o + 2), *format.add(o + 3)] });
        format!("tag {:#x} {} ch {} Hz {} bits", w(0), w(2), d(4), w(14))
    };
    super::log_line(format_args!(
        "audio endpoint: IAudioClient::Initialize {} flags {flags:#x} buffer {:.1} ms period {:.1} ms format {fmt} -> {:#x}; callers {}",
        if mode == 1 { "exclusive" } else { "shared" }, duration as f64 / 1e4, period as f64 / 1e4, hr.0, callers()));
    hr
}
