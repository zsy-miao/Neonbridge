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
//! test139: both files lived in the translator build's folder, so every
//! rebuild started with an empty list and library: the 84,186 pipelines the
//! title created were all lazy, and the cutscene's first binds compiled about
//! 1,200 of them in the driver on two workers while every frame waited. The
//! list's keys describe the guest pipeline, and library entries are named by
//! the full host description (translated DXIL included) and checked by the
//! driver, so neither depends on the build: both now live beside the build
//! folders (`runtime/`), shared by every build. The first shared list is the
//! union of the builds' lists. TUXBOX_PIPELINE_CACHE_SHARED=0 keeps them per
//! build; a library over TUXBOX_PIPELINE_LIBRARY_MB (16384) starts afresh.
//!
//! The library reads its entries from the loaded blob for its whole life,
//! and serialising needs a buffer the library's size. Both used to be heap
//! copies, so a library cost twice its size in private bytes and commit
//! charge, and the cap had to stay at 256 MiB (about 2.7k of test140's 80k
//! pipelines). Both are now views of the file itself: the loaded file is
//! mapped read-only (opened with delete sharing, so a save can still rename
//! over it while the old view lives on), and a save serialises straight into
//! a mapped staging file. File-backed pages take no commit charge and the
//! system may drop them under pressure. TUXBOX_PIPELINE_LIBRARY_MAPPED=0
//! reads and serialises through the heap as before.
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
/// Names of every pipeline stored in the library, for the miss split below.
const NAMES_FILE: &str = "pipeline-names.txt";

struct Library {
    library: ID3D12PipelineLibrary1,
    /// The library reads its entries from this blob for its whole life.
    _blob: Blob,
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

fn mapped() -> bool {
    crate::env_toggle!("TUXBOX_PIPELINE_LIBRARY_MAPPED").as_deref() != Some("0")
}

/// A view of a whole file; unmapped when dropped.
struct View {
    at: *mut u8,
    len: usize,
}

impl View {
    /// Maps `path` read-only, or (with `len`) creates it that size and maps
    /// it writable.
    fn map(path: &Path, len: Option<usize>) -> Option<View> {
        use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
        use windows::Win32::Storage::FileSystem::*;
        use windows::Win32::System::Memory::*;
        let write = len.is_some();
        let wide = HSTRING::from(path.as_os_str());
        let access = if write { GENERIC_READ.0 | GENERIC_WRITE.0 } else { GENERIC_READ.0 };
        let share = FILE_SHARE_READ | FILE_SHARE_DELETE;
        let disposition = if write { CREATE_ALWAYS } else { OPEN_EXISTING };
        let file = unsafe { CreateFileW(&wide, access, share, None, disposition, FILE_ATTRIBUTE_NORMAL, None) }.ok()?;
        let len = len.unwrap_or_else(|| {
            let mut size = 0i64;
            let _ = unsafe { GetFileSizeEx(file, &mut size) };
            size as usize
        });
        let view = (len != 0).then(|| unsafe {
            let protect = if write { PAGE_READWRITE } else { PAGE_READONLY };
            let mapping = CreateFileMappingW(file, None, protect, (len as u64 >> 32) as u32, len as u32, None).ok()?;
            let view = MapViewOfFile(mapping, if write { FILE_MAP_WRITE } else { FILE_MAP_READ }, 0, 0, len);
            // The view keeps the section, and the section the file.
            let _ = CloseHandle(mapping);
            (!view.Value.is_null()).then(|| View { at: view.Value.cast(), len })
        }).flatten();
        let _ = unsafe { CloseHandle(file) };
        view
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.at, self.len) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.at, self.len) }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        use windows::Win32::System::Memory::*;
        let _ = unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.at.cast() }) };
    }
}

/// The blob a library was opened from.
enum Blob {
    Heap(Vec<u8>),
    File(View),
}

impl Blob {
    fn bytes(&self) -> &[u8] {
        match self {
            Blob::Heap(bytes) => bytes,
            Blob::File(view) => view.bytes(),
        }
    }
}
static LOADED: AtomicU64 = AtomicU64::new(0);
static STORED: AtomicU64 = AtomicU64::new(0);
/// Library misses whose name no run ever stored (the host description
/// changed between runs: a translation or record that differs) and misses
/// whose name an earlier run stored (the driver refused the description, or
/// the library holding it was started afresh).
static MISSED_NEW: AtomicU64 = AtomicU64::new(0);
/// Library misses that waited for the same name's compile already running
/// on another thread instead of compiling it again.
static JOINED: AtomicU64 = AtomicU64::new(0);

/// A driver compile of one library name in progress; threads that miss the
/// same name meanwhile wait for its result (xcl 04f34ce, GPL-3.0, idea:
/// duplicate builds coalesced). The title creates many pipelines whose host
/// descriptions are identical, and the pipeline workers, the title's own
/// threads and prefetch reached the driver with them at once, each paying
/// the whole compile. TUXBOX_PIPELINE_JOIN=0 compiles each again.
struct Flight {
    done: Mutex<Option<Result<ID3D12PipelineState>>>,
    ready: std::sync::Condvar,
}

// SAFETY: D3D12 pipeline states are free-threaded; the result is only
// cloned (AddRef) out of the mutex.
unsafe impl Send for Flight {}
unsafe impl Sync for Flight {}

static FLIGHTS: Mutex<Option<std::collections::HashMap<String, std::sync::Arc<Flight>>>> = Mutex::new(None);

/// Library names one thread at a time loads or stores (VIctory b00c14a,
/// GPL-3.0, credit to its authors). The library is free-threaded but for one
/// name used on several threads at once (ID3D12Device1::CreatePipelineLibrary,
/// "Thread Safety"), and a load is not always a lookup: what the driver's
/// own cache lacks is compiled inside the load, for up to seconds. Loads held
/// the library's lock, so every other pipeline thread waited for the one
/// loading. Loads of different names now overlap; stores and serialising
/// keep the lock. TUXBOX_PIPELINE_PARALLEL_LOAD=0 loads under the lock again.
struct Held(String);

static HELD: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static RELEASED: std::sync::Condvar = std::sync::Condvar::new();

impl Held {
    /// Holds `name`, once no other thread does.
    fn new(name: &str) -> Held {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        while held.as_ref().is_some_and(|h| h.contains(name)) {
            held = RELEASED.wait(held).unwrap_or_else(|e| e.into_inner());
        }
        held.get_or_insert_default().insert(name.to_owned());
        Held(name.to_owned())
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(held) = HELD.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            held.remove(&self.0);
        }
        RELEASED.notify_all();
    }
}

fn parallel_loads() -> bool {
    crate::env_toggle!("TUXBOX_PIPELINE_PARALLEL_LOAD") != Some("0")
}
static MISSED_KNOWN: AtomicU64 = AtomicU64::new(0);

struct Names {
    known: HashSet<String>,
    new: Vec<String>,
}

static NAMES: OnceLock<Option<Mutex<Names>>> = OnceLock::new();

fn names() -> Option<&'static Mutex<Names>> {
    NAMES
        .get_or_init(|| {
            let dir = folder()?;
            let known = std::fs::read_to_string(dir.join(NAMES_FILE))
                .map(|text| text.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default();
            Some(Mutex::new(Names { known, new: Vec::new() }))
        })
        .as_ref()
}

pub(in crate::device) fn folder() -> Option<&'static Path> {
    let build = super::shader_cache::runtime_dir()?;
    if crate::env_toggle!("TUXBOX_PIPELINE_CACHE_SHARED") == Some("0") {
        return Some(build);
    }
    build.parent().or(Some(build))
}

/// The union of the per-build lists earlier builds saved (the first shared list).
fn build_lists(shared: &Path) -> HashSet<String> {
    let mut keys = HashSet::new();
    for entry in std::fs::read_dir(shared).into_iter().flatten().flatten() {
        if let Ok(text) = std::fs::read_to_string(entry.path().join(USED_FILE)) {
            keys.extend(text.lines().filter(|l| !l.is_empty()).map(str::to_owned));
        }
    }
    keys
}

fn open(host: &ID3D12Device, dir: &Path) -> Option<Library> {
    let device = host.cast::<ID3D12Device1>().ok()?;
    let cap = library_cap_mb();
    let path = dir.join(LIBRARY_FILE);
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    let blob = if cap != 0 && size > cap << 20 {
        crate::diagnostics::log_line(format_args!(
            "pipeline library: {} MiB is over TUXBOX_PIPELINE_LIBRARY_MB={cap}; starting afresh", size >> 20));
        Blob::Heap(Vec::new())
    } else if size != 0 && mapped() {
        match View::map(&path, None) {
            Some(view) => Blob::File(view),
            None => Blob::Heap(std::fs::read(&path).unwrap_or_default()),
        }
    } else {
        Blob::Heap(std::fs::read(&path).unwrap_or_default())
    };
    let opened = unsafe { device.CreatePipelineLibrary::<ID3D12PipelineLibrary1>(blob.bytes()) };
    let (library, blob) = match opened {
        Ok(library) => (library, blob),
        // Another adapter or driver, or a damaged file: start empty.
        Err(_) => (unsafe { device.CreatePipelineLibrary(&[]) }.ok()?, Blob::Heap(Vec::new())),
    };
    let (len, file) = (blob.bytes().len(), matches!(blob, Blob::File(_)));
    crate::diagnostics::log_once(&std::sync::Once::new(), || {
        format!("pipeline library: {} ({len} bytes{})", if len != 0 { "loaded" } else { "new" },
            if file { ", mapped from the file" } else { "" })
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
        let _measured = super::compile_budget::measure(shader_bytes);
        crate::big_stack::run(make)
    };
    let Some(name) = name.map(|n| HSTRING::from(n.as_str())) else {
        return make();
    };
    let text = name.to_string();
    let loaded = if parallel_loads() {
        // Only the interface under the lock. The library (and the blob its
        // entries live in) stays in LIBRARY for the process's life.
        let library = with_library(host, |l| l.library.clone());
        library.map(|library| {
            let _held = Held::new(&text);
            load(&library, &name)
        })
    } else {
        with_library(host, |l| load(&l.library, &name))
    };
    if let Some(Ok(pipeline)) = loaded {
        LOADED.fetch_add(1, Ordering::Relaxed);
        return Ok(pipeline);
    }
    if let Some(names) = names() {
        let names = names.lock().unwrap_or_else(|e| e.into_inner());
        if names.known.contains(&text) { &MISSED_KNOWN } else { &MISSED_NEW }.fetch_add(1, Ordering::Relaxed);
    }
    let join = crate::env_toggle!("TUXBOX_PIPELINE_JOIN") != Some("0");
    let flight = if join {
        let mut guard = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
        let flights = guard.get_or_insert_with(Default::default);
        if let Some(running) = flights.get(&text) {
            let running = running.clone();
            // Let go of the map before waiting: other names' misses need it.
            drop(guard);
            JOINED.fetch_add(1, Ordering::Relaxed);
            let mut done = running.done.lock().unwrap_or_else(|e| e.into_inner());
            while done.is_none() {
                done = running.ready.wait(done).unwrap_or_else(|e| e.into_inner());
            }
            return done.clone().expect("a finished flight holds its result");
        }
        let flight = std::sync::Arc::new(Flight { done: Mutex::new(None), ready: std::sync::Condvar::new() });
        flights.insert(text.clone(), flight.clone());
        Some(flight)
    } else {
        None
    };
    // Whatever happens below, waiters get a result and the name leaves the map.
    struct Land<'a>(Option<std::sync::Arc<Flight>>, &'a str, Option<Result<ID3D12PipelineState>>);
    impl Drop for Land<'_> {
        fn drop(&mut self) {
            let Some(flight) = self.0.take() else { return };
            let result = self.2.take().unwrap_or_else(|| Err(windows::Win32::Foundation::E_FAIL.into()));
            *flight.done.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            flight.ready.notify_all();
            if let Some(flights) = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                flights.remove(self.1);
            }
        }
    }
    let mut land = Land(flight, &text, None);
    // The driver compiles outside the lock.
    let made = make();
    land.2 = Some(made.clone());
    let made = made?;
    let _held = parallel_loads().then(|| Held::new(&text));
    with_library(host, |l| {
        // A name already present (another description) keeps its entry.
        if unsafe { l.library.StorePipeline(&name, &made) }.is_ok() {
            l.dirty = true;
            STORED.fetch_add(1, Ordering::Relaxed);
            if let Some(names) = names() {
                let mut names = names.lock().unwrap_or_else(|e| e.into_inner());
                if names.known.insert(text.clone()) {
                    names.new.push(text.clone());
                }
            }
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
        let shared = super::shader_cache::runtime_dir().is_some_and(|build| build != dir);
        let before = std::fs::read_to_string(dir.join(USED_FILE))
            .ok()
            .map(|text| text.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect::<HashSet<_>>())
            .or_else(|| shared.then(|| build_lists(dir)))
            .filter(|keys| !keys.is_empty());
        let known = before.as_ref().map_or(0, HashSet::len);
        crate::diagnostics::log_line(format_args!(
            "pipeline precache: {known} pipelines earlier runs bound are built as the title creates them ({}; TUXBOX_PIPELINE_CACHE_SHARED=0 per build)",
            if shared { "list shared by every build" } else { "this build's list" }));
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
/// Serialises the library into a mapped staging file and renames it over
/// `path`; None when the staging file could not be mapped (the caller then
/// goes through the heap).
fn serialize_mapped(library: &ID3D12PipelineLibrary1, path: &Path, size: usize) -> Option<bool> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let pending = path.with_extension(format!("{}.{n}.map.tmp", std::process::id()));
    let Some(mut view) = View::map(&pending, Some(size)) else {
        let _ = std::fs::remove_file(&pending);
        return None;
    };
    let ok = unsafe { library.Serialize(view.bytes_mut()) }.is_ok();
    drop(view);
    let published = ok && std::fs::rename(&pending, path).is_ok();
    if !published {
        let _ = std::fs::remove_file(&pending);
    }
    Some(published)
}

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

/// TUXBOX_PIPELINE_LIBRARY_MB (16384; 0 no cap).
pub(in crate::device) fn library_cap_mb() -> u64 {
    crate::env_toggle!("TUXBOX_PIPELINE_LIBRARY_MB").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(16384)
}

/// The library's serialized size now (the pipeline builder's cap).
pub(in crate::device) fn library_bytes() -> u64 {
    let state = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    match &*state {
        State::Ready(Some(library)) => (unsafe { library.library.GetSerializedSize() }) as u64,
        _ => 0,
    }
}

/// Builds the pipeline journal's pipelines into the library outside the
/// title (see `stream::background::journal`); the pipelines made.
pub(crate) fn build_journal(threads: u32) -> u32 {
    super::stream::background::journal::build(threads)
}

fn save(dir: &Path) {
    super::stream::background::journal::flush();
    let written = {
        let mut state = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *state {
            State::Ready(Some(library)) if library.dirty => {
                let size = unsafe { library.library.GetSerializedSize() };
                let path = dir.join(LIBRARY_FILE);
                let ok = if mapped() { serialize_mapped(&library.library, &path, size) } else { None }
                    .unwrap_or_else(|| {
                        let mut blob = vec![0u8; size];
                        unsafe { library.library.Serialize(&mut blob) }.is_ok() && write_atomic(&path, &blob)
                    });
                library.dirty = !ok;
                ok.then_some(size)
            }
            _ => None,
        }
    };
    if let Some(len) = written {
        let (loaded, stored) = (LOADED.load(Ordering::Relaxed), STORED.load(Ordering::Relaxed));
        let (new, known) = (MISSED_NEW.load(Ordering::Relaxed), MISSED_KNOWN.load(Ordering::Relaxed));
        let joined = JOINED.load(Ordering::Relaxed);
        crate::diagnostics::log_once(&std::sync::Once::new(), || {
            format!("pipeline library saved: {loaded} loaded, {stored} stored, {} bytes; misses: {new} named by no earlier run (the host description changed), {known} named by an earlier run (refused by the driver or lost with the library); {joined} waited for the same pipeline compiling on another thread (TUXBOX_PIPELINE_JOIN=0 compiles it again)", len)
        });
    }
    if let Some(names) = names() {
        let mut names = names.lock().unwrap_or_else(|e| e.into_inner());
        if !names.new.is_empty() {
            names.new.clear();
            let mut text = String::with_capacity(names.known.len() * 65);
            for name in &names.known {
                text.push_str(name);
                text.push('\n');
            }
            drop(names);
            write_atomic(&dir.join(NAMES_FILE), text.as_bytes());
        }
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
