//! Device slot 55, CreateDerivedPipelineStateX(base, count, patches, riid,
//! out): a pipeline made from another with some fixed-function state
//! replaced. Each patch record is 0x14C bytes: a part at +0 and its payload
//! at +4 (0 depth-stencil, 1 rasterizer, 2 blend, 3 sample mask). Ported
//! from palmetto b00248ca (Widmer, GPL-3.0): the base's guest stream is kept
//! when it is made, patched, and made again through slot 86, so the derived
//! pipeline gets its own translation and lazy build like any other.
//! Repeated derivations of one base with the same patches share one
//! pipeline. A base without a kept stream (slot 57 records, or past the
//! store's TUXBOX_DERIVED_PSO_MB budget, 512) falls back to returning the
//! base, as before. TUXBOX_DERIVED_PSO=base always does.
use super::Device;
use super::stream::background::OwnedStream;
use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Graphics::Direct3D12::D3D12_PIPELINE_STATE_STREAM_DESC;
use windows::core::{GUID, HRESULT, Interface};

const RECORD: usize = 0x14C;
const MAX_PATCHES: u32 = 16;
const MAX_CACHED: usize = 16384;

static KEPT_BYTES: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static MADE: AtomicU64 = AtomicU64::new(0);
static SHARED: AtomicU64 = AtomicU64::new(0);
/// (base proxy, patch bytes) -> the derived proxy, holding one reference.
static CACHE: Mutex<Option<HashMap<(usize, Vec<u8>), usize>>> = Mutex::new(None);

fn on() -> bool {
    crate::env_toggle!("TUXBOX_DERIVED_PSO") != Some("base")
}

fn budget() -> usize {
    static MB: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MB.get_or_init(|| crate::env_toggle!("TUXBOX_DERIVED_PSO_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(512)) << 20
}

/// A copy of a graphics stream being made, kept for later derivations.
pub(super) fn keep(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<Arc<OwnedStream>> {
    if !on() {
        return None;
    }
    let budget = budget();
    if KEPT_BYTES.load(Ordering::Relaxed) >= budget {
        return None;
    }
    let stream = unsafe { OwnedStream::copy(desc) }?;
    if KEPT_BYTES.fetch_add(stream.owned_bytes(), Ordering::Relaxed) >= budget {
        static FULL: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FULL, || format!(
            "CreateDerivedPipelineStateX: the kept base streams reached {} MiB; later bases derive as the base itself (TUXBOX_DERIVED_PSO_MB)", budget >> 20));
    }
    Some(Arc::new(stream))
}

/// The tags a part replaces (the first is added when none is present) and its payload size.
fn part(part: u32) -> Option<(&'static [i32], usize)> {
    Some(match part {
        0 => (&[21, 11], 0x34),
        1 => (&[10], 0x2C),
        2 => (&[8], 0x148),
        3 => (&[9], 4),
        _ => return None,
    })
}

unsafe fn com_call(object: *mut c_void, slot: usize) -> u32 {
    type Ref = unsafe extern "system" fn(*mut c_void) -> u32;
    // SAFETY: object is a live COM object; slots 1 and 2 are AddRef and Release.
    unsafe { (*object.cast::<*const Ref>()).add(slot).read()(object) }
}

unsafe fn query(object: *mut c_void, riid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    type Query = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    // SAFETY: object is a live COM object; slot 0 is QueryInterface.
    unsafe { (*object.cast::<*const Query>()).read()(object, riid, out) }
}

/// The derived pipeline's status, or `None` to fall back to the base.
pub(super) unsafe fn create(
    device: &Device, base: *mut c_void, count: u32, patches: *const u8, riid: *const GUID, out: *mut *mut c_void,
) -> Option<HRESULT> {
    if !on() || count == 0 || count > MAX_PATCHES || patches.is_null() {
        return None;
    }
    let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let owner = super::identity(device).as_raw();
    let base_pipeline = unsafe { super::compute::pipeline(base, owner) }.ok().flatten()?;
    if base_pipeline.state.compute {
        return None;
    }
    let stream = base_pipeline.state.derive_from.clone()?;
    // SAFETY: the title passes `count` records of RECORD bytes.
    let bytes = unsafe { core::slice::from_raw_parts(patches, count as usize * RECORD) }.to_vec();
    let mut parts = Vec::new();
    for record in bytes.chunks_exact(RECORD) {
        let number = u32::from_le_bytes(record[..4].try_into().ok()?);
        let Some((tags, size)) = part(number) else {
            static UNKNOWN: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&UNKNOWN, || format!(
                "CreateDerivedPipelineStateX: patch part {number} has no known state; the base pipeline is returned"));
            return None;
        };
        parts.push((tags, &record[4..4 + size]));
    }
    let key = (base as usize, bytes.clone());
    if let Some(&made) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&key) {
        SHARED.fetch_add(1, Ordering::Relaxed);
        return Some(unsafe { query(made as *mut c_void, riid, out) });
    }
    let Some(mut storage) = stream.patched(&parts) else {
        static UNPATCHED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&UNPATCHED, || "CreateDerivedPipelineStateX: a patched stream did not fit; the base pipeline is returned".to_owned());
        return None;
    };
    let desc = D3D12_PIPELINE_STATE_STREAM_DESC {
        SizeInBytes: storage.len() * size_of::<usize>(),
        pPipelineStateSubobjectStream: storage.as_mut_ptr().cast(),
    };
    // `stream` stays alive across the create: the patched records point into it.
    let status = unsafe { super::stream::create_pipeline(device, &desc, riid, out) };
    drop(stream);
    let made = if status.is_ok() { unsafe { out.read() } } else { core::ptr::null_mut() };
    if !made.is_null() {
        MADE.fetch_add(1, Ordering::Relaxed);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let cache = cache.get_or_insert_with(HashMap::new);
        if cache.len() < MAX_CACHED {
            unsafe { com_call(made, 1) };
            cache.insert(key, made as usize);
        }
    }
    if n <= 8 || n.is_power_of_two() {
        let names: Vec<i32> = parts.iter().map(|(tags, _)| tags[0]).collect();
        crate::diagnostics::log_line(format_args!(
            "CreateDerivedPipelineStateX: base {base:p}, patched subobjects {names:?}: {:#010x} ({n} calls, {} made, {} shared; TUXBOX_DERIVED_PSO=base returns the base)",
            status.0 as u32, MADE.load(Ordering::Relaxed), SHARED.load(Ordering::Relaxed)));
    }
    Some(status)
}
