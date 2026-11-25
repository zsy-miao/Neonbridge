//! TUXBOX_WC_ATTRIB (default on, =0 off): process private commit moved by
//! each kind of native create, summed per 10 s and logged with the moves no
//! create here accounts for. test182 held 12.9 GiB of write-combined host
//! private memory against 1.1 GiB our counters name, growing with
//! device-local use past its budget; this says which kind of create (if
//! any) it comes from. Other threads move private bytes during a call too,
//! so a kind's figure is an upper bound; summed over many calls the owner
//! stands out.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    /// The title's heaps (render heaps).
    Heap,
    /// Our page pools and span heaps.
    Pool,
    /// Reserved (tiled) resources.
    Reserved,
    /// Pipeline states and state objects.
    Pipeline,
    /// Committed resources of placed textures and device buffers.
    Committed,
    /// Shadow copy buffers and their upload slabs.
    Shadow,
}

const N: usize = 6;
const NAMES: [&str; N] = ["title heaps", "page pools/span heaps", "reserved resources", "pipelines", "committed resources", "shadow buffers"];
static CALLS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static MOVED: [AtomicI64; N] = [const { AtomicI64::new(0) }; N];
static WINDOW: std::sync::Mutex<Option<(std::time::Instant, u64)>> = std::sync::Mutex::new(None);

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_WC_ATTRIB") != Some("0"))
}

/// Runs one native create of `kind`, counting the private bytes it moved.
pub(crate) fn around<T>(kind: Kind, create: impl FnOnce() -> T) -> T {
    if !on() {
        return create();
    }
    let before = super::memory::private_bytes();
    let out = create();
    let moved = super::memory::private_bytes() as i64 - before as i64;
    CALLS[kind as usize].fetch_add(1, Relaxed);
    MOVED[kind as usize].fetch_add(moved, Relaxed);
    report();
    out
}

static TITLE_PIPELINES: AtomicI64 = AtomicI64::new(0);

static RELEASES: [AtomicI64; 3] = [const { AtomicI64::new(0) }; 3];

/// A title pipeline Release that left `left` references (compute::release_pipeline).
pub(crate) fn pipeline_release(left: u32) {
    if crate::env_toggle!("TUXBOX_PIPELINE_RELEASES") == Some("0") {
        return;
    }
    RELEASES[0].fetch_add(1, Relaxed);
    match left {
        0 => { RELEASES[2].fetch_add(1, Relaxed); }
        1 => { RELEASES[1].fetch_add(1, Relaxed); }
        _ => {}
    }
}

/// Held by each title pipeline's data: counts the live ones.
pub(crate) struct TitlePipeline(());

impl Default for TitlePipeline {
    fn default() -> Self {
        TITLE_PIPELINES.fetch_add(1, Relaxed);
        Self(())
    }
}

impl Drop for TitlePipeline {
    fn drop(&mut self) {
        TITLE_PIPELINES.fetch_sub(1, Relaxed);
    }
}

/// A host pipeline made or loaded: counted live until its last release.
pub(crate) fn host_pipeline(made: windows::core::Result<windows::Win32::Graphics::Direct3D12::ID3D12PipelineState>) -> windows::core::Result<windows::Win32::Graphics::Direct3D12::ID3D12PipelineState> {
    if on() && let Ok(p) = &made {
        super::memory::live_on(p, super::memory::Category::HostPipeline, 0);
    }
    made
}

/// Pipelines stored in the library (raw pointers: the library holds a
/// reference to each for its life, so they stay valid).
static STORED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

/// A StorePipeline result: a stored pipeline is remembered for the count of
/// those only the library still holds.
pub(crate) fn stored(ok: bool, made: &windows::Win32::Graphics::Direct3D12::ID3D12PipelineState) -> bool {
    if ok && on() {
        STORED.lock().unwrap_or_else(|e| e.into_inner()).push(windows::core::Interface::as_raw(made) as usize);
    }
    ok
}

/// (stored, held by the library alone): a reference count of 1.
fn library_only() -> (usize, usize) {
    let stored = STORED.lock().unwrap_or_else(|e| e.into_inner());
    let alone = stored.iter().filter(|&&raw| {
        // SAFETY: the library keeps each stored pipeline alive; AddRef then
        // Release leaves the count as it was and returns it.
        unsafe {
            let ptr = raw as *mut core::ffi::c_void;
            let unknown = <windows::core::IUnknown as windows::core::Interface>::from_raw_borrowed(&ptr).expect("non-null");
            let vtable = windows::core::Interface::vtable(unknown);
            (vtable.AddRef)(raw as _);
            (vtable.Release)(raw as _) == 1
        }
    }).count();
    (stored.len(), alone)
}

fn report() {
    let now = std::time::Instant::now();
    let private = super::memory::private_bytes();
    let mut window = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let Some((start, base)) = *window else {
        *window = Some((now, private));
        for m in &MOVED { m.store(0, Relaxed); }
        return;
    };
    if now.duration_since(start).as_secs() < 10 {
        return;
    }
    *window = Some((now, private));
    drop(window);
    let mut parts = String::new();
    let mut named = 0i64;
    for i in 0..N {
        let calls = CALLS[i].swap(0, Relaxed);
        let moved = MOVED[i].swap(0, Relaxed);
        named += moved;
        if calls != 0 {
            parts.push_str(&format!("{} {calls} calls {:+} MiB; ", NAMES[i], moved >> 20));
        }
    }
    let total = private as i64 - base as i64;
    let (stored, alone) = library_only();
    crate::diagnostics::log_line(format_args!(
        "private commit by create, last {:.1} s: {parts}all creates {:+} MiB of {:+} MiB in all, so {:+} MiB moved outside these creates; live pipelines: host (ours) {}, title {}, held only by the library {alone} of {stored} stored; title pipeline Releases {} (left at 1 ref {}, freed {}; TUXBOX_PIPELINE_RELEASES=0 off) (TUXBOX_WC_ATTRIB=0 off)",
        now.duration_since(start).as_secs_f64(), named >> 20, total >> 20, (total - named) >> 20,
        super::memory::live_count(super::memory::Category::HostPipeline), TITLE_PIPELINES.load(Relaxed),
        RELEASES[0].load(Relaxed), RELEASES[1].load(Relaxed), RELEASES[2].load(Relaxed)));
}
