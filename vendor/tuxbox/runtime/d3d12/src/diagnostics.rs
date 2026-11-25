//! Game launch milestones for the Wine debug channel or a native console.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::sync::{Mutex, Once};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE, D3D12_COMMAND_LIST_TYPE_COMPUTE, D3D12_COMMAND_LIST_TYPE_COPY,
    D3D12_COMMAND_LIST_TYPE_DIRECT, ID3D12Device,
};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringA;
use windows::core::PCSTR;

pub(crate) fn logging() -> bool {
    matches!(
        std::env::var_os("TUXBOX_LOG").as_deref(),
        Some(target) if target == OsStr::new("stderr") || target == OsStr::new("1")
    )
}

pub(crate) fn log(detail: &str) {
    let Some(target) = std::env::var_os("TUXBOX_LOG") else { return };
    if target == OsStr::new("stderr") {
        // A closed console must not interrupt the game's rendering thread.
        // Build the complete line before writing it; formatting fragments
        // can otherwise interleave with the native driver's stderr output.
        let line = format!("TuxBox d3d12_xs: {detail}\n");
        let _ = io::stderr().lock().write_all(line.as_bytes());
    } else if target == OsStr::new("1") {
        let line = format!("TuxBox d3d12_xs: {detail}\n\0");
        // SAFETY: All milestone text is ASCII and the buffer is NUL-terminated.
        unsafe { OutputDebugStringA(PCSTR(line.as_ptr())) };
    }
}

pub(crate) fn log_once(site: &Once, detail: impl FnOnce() -> String) {
    site.call_once(|| log(&detail()));
}

/// Whether a keyed diagnostic should print. `Full` is returned once, for the
/// first new key past the cap, so truncation is visible in the log.
#[derive(Debug, PartialEq)]
pub(crate) enum Sighting {
    New,
    Full,
    Repeat,
}

/// Keys already reported, bounded so a long session cannot grow it without limit.
pub(crate) struct Seen<T> {
    state: Mutex<(Vec<T>, bool)>,
    cap: usize,
}

impl<T> Seen<T> {
    pub(crate) const fn new(cap: usize) -> Self {
        Self { state: Mutex::new((Vec::new(), false)), cap }
    }

    pub(crate) fn sight<Q>(&self, key: &Q) -> Sighting
    where
        Q: PartialEq + ToOwned<Owned = T> + ?Sized,
        T: std::borrow::Borrow<Q>,
    {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let (keys, full) = &mut *state;
        if keys.iter().any(|seen| seen.borrow() == key) {
            Sighting::Repeat
        } else if keys.len() < self.cap {
            keys.push(key.to_owned());
            Sighting::New
        } else if !*full {
            *full = true;
            Sighting::Full
        } else {
            Sighting::Repeat
        }
    }
}

pub(crate) struct RasterDraw {
    first: Once,
    samples: std::sync::atomic::AtomicUsize,
    vertex: String,
    pixel: String,
}

impl RasterDraw {
    pub(crate) fn trace(vertex: Option<&[u8]>, pixel: Option<&[u8]>, digest: impl Fn(&[u8]) -> Option<String>) -> Option<std::sync::Arc<Self>> {
        static ENABLED: std::sync::LazyLock<bool> = std::sync::LazyLock::new(||
            std::env::var_os("TUXBOX_TRACE_RASTER_DRAWS").as_deref() == Some(OsStr::new("1")));
        if !*ENABLED { return None; }
        Some(std::sync::Arc::new(Self {
            first: Once::new(),
            samples: Default::default(),
            vertex: digest(vertex?)?,
            pixel: digest(pixel?)?,
        }))
    }
    pub(crate) fn sample_label(&self) -> Option<String> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static LIMIT: AtomicUsize = AtomicUsize::new(16);
        let draw = self.samples.fetch_add(1, Ordering::Relaxed);
        if !matches!(draw, 0 | 240 | 1000 | 2000)
            || LIMIT.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_err()
        {
            return None;
        }
        Some(format!("draw PS={} index={draw}", self.pixel))
    }

    pub(crate) fn recorded(&self, detail: impl FnOnce() -> String) {
        log_once(&self.first, || format!("recorded raster draw VS={} PS={}{}", self.vertex, self.pixel, detail()));
    }
}

pub(super) fn device_created(device: &ID3D12Device) {
    crate::survey::install();
    static FIRST_DEVICE: Once = Once::new();
    log_once(&FIRST_DEVICE, || {
        // SAFETY: The factory holds this native device reference through the call.
        let luid = unsafe { device.GetAdapterLuid() };
        format!(
            "created host D3D12 device (adapter LUID {:08x}:{:08x})",
            luid.HighPart, luid.LowPart
        )
    });
}

pub(super) fn submitted(kind: D3D12_COMMAND_LIST_TYPE, count: u32) {
    static FIRST_SUBMISSION: Once = Once::new();
    log_once(&FIRST_SUBMISSION, || {
        let queue = match kind {
            D3D12_COMMAND_LIST_TYPE_DIRECT => "direct",
            D3D12_COMMAND_LIST_TYPE_COMPUTE => "compute",
            D3D12_COMMAND_LIST_TYPE_COPY => "copy",
            _ => "unknown",
        };
        format!("submitted first guest batch (queue {queue}, command lists {count})")
    });
}

pub(super) fn presented(width: u64, height: u32) {
    static FIRST_PRESENT: Once = Once::new();
    log_once(&FIRST_PRESENT, || {
        format!("PresentX presented first frame ({width}x{height})")
    });
}

fn describe_dxbc(shader: &[u8], id: Option<[u8; 16]>) -> String {
    let Some(id) = id else {
        return "unrecognized shader container".to_owned();
    };
    let count = u32::from_le_bytes([shader[28], shader[29], shader[30], shader[31]]) as usize;
    let mut chunks = Vec::new();
    for entry in shader[32..].as_chunks::<4>().0.iter().take(count.min(16)) {
        let offset = u32::from_le_bytes(*entry) as usize;
        if let Some(tag) = shader.get(offset..).and_then(|rest| rest.get(..4))
            && tag.iter().all(u8::is_ascii_alphanumeric)
            && let Ok(name) = std::str::from_utf8(tag)
        {
            chunks.push(name);
        }
    }
    let id: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!("DXBC {id}, chunks {}", chunks.join("/"))
}

fn log_compute_failure(seen: &Seen<Option<[u8; 16]>>, shader: &[u8], reason: &str) {
    let id = (shader.starts_with(b"DXBC") && shader.len() >= 32)
        .then(|| <[u8; 16]>::try_from(&shader[4..20]).unwrap());
    match seen.sight(&id) {
        Sighting::Repeat => return,
        Sighting::Full => {
            return log_once(&Once::new(), || {
                "compute pipeline failures exceeded 64 distinct shaders; further ones are not logged"
                    .to_owned()
            });
        }
        Sighting::New => {}
    }
    log_once(&Once::new(), || {
        let description = describe_dxbc(shader, id);
        format!(
            "compute pipeline creation failed ({reason}, {} bytes, {description})",
            shader.len()
        )
    });
}

/// A shader's container ID and chunk types let the package tools locate the
/// original program without copying shader bytecode into the game log.
pub(super) fn compute_failure(shader: &[u8], status: i32) {
    // Each distinct program is worth one line; repeats of the same ID add nothing.
    static FAILURES: Seen<Option<[u8; 16]>> = Seen::new(64);
    log_compute_failure(&FAILURES, shader, &format!("{status:#010x}"));
}

/// Log a compute shader skipped before driver compilation because the host
/// device's supported wave range excludes the shader's required `[WaveSize]`.
pub(crate) fn compute_wave_failure(shader: &[u8], reason: &str) {
    static WAVE_FAILURES: Seen<Option<[u8; 16]>> = Seen::new(64);
    log_compute_failure(&WAVE_FAILURES, shader, reason);
}

pub(super) fn pipeline_stream(bytes: usize, error: Option<i32>) {
    static SUCCESS: Once = Once::new();
    static FAILURE: Once = Once::new();
    match error {
        Some(code) => log_once(&FAILURE, || {
            format!("slot 86 pipeline stream ({bytes} bytes) failed: {code:#010x}")
        }),
        None => log_once(&SUCCESS, || {
            format!("created pipeline from Xbox slot 86 stream ({bytes} bytes)")
        }),
    }
}

pub(super) fn recording_error(reason: std::fmt::Arguments<'_>) {
    static FIRST: Once = Once::new();
    log_once(&FIRST, || format!("command list cannot record {reason}"));
}

// Multiple pipelines can fail on worker threads during the same frame. Keep a
// bounded list of distinct failures so the exact shader pair can be prepared.
pub(crate) fn pipeline_stages(stages: &[Option<(String, bool)>; 8], status: i32, linked: bool) {
    use std::sync::Mutex;
    static REPORTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let names = ["VS", "PS", "DS", "HS", "GS", "CS", "AS", "MS"];
    let shaders: Vec<_> = names
        .iter()
        .zip(stages)
        .filter_map(|(stage, entry)| {
            entry.as_ref().map(|(hash, cached)| {
                format!(
                    "{stage}={hash}({})",
                    if *cached { "prepared" } else { "original" }
                )
            })
        })
        .collect();
    let key = format!("{status:#010x}/{linked}/{}", shaders.join(" "));
    let id = {
        let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
        if reported.len() == 32 || reported.contains(&key) {
            return;
        }
        reported.push(key);
        reported.len()
    };
    // Wine clips long debug strings, and worker threads can interleave their
    // messages. Carry a shared number and keep at most two stages per line.
    for pair in shaders.chunks(2) {
        log_once(&Once::new(), || {
            format!(
                "slot 86 pipeline stage failure #{id} {status:#010x} linked={linked} {}",
                pair.join(" ")
            )
        });
    }
}

// A title checks the returned resource, not the HRESULT. Report the first
// distinct failed creations so the unsupported description is visible.
pub(crate) fn resource_failure(
    slot: u32, status: windows::core::HRESULT, initial: i32,
    desc: Option<&windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DESC>,
) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<(u32, i32, i32, u32)>> = Mutex::new(Vec::new());
    if status.is_ok() { return; }
    let key = desc.map_or((slot, status.0, 0, 0), |d| (slot, status.0, d.Format.0, d.Flags.0 as u32));
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() >= 32 || seen.contains(&key) { return; }
        seen.push(key);
    }
    let detail = desc.map_or_else(|| "no description".to_owned(), |d| format!(
        "dim={} {}x{}x{} mips={} format={} samples={} layout={:#x} flags={:#x} align={:#x}",
        d.Dimension.0, d.Width, d.Height, d.DepthOrArraySize, d.MipLevels, d.Format.0,
        d.SampleDesc.Count, d.Layout.0, d.Flags.0, d.Alignment));
    log_once(&Once::new(), || format!("resource slot {slot} failed {:#010x}: {detail} initial={initial:#x}", status.0));
}

/// `TUXBOX_WATCH_ADDRESS=<hex>[,<hex radius>]`: report each resource whose
/// guest memory range comes within the radius of that address. Used to find
/// the writer of corrupted title memory.
pub(crate) fn watch(kind: &str, start: u64, len: u64, detail: impl FnOnce() -> String) {
    static ADDRESS: std::sync::LazyLock<Option<(u64, u64)>> = std::sync::LazyLock::new(|| {
        let text = std::env::var("TUXBOX_WATCH_ADDRESS").ok()?;
        let (at, radius) = text.split_once(',').unwrap_or((&text, "0"));
        let hex = |t: &str| u64::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok();
        Some((hex(at)?, hex(radius)?))
    });
    let Some((address, radius)) = *ADDRESS else { return };
    let end = start.saturating_add(len);
    if !(start <= address.saturating_add(radius) && address.saturating_sub(radius) < end) {
        return;
    }
    let rva = crate::survey::title_rva().unwrap_or(0);
    let line = format!("watch {address:#x}: {kind} {start:#x}+{len:#x} {} from title RVA {rva:#x}", detail());
    log_once(&Once::new(), || line);
}

/// With `TUXBOX_TRACE_PRESENT`, count GPU writes aimed at display resources
/// (created with ALLOW_DISPLAY) so a black presented image can be attributed.
pub(crate) mod null_usage;

pub(crate) mod display {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{LazyLock, Mutex};

    pub(crate) fn enabled() -> bool {
        static ON: LazyLock<bool> = LazyLock::new(|| {
            std::env::var("TUXBOX_TRACE_PRESENT").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) != 0
        });
        *ON
    }

    pub(crate) fn trace_views() -> bool {
        static TRACE: LazyLock<bool> = LazyLock::new(|| std::env::var_os("TUXBOX_TRACE_RASTER_INPUTS").as_deref() == Some(std::ffi::OsStr::new("1")));
        *TRACE
    }

    // Diagnostic metadata only. The descriptor owner and queue retain resources.
    type TextureView = (usize, u64, u32, i32, i32);
    static TEXTURE_VIEWS: LazyLock<Mutex<HashMap<usize, TextureView>>> = LazyLock::new(Default::default);

    pub(crate) fn texture_view(handle: usize, resource: Option<&windows::Win32::Graphics::Direct3D12::ID3D12Resource>, format: Option<i32>) {
        use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        use windows::core::Interface;
        if !trace_views() { return; }
        let mut map = TEXTURE_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(resource) = resource {
            let desc = unsafe { resource.GetDesc() };
            if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D {
                map.insert(handle, (resource.as_raw() as usize, desc.Width, desc.Height, desc.Format.0, format.unwrap_or(desc.Format.0)));
                return;
            }
        }
        map.remove(&handle);
    }

    pub(crate) fn copy_texture_views(dst: usize, src: usize, count: u32, stride: usize) {
        if !trace_views() { return; }
        let mut map = TEXTURE_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        for index in 0..count as usize {
            let offset = index * stride;
            if let Some(view) = map.get(&(src + offset)).copied() { map.insert(dst + offset, view); }
            else { map.remove(&(dst + offset)); }
        }
    }

    pub(crate) fn texture_views(range: std::ops::Range<usize>, stride: usize) -> Vec<TextureView> {
        let map = TEXTURE_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        range.step_by(stride).take(64).filter_map(|handle| map.get(&handle).copied()).collect()
    }

    pub(crate) fn capture_draws() -> bool {
        static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("TUXBOX_CAPTURE_DRAW_READBACK").as_deref() == Some(std::ffi::OsStr::new("1")));
        *ON
    }
    use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
    static DRAW_TARGETS: LazyLock<Mutex<HashMap<usize, ID3D12Resource>>> = LazyLock::new(Default::default);
    pub(crate) fn draw_target(handle: usize) -> Option<ID3D12Resource> {
        DRAW_TARGETS.lock().unwrap_or_else(|e| e.into_inner()).get(&handle).cloned()
    }

    pub(crate) fn capture_target(handle: usize, resource: Option<&ID3D12Resource>) {
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
        if !capture_draws() { return; }
        let mut targets = DRAW_TARGETS.lock().unwrap_or_else(|e| e.into_inner());
        targets.remove(&handle);
        if let Some(resource) = resource {
            let desc = unsafe { resource.GetDesc() };
            if targets.len() < 256 && desc.Width >= 512 && desc.Height >= 256
                && desc.MipLevels == 1 && desc.DepthOrArraySize == 1 && desc.SampleDesc.Count == 1
                && desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM
            {
                targets.insert(handle, resource.clone());
            }
        }
    }

    static RESOURCES: LazyLock<Mutex<HashSet<usize>>> = LazyLock::new(Default::default);
    static RTVS: LazyLock<Mutex<HashMap<usize, bool>>> = LazyLock::new(Default::default);
    static NULL_USAGE: LazyLock<Mutex<super::null_usage::Usage>> = LazyLock::new(Default::default);
    // draws, null-pipeline draws, clears, copies, all draws, RTV binds
    static COUNTS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

    pub(crate) fn mark_resource(native: *mut core::ffi::c_void) {
        if enabled() { RESOURCES.lock().unwrap_or_else(|e| e.into_inner()).insert(native as usize); }
    }
    pub(crate) fn is_display(native: *mut core::ffi::c_void) -> bool {
        enabled() && RESOURCES.lock().unwrap_or_else(|e| e.into_inner()).contains(&(native as usize))
    }
    pub(crate) fn rtv(handle: usize, native: Option<*mut core::ffi::c_void>) {
        if !enabled() { return; }
        let display = native.is_some_and(is_display);
        RTVS.lock().unwrap_or_else(|e| e.into_inner()).insert(handle, display);
    }
    pub(crate) fn bound(handles: &[usize]) -> bool {
        if !enabled() { return false; }
        let rtvs = RTVS.lock().unwrap_or_else(|e| e.into_inner());
        let any = handles.iter().any(|h| rtvs.get(h).copied().unwrap_or(false));
        if any { COUNTS[5].fetch_add(1, Ordering::Relaxed); }
        any
    }
    pub(crate) fn record_null(tag: &std::sync::Arc<super::null_usage::Tag>, display: bool) {
        if !enabled() { return; }
        let rows = {
            let mut usage = NULL_USAGE.lock().unwrap_or_else(|e| e.into_inner());
            usage.record(tag, display).then(|| usage.drain())
        };
        if let Some(rows) = rows {
            log_null_usage(rows, "recording_checkpoint");
        }
    }
    // Retain the previous direct-dispatch shape samples, but deduplicate
    // by shader identity instead of a potentially recycled native pointer.
    pub(crate) fn sample_null_dispatch(tag: &std::sync::Arc<super::null_usage::Tag>, groups: [u32; 3]) {
        if !enabled() || tag.kind != "compute" { return; }
        type Sample = (std::sync::Arc<super::null_usage::Tag>, [u32; 3]);
        static SEEN: LazyLock<Mutex<HashSet<Sample>>> = LazyLock::new(Default::default);
        let report = {
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            seen.len() < 120 && seen.insert((tag.clone(), groups))
        };
        if report {
            super::log_once(&std::sync::Once::new(), || format!(
                "null compute dispatch {} groups=({},{},{})", tag.stages, groups[0], groups[1], groups[2]));
        }
    }
    pub(crate) fn report_null_usage() {
        if !enabled() { return; }
        let rows = NULL_USAGE.lock().unwrap_or_else(|e| e.into_inner()).drain();
        log_null_usage(rows, "present_sample");
    }
    fn log_null_usage(rows: Vec<String>, trigger: &'static str) {
        for row in rows {
            super::log_once(&std::sync::Once::new(), || format!("{row} trigger={trigger}"));
        }
    }
    pub(crate) fn draw(display_bound: bool, null_pipeline: bool) {
        if !enabled() { return; }
        COUNTS[4].fetch_add(1, Ordering::Relaxed);
        if display_bound {
            COUNTS[0].fetch_add(1, Ordering::Relaxed);
            if null_pipeline { COUNTS[1].fetch_add(1, Ordering::Relaxed); }
        }
    }
    pub(crate) fn clear(handle: usize) {
        if enabled() && RTVS.lock().unwrap_or_else(|e| e.into_inner()).get(&handle).copied().unwrap_or(false) {
            COUNTS[2].fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn copy(dst: *mut core::ffi::c_void) {
        if is_display(dst) { COUNTS[3].fetch_add(1, Ordering::Relaxed); }
    }
    /// Counters since the previous summary (recorded, not necessarily executed).
    pub(crate) fn summary() -> String {
        let c: Vec<u64> = COUNTS.iter().map(|c| c.swap(0, Ordering::Relaxed)).collect();
        format!("display writes recorded: draws={} (null pipeline {}) clears={} copies={} rtv-binds={} | all draws={} | display resources={}",
            c[0], c[1], c[2], c[3], c[5], c[4], RESOURCES.lock().unwrap_or_else(|e| e.into_inner()).len())
    }
}

#[cfg(test)]
#[path = "tests/diagnostics.rs"]
mod tests;
