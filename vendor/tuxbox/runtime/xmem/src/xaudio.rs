//! The title's host XAudio2 (xaudio2_9) gets console arguments that Windows
//! reads differently:
//! - XAudio2Create's processor argument is a console core mask; on Windows it
//!   pins the audio thread, possibly onto a busy core. It becomes
//!   XAUDIO2_USE_DEFAULT_PROCESSOR (0) (TUXBOX_XAUDIO_PROCESSOR=<n> sets it).
//! - Engine flags outside STOP_ENGINE_WHEN_IDLE 0x2000, 1024_QUANTUM 0x8000
//!   and NO_VIRTUAL_AUDIO_CLIENT 0x10000 are console-only and dropped;
//!   TUXBOX_XAUDIO_1024=0 also drops the 1024-sample quantum (~21 ms latency).
//! - CreateMasteringVoice (engine slot 7): the console device id is ignored
//!   (default device); COM missing (CO_E_NOTINITIALIZED) joins the MTA and
//!   retries; any other failure retries with the host's own format (0
//!   channels, 0 Hz) and the game-effects category.
//! - A mastering rate of 0 (the device's own) becomes 48000 Hz, as
//!   omonkeyz/xbox-emulator xaudio2_9 engine.rs create_mastering_voice does
//!   (GPL-3.0-or-later): test50's device mixed at 192 kHz, so XAudio2
//!   resampled every voice 4x on its thread and the intro audio broke up.
//!   TUXBOX_MASTER_RATE=<Hz> picks another rate; 0 leaves it to the device.
//!
//! Both the exe's imports and its GetProcAddress lookups are covered.
//! TUXBOX_XAUDIO_FIX=0 passes every call through unchanged (still logged).
//! Behaviour re-derived from xcl's description (GPL-2.0); no code taken.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::loader::{log, patch_imports};

type H = *mut c_void;
type Create = unsafe extern "system" fn(*mut H, u32, u32) -> i32;
type CreateVersion = unsafe extern "system" fn(*mut H, u32, u32, u32) -> i32;
type CreateMaster = unsafe extern "system" fn(H, *mut H, u32, u32, u32, *const u16, *const c_void, u32) -> i32;

unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> H;
    fn LoadLibraryExW(name: *const u16, file: H, flags: u32) -> H;
    fn GetProcAddress(module: H, name: *const u8) -> *const c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *const c_void, flags: u32) -> i32;
}

const KEPT_FLAGS: u32 = 0x2000 | 0x8000 | 0x10000;
const QUANTUM_1024: u32 = 0x8000;
const MASTER_SLOT: usize = 7;
const CO_E_NOTINITIALIZED: i32 = 0x8004_01F0_u32 as i32;
const GAME_EFFECTS: u32 = 6;

/// The rate a mastering voice asking for the device rate (0) gets.
fn master_rate(asked: u32) -> u32 {
    if asked != 0 {
        return asked;
    }
    std::env::var("TUXBOX_MASTER_RATE").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(48_000)
}

/// The created mastering voice's input rate (IXAudio2Voice::GetVoiceDetails).
unsafe fn voice_rate(voice: *mut H) -> u32 {
    if voice.is_null() {
        return 0;
    }
    unsafe {
        let object = *voice;
        if object.is_null() {
            return 0;
        }
        let mut details = [0u32; 4];
        let get: unsafe extern "system" fn(H, *mut u32) = std::mem::transmute(*(*object.cast::<*const usize>()));
        get(object, details.as_mut_ptr());
        details[3]
    }
}

static REAL_CREATE: AtomicUsize = AtomicUsize::new(0);
static REAL_CREATE_VERSION: AtomicUsize = AtomicUsize::new(0);
static REAL_MASTER: AtomicUsize = AtomicUsize::new(0);

fn enabled() -> bool {
    !std::env::var("TUXBOX_XAUDIO_FIX").is_ok_and(|v| v.trim() == "0")
}

/// The real export, from the loaded (or System32) xaudio2_9.dll.
unsafe fn real(slot: &AtomicUsize, name: &[u8]) -> usize {
    let cached = slot.load(Ordering::Acquire);
    if cached != 0 {
        return cached;
    }
    let dll: Vec<u16> = "xaudio2_9.dll".encode_utf16().chain([0]).collect();
    unsafe {
        let mut module = GetModuleHandleW(dll.as_ptr());
        if module.is_null() {
            module = LoadLibraryExW(dll.as_ptr(), std::ptr::null_mut(), 0x800); // LOAD_LIBRARY_SEARCH_SYSTEM32
        }
        if module.is_null() {
            return 0;
        }
        let found = GetProcAddress(module, name.as_ptr()) as usize;
        slot.store(found, Ordering::Release);
        found
    }
}

fn arguments(flags: u32, processor: u32) -> (u32, u32) {
    if !enabled() {
        return (flags, processor);
    }
    let mut kept = flags & KEPT_FLAGS;
    if std::env::var("TUXBOX_XAUDIO_1024").is_ok_and(|v| v.trim() == "0") {
        kept &= !QUANTUM_1024;
    }
    let processor = std::env::var("TUXBOX_XAUDIO_PROCESSOR").ok().and_then(|v| {
        let v = v.trim();
        v.strip_prefix("0x").map_or_else(|| v.parse().ok(), |h| u32::from_str_radix(h, 16).ok())
    }).unwrap_or(0);
    (kept, processor)
}

unsafe extern "system" fn create(engine: *mut H, flags: u32, processor: u32) -> i32 {
    let (f, p) = arguments(flags, processor);
    let real = unsafe { real(&REAL_CREATE, b"XAudio2Create\0") };
    if real == 0 {
        log("xaudio2: XAudio2Create not found in xaudio2_9.dll");
        return 0x8007_007E_u32 as i32; // ERROR_MOD_NOT_FOUND
    }
    let hr = unsafe { std::mem::transmute::<usize, Create>(real)(engine, f, p) };
    log(&format!("xaudio2: XAudio2Create flags {flags:#x} processor {processor:#x} -> flags {f:#x} processor {p:#x} = {hr:#x}"));
    if hr >= 0 { unsafe { hook_engine(engine) }; }
    hr
}

unsafe extern "system" fn create_version(engine: *mut H, flags: u32, processor: u32, version: u32) -> i32 {
    let (f, p) = arguments(flags, processor);
    let real = unsafe { real(&REAL_CREATE_VERSION, b"XAudio2CreateWithVersionInfo\0") };
    if real == 0 {
        log("xaudio2: XAudio2CreateWithVersionInfo not found in xaudio2_9.dll");
        return 0x8007_007E_u32 as i32;
    }
    let hr = unsafe { std::mem::transmute::<usize, CreateVersion>(real)(engine, f, p, version) };
    log(&format!("xaudio2: XAudio2CreateWithVersionInfo flags {flags:#x} processor {processor:#x} version {version:#x} -> flags {f:#x} processor {p:#x} = {hr:#x}"));
    if hr >= 0 { unsafe { hook_engine(engine) }; }
    hr
}

/// Wrap CreateMasteringVoice in the engine vtable (shared by every engine).
unsafe fn hook_engine(engine: *mut H) {
    if engine.is_null() {
        return;
    }
    unsafe {
        let object = *engine;
        if object.is_null() {
            return;
        }
        let at = (*object.cast::<*mut usize>()).add(MASTER_SLOT);
        let current = at.read();
        let ours = create_master as *const () as usize;
        if current == ours {
            return;
        }
        REAL_MASTER.store(current, Ordering::Release);
        let mut old = 0;
        if VirtualProtect(at.cast(), 8, 0x04, &mut old) == 0 {
            log("xaudio2: VirtualProtect on the engine vtable failed; CreateMasteringVoice not wrapped");
            return;
        }
        at.write(ours);
        let mut ignored = 0;
        VirtualProtect(at.cast(), 8, old, &mut ignored);
    }
}

unsafe extern "system" fn create_master(
    engine: H, voice: *mut H, channels: u32, rate: u32, flags: u32, device: *const u16, effects: *const c_void, category: u32,
) -> i32 {
    let real: CreateMaster = unsafe { std::mem::transmute(REAL_MASTER.load(Ordering::Acquire)) };
    let named = !device.is_null();
    if !enabled() {
        let hr = unsafe { real(engine, voice, channels, rate, flags, device, effects, category) };
        log(&format!("xaudio2: CreateMasteringVoice {channels} ch {rate} Hz flags {flags:#x} device {} category {category} = {hr:#x} (fix off)", if named { "named" } else { "default" }));
        return hr;
    }
    let call = |c, r, cat| unsafe { real(engine, voice, c, r, flags, std::ptr::null(), effects, cat) };
    let asked = rate;
    let rate = master_rate(rate);
    let mut hr = call(channels, rate, category);
    if hr < 0 && rate != asked {
        log(&format!("xaudio2: CreateMasteringVoice at {rate} Hz failed {hr:#x}; asking for the device rate"));
        hr = call(channels, asked, category);
    }
    let mut how = "as asked";
    if hr == CO_E_NOTINITIALIZED {
        let co = unsafe { CoInitializeEx(std::ptr::null(), 0) }; // COINIT_MULTITHREADED
        log(&format!("xaudio2: CreateMasteringVoice found no COM on this thread; CoInitializeEx(MTA) = {co:#x}"));
        hr = call(channels, rate, category);
        how = "after joining the MTA";
    }
    if hr < 0 {
        let first = hr;
        hr = call(0, 0, GAME_EFFECTS);
        log(&format!("xaudio2: CreateMasteringVoice {channels} ch {rate} Hz category {category} failed {first:#x}; retried with the host format"));
        how = "with the host format";
    }
    let got = if hr >= 0 { unsafe { voice_rate(voice) } } else { 0 };
    log(&format!("xaudio2: CreateMasteringVoice {channels} ch, asked {asked} Hz, requested {rate} Hz (TUXBOX_MASTER_RATE), got {got} Hz, flags {flags:#x} category {category}, console device id {} = {hr:#x} ({how})",
        if named { "ignored" } else { "none" }));
    hr
}

/// Wraps CreateMasteringVoice before any title engine exists: a private
/// engine from the real XAudio2Create gives the engine vtable, which every
/// engine shares. test51 never reached our wrap: the exe imports no
/// XAudio2Create (0 wrapped) and neither engine's mastering voice went
/// through it, so both mastered at the device's 192 kHz. Runs on its own
/// thread, after the loader lock, with COM joined (MTA).
/// TUXBOX_XAUDIO_EARLY=0 skips it.
pub(crate) fn hook_vtable_early() {
    if !enabled() || std::env::var("TUXBOX_XAUDIO_EARLY").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let _ = std::thread::Builder::new().name("TuxBox XAudio2 hook".into()).spawn(|| unsafe {
        let co = CoInitializeEx(std::ptr::null(), 0);
        let real = real(&REAL_CREATE, b"XAudio2Create\0");
        if real == 0 {
            log("xaudio2: early hook found no XAudio2Create in xaudio2_9.dll");
            return;
        }
        let mut engine: H = std::ptr::null_mut();
        let hr = std::mem::transmute::<usize, Create>(real)(&mut engine, 0, 0);
        if hr < 0 || engine.is_null() {
            log(&format!("xaudio2: early hook XAudio2Create = {hr:#x}; CreateMasteringVoice not wrapped yet"));
            return;
        }
        hook_engine(&mut engine);
        let wrapped = (*(*engine.cast::<*const usize>()).add(MASTER_SLOT)) == create_master as *const () as usize;
        let release: unsafe extern "system" fn(H) -> u32 = std::mem::transmute(*(*engine.cast::<*const usize>()).add(2));
        release(engine);
        log(&format!("xaudio2: engine vtable CreateMasteringVoice {} at startup (CoInitializeEx {co:#x}; TUXBOX_XAUDIO_EARLY=0 skips)",
            if wrapped { "wrapped" } else { "NOT wrapped" }));
    });
}

/// GetProcAddress by ordinal (net.rs): #1 is XAudio2Create.
pub(crate) fn lookup_ordinal(dll: &str, ordinal: usize) -> Option<usize> {
    (dll.starts_with("xaudio2_9") && ordinal == CREATE_ORDINAL as usize).then_some(create as *const () as usize)
}

/// GetProcAddress hook (net.rs): our wrappers for xaudio2_9's create exports.
pub(crate) fn lookup(dll: &str, name: &str) -> Option<usize> {
    if !dll.starts_with("xaudio2_9") {
        return None;
    }
    match name {
        "XAudio2Create" => Some(create as *const () as usize),
        "XAudio2CreateWithVersionInfo" => Some(create_version as *const () as usize),
        _ => None,
    }
}

/// XAudio2Create is export #1 of xaudio2_9; Bink2Scarlett.dll imports it by
/// ordinal for the intro movies' own engine.
const CREATE_ORDINAL: u16 = 1;

/// Wraps a title DLL's XAudio2 imports (by name and by ordinal), so its
/// engine gets host arguments too: Bink created its own engine with the
/// console core mask and flags, and the intro movie sounded bad while the
/// title's own audio was fine. TUXBOX_XAUDIO_DLLS=0 leaves DLLs alone.
pub(crate) unsafe fn patch_module(module: H, label: &str) -> usize {
    if module.is_null() || std::env::var("TUXBOX_XAUDIO_DLLS").is_ok_and(|v| v.trim() == "0") {
        return 0;
    }
    let n = unsafe {
        crate::loader::patch_module_imports(module.cast(), |d| d.starts_with("xaudio2_9"), &[
            ("XAudio2Create", create as *const () as usize),
            ("XAudio2CreateWithVersionInfo", create_version as *const () as usize),
        ], &[(CREATE_ORDINAL, create as *const () as usize)])
    };
    if n > 0 {
        log(&format!("xaudio2: {n} XAudio2Create import(s) of {label} wrapped (TUXBOX_XAUDIO_DLLS=0 skips)"));
    }
    n
}

/// Called from loader::install (xmem DllMain), after the exe's imports are bound.
pub(crate) unsafe fn install() {
    // Bink2Scarlett is bound with the exe, before any late-load hook runs.
    let bink: Vec<u16> = "bink2scarlett.dll".encode_utf16().chain([0]).collect();
    unsafe { patch_module(GetModuleHandleW(bink.as_ptr()), "Bink2Scarlett.dll") };
    let patched = unsafe {
        patch_imports(|d| d.starts_with("xaudio2_9"), &[
            ("XAudio2Create", create as *const () as usize),
            ("XAudio2CreateWithVersionInfo", create_version as *const () as usize),
        ])
    };
    log(&format!("xaudio2: {patched} XAudio2Create import(s) wrapped{}", if enabled() { "" } else { " (TUXBOX_XAUDIO_FIX=0: arguments pass through)" }));
    hook_vtable_early();
}
