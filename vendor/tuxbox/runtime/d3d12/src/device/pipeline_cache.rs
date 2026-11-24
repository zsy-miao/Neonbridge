//! Host pipelines kept on disk and the pipelines a run bound (VIctory's
//! pipeline library and early list, decision 0007), in this build's
//! translation cache folder so a rebuilt translator starts both afresh.
//!
//! 1. Every host pipeline TuxBox makes from a translated stream or compute
//!    descriptor goes into a D3D12 pipeline library, named by a digest of
//!    its host description. A later run loads it instead of compiling it
//!    again. The driver refuses a library of another adapter or driver, and
//!    an entry whose description differs, so neither is ever used.
//! 2. The guest pipelines a run bound are listed by a key known when the
//!    title creates them. TuxBox makes pipelines when they are created (not
//!    on first use, as VIctory does), so the list works the other way round:
//!    once a list exists, a pipeline that no run bound spends none of the
//!    title's translator wait budget and goes straight to the workers.
//!
//! All library calls are serialised: the interface does not promise that
//! stores, loads and serialising may overlap. A saver thread writes both
//! files every 20 s when they changed.
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{HSTRING, Interface, Result};

const LIBRARY_FILE: &str = "pipelines.d3d12lib";
const USED_FILE: &str = "used-pipelines.txt";

struct Library {
    library: ID3D12PipelineLibrary1,
    /// The library reads its entries from this blob for its whole life.
    _blob: Vec<u8>,
    dirty: bool,
}

// SAFETY: D3D12 pipeline libraries are free-threaded; every call is made
// with the mutex held.
unsafe impl Send for Library {}

enum State {
    /// The translation cache folder is not published yet.
    Unknown,
    Ready(Option<Library>),
}

static LIBRARY: Mutex<State> = Mutex::new(State::Unknown);
static LOADED: AtomicU64 = AtomicU64::new(0);
static STORED: AtomicU64 = AtomicU64::new(0);

fn folder() -> Option<&'static Path> {
    super::shader_cache::runtime_dir()
}

fn open(host: &ID3D12Device, dir: &Path) -> Option<Library> {
    let device = host.cast::<ID3D12Device1>().ok()?;
    let blob = std::fs::read(dir.join(LIBRARY_FILE)).unwrap_or_default();
    let opened = unsafe { device.CreatePipelineLibrary::<ID3D12PipelineLibrary1>(&blob) };
    let (library, blob) = match opened {
        Ok(library) => (library, blob),
        // Another adapter or driver, or a damaged file: start empty.
        Err(_) => (unsafe { device.CreatePipelineLibrary(&[]) }.ok()?, Vec::new()),
    };
    let entries = !blob.is_empty();
    crate::diagnostics::log_once(&std::sync::Once::new(), || {
        format!("pipeline library: {} ({} bytes)", if entries { "loaded" } else { "new" }, blob.len())
    });
    saver();
    Some(Library { library, _blob: blob, dirty: false })
}

/// Runs `f` on the library, opening it once the cache folder is known.
fn with_library<R>(host: &ID3D12Device, f: impl FnOnce(&mut Library) -> R) -> Option<R> {
    let mut state = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(*state, State::Unknown) {
        let dir = folder()?;
        *state = State::Ready(open(host, dir));
    }
    match &mut *state {
        State::Ready(Some(library)) => Some(f(library)),
        _ => None,
    }
}

fn load_or_make(
    host: &ID3D12Device,
    shader_bytes: u64,
    name: Option<String>,
    load: impl FnOnce(&ID3D12PipelineLibrary1, &HSTRING) -> Result<ID3D12PipelineState>,
    make: impl FnOnce() -> Result<ID3D12PipelineState>,
) -> Result<ID3D12PipelineState> {
    // Drivers that compile inside the call can recurse deeply (Proton).
    // five-star afdd699: the driver compile waits for the memory it may take.
    let make = || {
        let _admitted = super::compile_budget::enter(shader_bytes);
        crate::big_stack::run(make)
    };
    let Some(name) = name.map(|n| HSTRING::from(n.as_str())) else {
        return make();
    };
    if let Some(Ok(pipeline)) = with_library(host, |l| load(&l.library, &name)) {
        LOADED.fetch_add(1, Ordering::Relaxed);
        return Ok(pipeline);
    }
    // The driver compiles outside the lock.
    let made = make()?;
    with_library(host, |l| {
        // A name already present (another description) keeps its entry.
        if unsafe { l.library.StorePipeline(&name, &made) }.is_ok() {
            l.dirty = true;
            STORED.fetch_add(1, Ordering::Relaxed);
        }
    });
    Ok(made)
}

/// CreatePipelineState through the library.
pub(super) unsafe fn stream(
    host: &ID3D12Device2,
    desc: &D3D12_PIPELINE_STATE_STREAM_DESC,
) -> Result<ID3D12PipelineState> {
    let name = unsafe { super::stream::native_name(desc) };
    load_or_make(
        host,
        unsafe { super::stream::shader_bytes(desc) },
        name,
        |library, name| unsafe { library.LoadPipeline(name, desc) },
        || unsafe { host.CreatePipelineState(desc) },
    )
}

/// CreateComputePipelineState through the library.
pub(super) unsafe fn compute(
    host: &ID3D12Device,
    desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC,
) -> Result<ID3D12PipelineState> {
    let name = compute_name(desc);
    load_or_make(
        host,
        desc.CS.BytecodeLength as u64,
        name,
        |library, name| unsafe { library.LoadComputePipeline(name, desc) },
        || unsafe { host.CreateComputePipelineState(desc) },
    )
}

fn compute_name(desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC) -> Option<String> {
    let cs = &desc.CS;
    if cs.pShaderBytecode.is_null() || cs.BytecodeLength == 0 || cs.BytecodeLength > isize::MAX as usize {
        return None;
    }
    let code = unsafe { core::slice::from_raw_parts(cs.pShaderBytecode.cast::<u8>(), cs.BytecodeLength) };
    // Root signatures are left out, as for streams: the driver rejects a
    // loaded entry whose root differs, and the pipeline is then compiled.
    let canonical = format!(
        "host-compute:{};{};{}",
        super::shader_cache::source_digest(code)?,
        desc.NodeMask,
        desc.Flags.0
    );
    super::shader_cache::source_digest(canonical.as_bytes())
}

struct Used {
    /// Keys bound by earlier runs, or `None` before any run saved a list.
    before: Option<HashSet<String>>,
    /// Keys bound by this run that the saved list lacks.
    new: Vec<String>,
}

static USED: OnceLock<Mutex<Used>> = OnceLock::new();

fn used() -> Option<&'static Mutex<Used>> {
    if let Some(used) = USED.get() {
        return Some(used);
    }
    let dir = folder()?;
    Some(USED.get_or_init(|| {
        let before = std::fs::read_to_string(dir.join(USED_FILE))
            .ok()
            .map(|text| text.lines().map(str::to_owned).collect::<HashSet<_>>())
            .filter(|keys| !keys.is_empty());
        saver();
        Mutex::new(Used { before, new: Vec::new() })
    }))
}

/// Whether an earlier run bound the pipeline with this key; `None` without a list.
pub(super) fn used_before(key: &str) -> Option<bool> {
    let used = used()?.lock().unwrap_or_else(|e| e.into_inner());
    used.before.as_ref().map(|keys| keys.contains(key))
}

/// A command list bound the pipeline with this key for the first time.
pub(super) fn note_used(key: &str) {
    let Some(used) = used() else { return };
    let mut used = used.lock().unwrap_or_else(|e| e.into_inner());
    if !used.before.as_ref().is_some_and(|keys| keys.contains(key)) {
        used.new.push(key.to_owned());
    }
}

/// Publishes `bytes` at `path` through a staging file of its own (process
/// and a per-process counter, so two writers never share one), removed when
/// writing or publishing fails (VIctory 7d73e2b).
fn write_atomic(path: &Path, bytes: &[u8]) -> bool {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pending = path.with_extension(format!("{}.{n}.tmp", std::process::id()));
    let published = std::fs::write(&pending, bytes).and_then(|()| std::fs::rename(&pending, path));
    if published.is_err() {
        let _ = std::fs::remove_file(&pending);
    }
    published.is_ok()
}

#[cfg(test)]
mod staging_tests {
    /// A publication that fails (the destination is a directory) leaves no
    /// staging file behind; one that succeeds leaves only the result.
    #[test]
    fn failed_publication_removes_its_staging_file() {
        let dir = std::env::temp_dir().join(format!("tuxbox-pipeline-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("taken.bin")).unwrap();
        assert!(!super::write_atomic(&dir.join("taken.bin"), b"blob"));
        assert!(super::write_atomic(&dir.join("fresh.bin"), b"blob"));
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["fresh.bin", "taken.bin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn save(dir: &Path) {
    let blob = {
        let mut state = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *state {
            State::Ready(Some(library)) if library.dirty => {
                let mut blob = vec![0u8; unsafe { library.library.GetSerializedSize() }];
                let ok = unsafe { library.library.Serialize(&mut blob) }.is_ok();
                library.dirty = !ok;
                ok.then_some(blob)
            }
            _ => None,
        }
    };
    if let Some(blob) = blob {
        write_atomic(&dir.join(LIBRARY_FILE), &blob);
        let (loaded, stored) = (LOADED.load(Ordering::Relaxed), STORED.load(Ordering::Relaxed));
        crate::diagnostics::log_once(&std::sync::Once::new(), || {
            format!("pipeline library saved: {loaded} loaded, {stored} stored, {} bytes", blob.len())
        });
    }
    if let Some(used) = USED.get() {
        let mut used = used.lock().unwrap_or_else(|e| e.into_inner());
        if !used.new.is_empty() {
            let new = std::mem::take(&mut used.new);
            let keys = used.before.get_or_insert_with(HashSet::new);
            keys.extend(new);
            let mut text = String::new();
            for key in keys.iter() {
                text.push_str(key);
                text.push('\n');
            }
            drop(used);
            write_atomic(&dir.join(USED_FILE), text.as_bytes());
        }
    }
}

/// Saves the library and the used list now, if anything changed (at the end
/// of a run, as VIctory's `save_pipeline_cache`).
pub(crate) fn save_now() {
    if let Some(dir) = folder() {
        save(dir);
    }
}

/// Starts the saver thread once.
fn saver() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("tuxbox-pipeline-saver".into())
        .spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(20));
                if let Some(dir) = folder() {
                    save(dir);
                }
            }
        });
}
