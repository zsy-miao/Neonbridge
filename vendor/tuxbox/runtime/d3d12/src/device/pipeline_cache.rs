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
//! The mapped file's handle stays open while its view lives (VIctory-fxceless
//! 578d459, GPL-3.0, credit to its authors). With it closed, a save renaming
//! the new library over the mapped file makes Windows drop the old file's
//! contents: every page of the view not in memory then, or trimmed later,
//! reads as zeros, the driver loads zeroed pipelines and removes the device
//! with DXGI_ERROR_DRIVER_INTERNAL_ERROR (their runs 72/73/82; they measured
//! 16,352 of 16,384 pages of a replaced 64 MB file reading zero). test142 was
//! removed with that reason after five saves, at 165 MiB of free commit.
//! TUXBOX_PIPELINE_LIBRARY_HANDLE=0 closes it after mapping, as before.
//!
//! Stores and serialising never overlap (STORES). Loads only take LIBRARY
//! to clone the interface, so a save does not stall them. A saver thread
//! writes both files every 20 s when they changed.
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{HSTRING, Interface, Result};

const LIBRARY_FILE: &str = "pipelines.d3d12lib";
/// A saved library the system would not rename over LIBRARY_FILE, left
/// beside it; the next start moves it into place first.
const NEXT_FILE: &str = "pipelines.d3d12lib.next";
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
    /// The mapped file, open until the view is unmapped (module notes).
    file: Option<windows::Win32::Foundation::HANDLE>,
}

fn holds_file() -> bool {
    crate::env_toggle!("TUXBOX_PIPELINE_LIBRARY_HANDLE") != Some("0")
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
            (!view.Value.is_null()).then(|| View { at: view.Value.cast(), len, file: None })
        }).flatten();
        match view {
            Some(mut view) if holds_file() => {
                view.file = Some(file);
                Some(view)
            }
            view => {
                let _ = unsafe { CloseHandle(file) };
                view
            }
        }
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
        if let Some(file) = self.file.take() {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(file) };
        }
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
    let next = dir.join(NEXT_FILE);
    if next.exists() && std::fs::rename(&next, &path).is_err() {
        crate::diagnostics::log_line(format_args!(
            "pipeline library: the library saved last ({NEXT_FILE}) could not take the old one's place"));
    }
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
    if let Blob::File(view) = &blob {
        VIEW[0].store(view.at as usize, Ordering::Relaxed);
        VIEW[1].store(view.len, Ordering::Relaxed);
        LOADED_AT.store(now_ms(), Ordering::Relaxed);
    }
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

thread_local! {
    /// This thread builds the journal offline (`--pipelines`).
    static OFFLINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
static CAPPED: AtomicU64 = AtomicU64::new(0);

/// Marks this thread as the offline journal builder's.
pub(in crate::device) fn offline_thread() {
    OFFLINE.with(|o| o.set(true));
}

/// Offline builds skipped for their DXIL size so far.
pub(in crate::device) fn capped() -> u64 {
    CAPPED.load(Ordering::Relaxed)
}

/// xcl cbff0b8 + 6b9b14f (GPL-2.0-only, credit to their authors and
/// WebIdentity1): offline, a program over TUXBOX_PIPELINE_MAX_KB of DXIL
/// (8192; 0 no cap) that the library lacks is left to the runtime. Their
/// driver compiles take tens of minutes and gigabytes (29-30 MB programs hung
/// a precompile pass for 33+ minutes); 8 MiB sits above the largest that ever
/// finished (~2.2 MB) and below the smallest that hung (14.1 MB). Checked
/// after the library lookup, so a stale entry the driver refuses cannot
/// carry an over-cap program into the uncached compile.
fn over_offline_cap(shader_bytes: u64) -> bool {
    if !OFFLINE.with(|o| o.get()) {
        return false;
    }
    static KB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let kb = *KB.get_or_init(|| crate::env_toggle!("TUXBOX_PIPELINE_MAX_KB").and_then(|v| v.trim().parse().ok()).unwrap_or(8192));
    kb != 0 && shader_bytes > kb << 10
}

/// test177: in-game driver compiles of 1.5-5.5 MB of DXIL each took 5-7 GB of
/// private memory at peak, up to five at once (private 33 GB; the budget's
/// 30 s give-up let them all start). One of 1 MiB of DXIL or more now waits
/// for the one before it, with no give-up: title threads skip such compiles
/// (BIND_SKIP_HEAVY), so only background compiles queue. The queue depth is
/// logged. TUXBOX_HEAVY_COMPILE_SERIAL=0 runs them together again.
/// test178: run one at a time, 1.1-1.3 MB compiles of 22-45 s each held
/// 1901 pipelines back and a third of the dispatches were dropped for
/// minutes. One now starts beside those running while free commit covers the
/// peak of all of them (1 GiB per MB of DXIL, test177's 5-7 GB for 5.5 MB)
/// plus 2 GiB; the first always starts. TUXBOX_HEAVY_COMPILE_BUDGET=0 runs
/// them strictly one at a time again.
struct HeavyTurn(u64);

#[derive(Default)]
struct Heavy { running: u32, need: u64 }

static HEAVY: (std::sync::Mutex<Heavy>, std::sync::Condvar) = (std::sync::Mutex::new(Heavy { running: 0, need: 0 }), std::sync::Condvar::new());

impl Drop for HeavyTurn {
    fn drop(&mut self) {
        let mut h = HEAVY.0.lock().unwrap_or_else(|e| e.into_inner());
        h.running -= 1;
        h.need -= self.0;
        drop(h);
        HEAVY.1.notify_all();
    }
}

fn free_commit() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return 0;
    }
    status.ullAvailPageFile
}

fn heavy_turn(shader_bytes: u64) -> Option<HeavyTurn> {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static QUEUED: AtomicU64 = AtomicU64::new(0);
    static LOGGED: AtomicU64 = AtomicU64::new(0);
    if shader_bytes < 1 << 20 || crate::env_toggle!("TUXBOX_HEAVY_COMPILE_SERIAL") == Some("0") {
        return None;
    }
    let budget = crate::env_toggle!("TUXBOX_HEAVY_COMPILE_BUDGET") != Some("0");
    let need = shader_bytes << 10;
    let depth = QUEUED.fetch_add(1, Relaxed) + 1;
    let start = std::time::Instant::now();
    let (lock, ready) = &HEAVY;
    let mut h = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if h.running == 0 || (budget && free_commit() >= h.need + need + (2 << 30)) {
            break;
        }
        // Recheck memory now and then, not only when one finishes.
        h = ready.wait_timeout(h, std::time::Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner()).0;
    }
    h.running += 1;
    h.need += need;
    let others = h.running - 1;
    drop(h);
    QUEUED.fetch_sub(1, Relaxed);
    let n = LOGGED.fetch_add(1, Relaxed) + 1;
    if n <= 16 || n.is_power_of_two() || depth > 1 {
        crate::diagnostics::log_line(format_args!(
            "heavy compile: {} KiB of DXIL starts after {} ms in a queue of {depth}, beside {others} (free commit {} MiB; TUXBOX_HEAVY_COMPILE_BUDGET=0 one at a time; {n} so far; TUXBOX_HEAVY_COMPILE_SERIAL=0 off)",
            shader_bytes >> 10, start.elapsed().as_millis(), free_commit() >> 20));
    }
    Some(HeavyTurn(need))
}

fn load_or_make(
    host: &ID3D12Device,
    shader_bytes: u64,
    name: Option<String>,
    load: impl FnOnce(&ID3D12PipelineLibrary1, &HSTRING) -> Result<ID3D12PipelineState>,
    make: impl FnOnce() -> Result<ID3D12PipelineState>,
    fields: impl FnOnce() -> String,
) -> Result<ID3D12PipelineState> {
    // Drivers that compile inside the call can recurse deeply (Proton).
    // five-star afdd699: the driver compile waits for the memory it may take.
    let key = name.clone();
    let make = || {
        if super::stream::background::skip_heavy_on_title(shader_bytes) {
            return Err(windows::Win32::Foundation::E_ABORT.into());
        }
        let _heavy = heavy_turn(shader_bytes);
        let _admitted = super::compile_budget::enter(shader_bytes);
        let _measured = super::compile_budget::measure(shader_bytes, key.as_deref());
        if shader_bytes >= 256 << 10 && crate::big_stack::lane_enabled() {
            crate::big_stack::run_on_lane(make, !super::stream::background::prefetching() || super::stream::background::waited_on())
        } else {
            crate::big_stack::run(make)
        }
    };
    let Some(name) = name.map(|n| HSTRING::from(n.as_str())) else {
        if over_offline_cap(shader_bytes) {
            CAPPED.fetch_add(1, Ordering::Relaxed);
            return Err(windows::Win32::Foundation::E_ABORT.into());
        }
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
    if loaded.is_some() {
        // The load read the library's file (see `trim`).
        LOADED_AT.store(now_ms(), Ordering::Relaxed);
        TRIMMED.store(false, Ordering::Relaxed);
    }
    if let Some(Ok(pipeline)) = loaded {
        LOADED.fetch_add(1, Ordering::Relaxed);
        return Ok(pipeline);
    }
    if over_offline_cap(shader_bytes) {
        CAPPED.fetch_add(1, Ordering::Relaxed);
        return Err(windows::Win32::Foundation::E_ABORT.into());
    }
    if let Some(names) = names() {
        let names = names.lock().unwrap_or_else(|e| e.into_inner());
        let known = names.known.contains(&text);
        let n = if known { &MISSED_KNOWN } else { &MISSED_NEW }.fetch_add(1, Ordering::Relaxed) + 1;
        drop(names);
        // TUXBOX_PIPELINE_MISS_LOG (=0 off): the first 16 misses of each kind,
        // then powers of two, with the load's result and the name's inputs.
        if (n <= 16 || n.is_power_of_two()) && crate::env_toggle!("TUXBOX_PIPELINE_MISS_LOG") != Some("0") {
            let why = match &loaded {
                None => "no library".to_string(),
                Some(Err(e)) => format!("{:#x}", e.code().0),
                Some(Ok(_)) => "loaded".to_string(),
            };
            crate::diagnostics::log_line(format_args!(
                "pipeline library miss ({} #{n}): name {text}, load {why}, {shader_bytes} shader bytes; fields {}",
                if known { "named by an earlier run" } else { "new name" }, fields()));
        }
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
    // Stores ran one at a time and inside LIBRARY, so every library hit
    // waited out each StorePipeline. VIctory-fxceless (library.rs:556-612,
    // GPL-3.0, credit to its authors) stores under the read side of a lock
    // only the save writes, on the interface outside the library lock.
    // TUXBOX_PIPELINE_STORE_LOCKED=1 stores inside LIBRARY, one at a time.
    let stored = if crate::env_toggle!("TUXBOX_PIPELINE_STORE_LOCKED") == Some("1") {
        let _stores = STORES.write().unwrap_or_else(|e| e.into_inner());
        with_library(host, |l| {
            // A name already present (another description) keeps its entry.
            let ok = store_logged(unsafe { l.library.StorePipeline(&name, &made) }, &text);
            if ok {
                l.dirty = true;
            }
            ok
        })
        .unwrap_or(false)
    } else {
        let _stores = STORES.read().unwrap_or_else(|e| e.into_inner());
        match with_library(host, |l| l.library.clone()) {
            Some(library) => {
                let ok = store_logged(unsafe { library.StorePipeline(&name, &made) }, &text);
                if ok {
                    // Under STORES' read side: the save cannot clear this
                    // before it serialises the store.
                    with_library(host, |l| l.dirty = true);
                }
                ok
            }
            None => false,
        }
    };
    if stored {
        STORED.fetch_add(1, Ordering::Relaxed);
        if let Some(names) = names() {
            let mut names = names.lock().unwrap_or_else(|e| e.into_inner());
            if names.known.insert(text.clone()) {
                names.new.push(text.clone());
            }
        }
    }
    Ok(made)
}

/// A StorePipeline result; a failure is logged with its HRESULT and name
/// (first 16, then powers of two; TUXBOX_PIPELINE_MISS_LOG=0 off): it showed
/// only as "made but not in the library" (Coder2).
fn store_logged(result: Result<()>, name: &str) -> bool {
    let Err(error) = result else { return true };
    static FAILED: AtomicU64 = AtomicU64::new(0);
    let n = FAILED.fetch_add(1, Ordering::Relaxed) + 1;
    if (n <= 16 || n.is_power_of_two()) && crate::env_toggle!("TUXBOX_PIPELINE_MISS_LOG") != Some("0") {
        crate::diagnostics::log_line(format_args!(
            "pipeline library store failed (#{n}): name {name}, {:#x}", error.code().0));
    }
    false
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
        || unsafe { super::stream::native_fields(desc) },
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
        || {
            let cs = &desc.CS;
            let code = unsafe { core::slice::from_raw_parts(cs.pShaderBytecode.cast::<u8>(), cs.BytecodeLength) };
            format!("cs {} node {} flags {:#x}", super::shader_cache::source_digest(code).unwrap_or_default(), desc.NodeMask, desc.Flags.0)
        },
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
    let mut canonical = format!(
        "host-compute:{};{};{}",
        super::shader_cache::source_digest(code)?,
        desc.NodeMask,
        desc.Flags.0
    );
    if super::root_name::on() {
        let raw = desc.pRootSignature.as_ref().map_or(core::ptr::null_mut(), |r| r.as_raw());
        canonical.push_str(&format!(";root:{}", super::root_name::digest(raw).unwrap_or_default()));
    }
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
        super::stream::background::journal::warm();
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
    let published = ok && publish(&pending, path, true);
    if !published {
        let _ = std::fs::remove_file(&pending);
    }
    Some(published)
}

/// Renames `pending` over `path`. When the system refuses and `library`,
/// the new library is left beside the old one as NEXT_FILE for the next
/// start rather than lost (VIctory-fxceless library.rs:206-211, 668-670,
/// GPL-3.0, credit to its authors). TUXBOX_PIPELINE_LIBRARY_NEXT=0 drops
/// it as before.
fn publish(pending: &Path, path: &Path, library: bool) -> bool {
    if std::fs::rename(pending, path).is_ok() {
        return true;
    }
    if !library || crate::env_toggle!("TUXBOX_PIPELINE_LIBRARY_NEXT") == Some("0") {
        return false;
    }
    let next = path.with_file_name(NEXT_FILE);
    let kept = std::fs::rename(pending, &next).is_ok();
    crate::diagnostics::log_line(format_args!(
        "pipeline library: the save could not replace {}; {}", path.display(),
        if kept { "left beside it for the next start" } else { "dropped" }));
    kept
}

fn write_atomic(path: &Path, bytes: &[u8]) -> bool {
    write_publish(path, bytes, false)
}

fn write_publish(path: &Path, bytes: &[u8], library: bool) -> bool {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pending = path.with_extension(format!("{}.{n}.tmp", std::process::id()));
    let published = std::fs::write(&pending, bytes).is_ok() && publish(&pending, path, library);
    if !published {
        let _ = std::fs::remove_file(&pending);
    }
    published
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
/// (pipelines loaded from the library, pipelines stored into it) so far.
pub(in crate::device) fn library_counts() -> (u64, u64) {
    (LOADED.load(Ordering::Relaxed), STORED.load(Ordering::Relaxed))
}

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

/// Read by stores, written by a save across its Serialize: the library does
/// not promise that a store and Serialize may overlap. Loads take neither it nor LIBRARY past
/// a clone of the interface.
static STORES: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn save(dir: &Path) {
    super::stream::background::journal::flush();
    // The save held LIBRARY across Serialize, and every load takes LIBRARY
    // for a moment (with_library) to clone the interface: each 20 s save of
    // a GB-size library stalled every library hit for its length.
    // VIctory-fxceless (library.rs:557, 589-612, GPL-3.0, credit to its
    // authors) saves under a lock stores take and loads do not. Here a save
    // holds STORES across Serialize and LIBRARY only to read and clear the
    // dirty flag, so loads go on. TUXBOX_PIPELINE_SAVE_LOCKED=1 holds
    // LIBRARY throughout as before.
    let locked = crate::env_toggle!("TUXBOX_PIPELINE_SAVE_LOCKED") == Some("1");
    let _stores = STORES.write().unwrap_or_else(|e| e.into_inner());
    let mut held = Some(LIBRARY.lock().unwrap_or_else(|e| e.into_inner()));
    let library = match held.as_deref_mut() {
        Some(State::Ready(Some(library))) if library.dirty => library.library.clone(),
        _ => return save_lists(dir),
    };
    if !locked {
        held = None;
    }
    let size = unsafe { library.GetSerializedSize() };
    let path = dir.join(LIBRARY_FILE);
    let ok = if mapped() { serialize_mapped(&library, &path, size) } else { None }
        .unwrap_or_else(|| {
            let mut blob = vec![0u8; size];
            unsafe { library.Serialize(&mut blob) }.is_ok() && write_publish(&path, &blob, true)
        });
    if ok {
        // No store ran meanwhile (STORES), so nothing new is left unsaved.
        let mut state = held.unwrap_or_else(|| LIBRARY.lock().unwrap_or_else(|e| e.into_inner()));
        if let State::Ready(Some(library)) = &mut *state {
            library.dirty = false;
        }
    } else {
        drop(held);
    }
    let written = ok.then_some(size);
    if let Some(len) = written {
        let (loaded, stored) = (LOADED.load(Ordering::Relaxed), STORED.load(Ordering::Relaxed));
        let (new, known) = (MISSED_NEW.load(Ordering::Relaxed), MISSED_KNOWN.load(Ordering::Relaxed));
        let joined = JOINED.load(Ordering::Relaxed);
        crate::diagnostics::log_once(&std::sync::Once::new(), || {
            format!("pipeline library saved: {loaded} loaded, {stored} stored, {} bytes; misses: {new} named by no earlier run (the host description changed), {known} named by an earlier run (refused by the driver or lost with the library); {joined} waited for the same pipeline compiling on another thread (TUXBOX_PIPELINE_JOIN=0 compiles it again)", len)
        });
    }
    save_lists(dir);
}

/// Writes the stored names and the used list, when they changed.
fn save_lists(dir: &Path) {
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

/// The mapped library file's view (address, length), or zeros.
static VIEW: [std::sync::atomic::AtomicUsize; 2] =
    [std::sync::atomic::AtomicUsize::new(0), std::sync::atomic::AtomicUsize::new(0)];
static LOADED_AT: AtomicU64 = AtomicU64::new(0);
static TRIMMED: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// Lets the mapped library file's pages go from the process's working set
/// once loads have been quiet a while: each load reads entries out of the
/// view, and the pages it touched stayed resident (charged to the game's
/// working set) until Windows trimmed them. They stay in the file cache while
/// nothing else wants the memory and come back on the next read. The quiet
/// spell is 10 s, 5 s under 4 GiB free and 2 s under 2 GiB. Ported from
/// VIctory-fxceless library.rs:360-393 and host.rs:146-161 (GPL-3.0, credit
/// to its authors). TUXBOX_PIPELINE_LIBRARY_TRIM=0 leaves the pages alone.
fn trim() {
    const GIB: u64 = 1 << 30;
    let (at, len) = (VIEW[0].load(Ordering::Relaxed), VIEW[1].load(Ordering::Relaxed));
    if at == 0 || TRIMMED.load(Ordering::Relaxed) || crate::env_toggle!("TUXBOX_PIPELINE_LIBRARY_TRIM") == Some("0") {
        return;
    }
    let free = {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok().map(|()| status.ullAvailPhys)
    };
    let quiet_ms = match free {
        Some(free) if free < 2 * GIB => 2_000,
        Some(free) if free < 4 * GIB => 5_000,
        _ => 10_000,
    };
    if now_ms() < LOADED_AT.load(Ordering::Relaxed) + quiet_ms || TRIMMED.swap(true, Ordering::Relaxed) {
        return;
    }
    // The view lives as long as the library, which LIBRARY keeps for the
    // process's life. Unlocking pages that are not locked fails, as
    // expected, and takes them out of the working set: the documented way
    // to let pages of a mapped file go.
    let _ = unsafe { windows::Win32::System::Memory::VirtualUnlock(at as *const _, len) };
    crate::diagnostics::log_once(&std::sync::Once::new(), || {
        format!("pipeline library: the mapped file's pages let go from memory ({len} bytes; TUXBOX_PIPELINE_LIBRARY_TRIM=0 keeps them)")
    });
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
            for tick in 1u64.. {
                std::thread::sleep(Duration::from_secs(1));
                trim();
                if tick % 20 == 0
                    && let Some(dir) = folder()
                {
                    save(dir);
                }
            }
        });
}
