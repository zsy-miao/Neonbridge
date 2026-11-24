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

/// One milestone line, every time it is called.
pub(crate) fn log_line(detail: std::fmt::Arguments) {
    log_once(&Once::new(), || detail.to_string());
}

pub(crate) fn log_once(site: &Once, detail: impl FnOnce() -> String) {
    site.call_once(|| {
        let target = std::env::var_os("TUXBOX_LOG");
        if target.as_deref() == Some(OsStr::new("stderr")) {
            // A closed console must not interrupt the game's rendering thread.
            let _ = writeln!(io::stderr().lock(), "TuxBox d3d12_xs: {}", detail());
        } else if target.as_deref() == Some(OsStr::new("1")) {
            let line = format!("TuxBox d3d12_xs: {}\n\0", detail());
            // SAFETY: All milestone text is ASCII and the buffer is NUL-terminated.
            unsafe { OutputDebugStringA(PCSTR(line.as_ptr())) };
        }
    });
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

/// The host's wave lane range (D3D12_OPTIONS1 WaveLaneCountMin/Max), read at
/// the first device creation.
static WAVE_LANES: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();

/// (min, max) wave lanes of the host adapter, once a device exists.
#[allow(dead_code)]
pub(crate) fn wave_lanes() -> Option<(u32, u32)> {
    WAVE_LANES.get().copied()
}

/// The xshader defines that compile out [WaveSize(n)] for each width the host
/// cannot run (TB_NO_WAVESIZE_<n>); a shader cache key must include them.
#[allow(dead_code)]
pub(crate) fn wave_size_defines() -> Vec<&'static str> {
    let Some((min, max)) = wave_lanes() else { return Vec::new() };
    [(32, "TB_NO_WAVESIZE_32"), (64, "TB_NO_WAVESIZE_64")]
        .into_iter()
        .filter(|&(n, _)| n < min || n > max)
        .map(|(_, define)| define)
        .collect()
}

fn note_wave_lanes(device: &ID3D12Device) {
    let mut options = windows::Win32::Graphics::Direct3D12::D3D12_FEATURE_DATA_D3D12_OPTIONS1::default();
    // SAFETY: the out structure matches the feature and outlives the call.
    if unsafe { device.CheckFeatureSupport(windows::Win32::Graphics::Direct3D12::D3D12_FEATURE_D3D12_OPTIONS1, (&raw mut options).cast(), size_of_val(&options) as u32) }.is_err() {
        return;
    }
    let lanes = (options.WaveLaneCountMin, options.WaveLaneCountMax);
    if WAVE_LANES.set(lanes).is_err() {
        return;
    }
    log_line(format_args!("host wave lanes {}..{}; xshader defines {:?}", lanes.0, lanes.1, wave_size_defines()));
    // The launcher's shader worker compiles outside this process: leave the
    // range beside its cache ("min max") for it to pick the defines.
    let path = std::env::temp_dir().join("tuxbox_shader_cache").join("wave_lanes.txt");
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
    let _ = std::fs::write(path, format!("{} {}
", lanes.0, lanes.1));
}

pub(super) fn device_created(device: &ID3D12Device) {
    crate::survey::install();
    note_wave_lanes(device);
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

/// A shader's container ID and chunk types let the package tools locate the
/// original program without copying shader bytecode into the game log.
pub(super) fn compute_failure(shader: &[u8], status: i32) {
    // Each distinct program is worth one line; repeats of the same ID add nothing.
    static FAILURES: Seen<Option<[u8; 16]>> = Seen::new(64);
    let id = (shader.starts_with(b"DXBC") && shader.len() >= 32)
        .then(|| <[u8; 16]>::try_from(&shader[4..20]).unwrap());
    match FAILURES.sight(&id) {
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
        let description = if let Some(id) = id {
            let count =
                u32::from_le_bytes([shader[28], shader[29], shader[30], shader[31]]) as usize;
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
        } else {
            "unrecognized shader container".to_owned()
        };
        format!(
            "compute pipeline creation failed ({status:#010x}, {} bytes, {description})",
            shader.len()
        )
    });
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

/// Each distinct refusal is logged once (up to 64), with its guest caller:
/// a single process-wide line hid every refusal after the first.
pub(super) fn recording_error(reason: std::fmt::Arguments<'_>) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let text = reason.to_string();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 64 && !seen.contains(&text) {
        log_line(format_args!(
            "command list cannot record {text} (guest RVA {:#x})", crate::survey::title_rva().unwrap_or(0)
        ));
        seen.push(text);
    }
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
        BUFFER_VIEWS.lock().unwrap_or_else(|e| e.into_inner()).remove(&handle);
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
        let mut buffers = BUFFER_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        for index in 0..count as usize {
            let offset = index * stride;
            if let Some(view) = map.get(&(src + offset)).copied() { map.insert(dst + offset, view); }
            else { map.remove(&(dst + offset)); }
            if let Some(view) = buffers.get(&(src + offset)).copied() { buffers.insert(dst + offset, view); }
            else { buffers.remove(&(dst + offset)); }
        }
    }

    /// A buffer view by the address range it covers (VIctory d74558c,
    /// re-derived: drop traces named buffer UAVs "None"). `title` says
    /// whether the address is the title's or, when unknown, the host's.
    #[derive(Clone, Copy)]
    pub(crate) struct BufferView { pub(crate) kind: &'static str, pub(crate) address: u64, pub(crate) bytes: u64, pub(crate) title: bool }
    impl std::fmt::Debug for BufferView {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} buffer {}{:#x}..{:#x} ({} bytes)", self.kind, if self.title { "" } else { "host " },
                self.address, self.address.saturating_add(self.bytes), self.bytes)
        }
    }
    static BUFFER_VIEWS: LazyLock<Mutex<HashMap<usize, BufferView>>> = LazyLock::new(Default::default);

    /// TUXBOX_TRACE_BUFFER_VIEWS=0: traces list texture views only, as before.
    pub(crate) fn buffer_view(handle: usize, view: BufferView) {
        if !trace_views() || std::env::var_os("TUXBOX_TRACE_BUFFER_VIEWS").as_deref() == Some(std::ffi::OsStr::new("0")) { return; }
        BUFFER_VIEWS.lock().unwrap_or_else(|e| e.into_inner()).insert(handle, view);
    }

    pub(crate) fn buffer_views(range: std::ops::Range<usize>, stride: usize) -> Vec<BufferView> {
        let map = BUFFER_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        range.step_by(stride).take(64).filter_map(|handle| map.get(&handle).copied()).collect()
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
    static NULL_PIPELINES: LazyLock<Mutex<HashSet<usize>>> = LazyLock::new(Default::default);
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
    pub(crate) fn mark_null_pipeline(native: *mut core::ffi::c_void) {
        if enabled() { NULL_PIPELINES.lock().unwrap_or_else(|e| e.into_inner()).insert(native as usize); }
    }
    static NULL_COMPUTE: LazyLock<Mutex<HashMap<usize, String>>> = LazyLock::new(Default::default);
    static NULL_DISPATCHES: LazyLock<Mutex<HashSet<(usize, [u32; 3])>>> = LazyLock::new(Default::default);
    /// Remember which untranslated compute shader a null pipeline stands in for.
    pub(crate) fn mark_null_compute(native: *mut core::ffi::c_void, digest: String) {
        if !enabled() { return; }
        NULL_COMPUTE.lock().unwrap_or_else(|e| e.into_inner()).insert(native as usize, digest);
    }
    /// Report each distinct (null compute shader, group count) dispatch once.
    pub(crate) fn null_dispatch(native: *mut core::ffi::c_void, groups: [u32; 3]) {
        if !enabled() { return; }
        let Some(digest) = NULL_COMPUTE.lock().unwrap_or_else(|e| e.into_inner()).get(&(native as usize)).cloned() else { return };
        let mut seen = NULL_DISPATCHES.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() < 120 && seen.insert((native as usize, groups)) {
            super::log_once(&std::sync::Once::new(), || format!("null compute dispatch CS={digest} groups=({},{},{})", groups[0], groups[1], groups[2]));
        }
    }
    pub(crate) fn is_null_pipeline(native: *mut core::ffi::c_void) -> bool {
        enabled() && NULL_PIPELINES.lock().unwrap_or_else(|e| e.into_inner()).contains(&(native as usize))
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
        format!("display writes recorded: draws={} (null pipeline {}) clears={} copies={} rtv-binds={} | all draws={} | display resources={}{}",
            c[0], c[1], c[2], c[3], c[5], c[4], RESOURCES.lock().unwrap_or_else(|e| e.into_inner()).len(), super::null_causes::summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_each_key_once_and_the_cap_once() {
        let seen = Seen::<String>::new(2);
        assert_eq!(seen.sight("a"), Sighting::New);
        assert_eq!(seen.sight("a"), Sighting::Repeat);
        assert_eq!(seen.sight("b"), Sighting::New);
        assert_eq!(seen.sight("c"), Sighting::Full);
        assert_eq!(seen.sight("d"), Sighting::Repeat);
        assert_eq!(seen.sight("b"), Sighting::Repeat);
    }
}

/// GPU latency per presented frame: from the present's fence signal to the GPU
/// completing it, timed on one waiter thread. Close to the frame time means the
/// GPU (all work queued before the present) is the bottleneck.
pub(crate) mod gpu_clock {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::Instant;
    use windows::Win32::Graphics::Direct3D12::ID3D12Fence;
    use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

    struct Item(ID3D12Fence, u64, Instant);
    // SAFETY: D3D12 fences are free-threaded.
    unsafe impl Send for Item {}
    static SENDER: Mutex<Option<mpsc::Sender<Item>>> = Mutex::new(None);
    static NS: AtomicU64 = AtomicU64::new(0);
    static COUNT: AtomicU64 = AtomicU64::new(0);
    static SEEN: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

    pub(crate) fn track(fence: &ID3D12Fence, value: u64) {
        let mut sender = SENDER.lock().unwrap_or_else(|e| e.into_inner());
        if sender.is_none() {
            let (tx, rx) = mpsc::channel::<Item>();
            let spawned = std::thread::Builder::new().name("TuxBox gpu clock".into()).spawn(move || {
                let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else { return };
                for Item(fence, value, start) in rx {
                    unsafe {
                        if fence.GetCompletedValue() < value {
                            if fence.SetEventOnCompletion(value, event).is_err() { continue; }
                            WaitForSingleObject(event, INFINITE);
                        }
                    }
                    NS.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    COUNT.fetch_add(1, Ordering::Relaxed);
                }
            });
            if spawned.is_err() { return; }
            *sender = Some(tx);
        }
        if let Some(tx) = sender.as_ref() {
            let _ = tx.send(Item(fence.clone(), value, Instant::now()));
        }
    }

    /// Average ms per frame since the previous call, if any frame completed.
    pub(crate) fn take() -> Option<f64> {
        let (ns, count) = (NS.load(Ordering::Relaxed), COUNT.load(Ordering::Relaxed));
        let dns = ns - SEEN[0].swap(ns, Ordering::Relaxed);
        let dcount = count - SEEN[1].swap(count, Ordering::Relaxed);
        (dcount > 0).then(|| dns as f64 / 1e6 / dcount as f64)
    }
}

/// CPU time per frame of every thread in the process (title and ours), from
/// GetThreadTimes deltas between heartbeats: says whether a title thread is
/// CPU-bound or the frame is spent waiting.
pub(crate) mod audio;
pub(crate) mod crash;
pub(crate) mod ctl;
mod crash_detail;
pub(crate) mod teb;
mod breaks;
mod streams;
pub(crate) mod memwatch;
pub(crate) mod rootsig;
pub(crate) mod a8;
mod exe_sha;
pub(crate) mod null_causes;
pub(crate) mod sampler;

pub(crate) mod threads {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, LocalFree, HLOCAL};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, GetThreadDescription, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };

    static PREV: Mutex<Option<HashMap<u32, u64>>> = Mutex::new(None);

    /// The thread's description, or "?".
    pub(crate) fn name_of(tid: u32) -> String {
        unsafe {
            let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) else { return "?".into() };
            let mut name = String::new();
            if let Ok(desc) = GetThreadDescription(thread) {
                name = desc.to_string().unwrap_or_default();
                let _ = LocalFree(Some(HLOCAL(desc.0.cast())));
            }
            let _ = CloseHandle(thread);
            if name.is_empty() { "?".into() } else { name }
        }
    }

    fn ticks(t: FILETIME) -> u64 { (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime) }

    /// "total 410.2, busy threads >50%: 3; top: Render(1234) 190.1, ..." in CPU ms per frame.
    pub(crate) fn split(frames: f64, frame_ms: f64) -> String {
        let mut now: HashMap<u32, u64> = HashMap::new();
        let mut names: HashMap<u32, String> = HashMap::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return "-".into() };
            let pid = GetCurrentProcessId();
            let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut more = Thread32First(snap, &mut entry).is_ok();
            while more {
                if entry.th32OwnerProcessID == pid
                    && let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID)
                {
                    let (mut c, mut e, mut k, mut u) = Default::default();
                    if GetThreadTimes(thread, &mut c, &mut e, &mut k, &mut u).is_ok() {
                        now.insert(entry.th32ThreadID, ticks(k) + ticks(u));
                        if let Ok(desc) = GetThreadDescription(thread) {
                            let name = desc.to_string().unwrap_or_default();
                            let _ = LocalFree(Some(HLOCAL(desc.0.cast())));
                            if !name.is_empty() { names.insert(entry.th32ThreadID, name); }
                        }
                    }
                    let _ = CloseHandle(thread);
                }
                more = Thread32Next(snap, &mut entry).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        let mut prev = PREV.lock().unwrap_or_else(|e| e.into_inner());
        let Some(old) = prev.replace(now.clone()) else { return "first sample".into() };
        // 100 ns ticks -> ms per frame.
        let mut rows: Vec<(f64, u32)> = now.iter()
            .map(|(tid, t)| ((t - old.get(tid).copied().unwrap_or(0).min(*t)) as f64 / 1e4 / frames, *tid))
            .collect();
        rows.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let total: f64 = rows.iter().map(|r| r.0).sum();
        let busy = rows.iter().filter(|r| r.0 > frame_ms * 0.5).count();
        // The next interval samples where the busiest threads execute.
        let hot: Vec<(u32, String)> = rows.iter().take(2).filter(|r| r.0 > frame_ms * 0.2)
            .map(|(_, tid)| (*tid, names.get(tid).cloned().unwrap_or_else(|| "?".into()))).collect();
        super::sampler::watch(&hot);
        let top = rows.iter().take(6).map(|(ms, tid)| {
            format!("{}({tid}) {ms:.1}", names.get(tid).map_or("?", String::as_str))
        }).collect::<Vec<_>>().join(", ");
        format!("total {total:.1}, threads over half a frame: {busy}; top: {top}")
    }
}

/// Where frame time goes on our side, for the 10 s heartbeat: nanoseconds per
/// bucket since the last heartbeat. Record covers every command-list call
/// (Close and first-use pipeline waits included); the others are subsets or
/// separate threads, so the buckets do not sum.
pub(crate) mod cost {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    #[derive(Clone, Copy)]
    pub(crate) enum Bucket { Record, Close, Submit, Present, PipelineWait, MemoryWait }
    const NAMES: [&str; 6] = ["record", "close", "submit", "present", "pipeline wait", "GPU memory wait (worker)"];
    static NS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
    static SEEN: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

    pub(crate) struct Timer(Bucket, Instant, Option<&'static Location<'static>>);
    impl Drop for Timer {
        fn drop(&mut self) {
            let ns = self.1.elapsed().as_nanos() as u64;
            NS[self.0 as usize].fetch_add(ns, Ordering::Relaxed);
            if let Some(site) = self.2 { sites::add(site, ns); }
        }
    }
    pub(crate) fn timer(bucket: Bucket) -> Timer { Timer(bucket, Instant::now(), None) }
    /// A timer that also charges its caller's call site (through any
    /// `#[track_caller]` chain), so the heartbeat names the costliest calls.
    #[track_caller]
    pub(crate) fn sited(bucket: Bucket) -> Timer { Timer(bucket, Instant::now(), Some(Location::caller())) }
    use std::panic::Location;
    /// Charges only its call site (no bucket): splits a bucket such as submit.
    pub(crate) struct Site(Instant, &'static Location<'static>);
    impl Drop for Site {
        fn drop(&mut self) { sites::add(self.1, self.0.elapsed().as_nanos() as u64); }
    }
    #[track_caller]
    pub(crate) fn site() -> Site { Site(Instant::now(), Location::caller()) }

    /// Lock-free per-call-site totals: an open-addressed table keyed by the
    /// static Location pointer. Sites past its capacity go uncounted.
    mod sites {
        use std::panic::Location;
        use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
        const N: usize = 256;
        static KEYS: [AtomicPtr<Location<'static>>; N] = [const { AtomicPtr::new(core::ptr::null_mut()) }; N];
        static NS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
        static CALLS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
        static SEEN_NS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
        static SEEN_CALLS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

        pub(super) fn add(site: &'static Location<'static>, ns: u64) {
            let key = core::ptr::from_ref(site).cast_mut();
            let start = (key as usize >> 3) % N;
            for probe in 0..N {
                let i = (start + probe) % N;
                let held = KEYS[i].load(Ordering::Relaxed);
                let ours = held == key || (held.is_null()
                    && match KEYS[i].compare_exchange(core::ptr::null_mut(), key, Ordering::Relaxed, Ordering::Relaxed) {
                        Ok(_) => true,
                        Err(other) => other == key,
                    });
                if ours {
                    NS[i].fetch_add(ns, Ordering::Relaxed);
                    CALLS[i].fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }

        /// "graphics.rs:169 80.12 (41 calls), ..." per frame since the previous call.
        pub(super) fn top(frames: f64, count: usize) -> String {
            let mut rows: Vec<(u64, u64, &'static Location<'static>)> = (0..N).filter_map(|i| {
                let key = KEYS[i].load(Ordering::Relaxed);
                if key.is_null() { return None; }
                let ns = NS[i].load(Ordering::Relaxed);
                let calls = CALLS[i].load(Ordering::Relaxed);
                let dns = ns - SEEN_NS[i].swap(ns, Ordering::Relaxed);
                let dcalls = calls - SEEN_CALLS[i].swap(calls, Ordering::Relaxed);
                // SAFETY: keys are only ever 'static Location pointers.
                (dns > 0).then(|| (dns, dcalls, unsafe { &*key }))
            }).collect();
            rows.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| (a.2.file(), a.2.line()).cmp(&(b.2.file(), b.2.line()))));
            rows.iter().take(count).map(|(ns, calls, site)| {
                let file = site.file().rsplit(['/', '\\']).next().unwrap_or(site.file());
                format!("{file}:{} {:.2} ({:.0} calls)", site.line(), *ns as f64 / 1e6 / frames, *calls as f64 / frames)
            }).collect::<Vec<_>>().join(", ")
        }
    }
    pub(crate) fn add(bucket: Bucket, ns: u64) { NS[bucket as usize].fetch_add(ns, Ordering::Relaxed); }

    /// "record 3.1, close 0.4, ..." in ms per frame since the previous call.
    pub(crate) fn split(frames: usize) -> String {
        let frames = frames.max(1) as f64;
        let buckets = (0..6).map(|i| {
            let now = NS[i].load(Ordering::Relaxed);
            let delta = now - SEEN[i].swap(now, Ordering::Relaxed);
            format!("{} {:.2}", NAMES[i], delta as f64 / 1e6 / frames)
        }).collect::<Vec<_>>().join(", ");
        // Record is summed over every recording thread, so it can exceed the
        // frame time; the top sites say which calls hold the list lock longest,
        // and the submit sub-steps (commands.rs execute_batch) split submit.
        format!("{buckets}; top sites ms/frame: {}", sites::top(frames, 8))
    }
}
