//! Pipelines whose translation missed the title's wait budget are rebuilt by
//! workers (VIctory's "pipelines made off the title's threads"). The title gets
//! a null pipeline at once; its draws are dropped only until a worker, which
//! waits on the translator without a budget, has made the real host pipeline
//! and published it in the proxy's upgrade slot. Command lists read that slot
//! whenever they bind the pipeline, so the render thread never waits on the
//! translator or on the driver's compile.
//!
//! Pipelines no earlier run bound are lazy (VIctory makes every host pipeline
//! on first use: the title creates some 60,000 in its first 20 s and draws
//! with a few dozen). Their proxies hold a shared placeholder; the first bind
//! queues them and waits within the frame's budget, and command lists drop
//! draws and dispatches until they exist.
use super::super::compute::{Pipeline, create_pipeline_x};
use super::super::Device;
use super::{AMPLIFICATION_X, MESH_X, STENCIL_X, layout};
use core::{ffi::c_void, mem::MaybeUninit, ptr, slice};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Interface, PCSTR};

/// The pipeline a worker made for one created without its translation: an
/// ordinary pipeline proxy of the same device, owned by the slot.
pub(crate) struct Built(IUnknown);

impl Built {
    pub(crate) fn pipeline(&self) -> &Pipeline {
        // SAFETY: made by create_pipeline or create_pipeline_x, retained by self.
        unsafe { &*self.0.as_raw().cast::<Pipeline>() }
    }
}

/// Written once by a worker, then only read by command lists.
#[derive(Default)]
pub(crate) struct Upgrade {
    built: OnceLock<Built>,
    /// A lazy pipeline's description until its first bind hands it to the workers.
    deferred: Mutex<Option<(IUnknown, Work)>>,
    /// A worker found no translation for it: it stays a null pipeline.
    failed: std::sync::atomic::AtomicBool,
    /// The thread id of the worker building it, 0 when none is.
    running: std::sync::atomic::AtomicU32,
}

// SAFETY: D3D12 objects are free-threaded. The built state is published once
// through the OnceLock and is immutable afterwards.
unsafe impl Send for Upgrade {}
// SAFETY: as above.
unsafe impl Sync for Upgrade {}

impl Upgrade {
    pub(crate) fn get(&self) -> Option<&Built> {
        self.built.get()
    }
}

/// Signalled whenever a worker finishes a pipeline.
static DONE: (Mutex<()>, std::sync::Condvar) = (Mutex::new(()), std::sync::Condvar::new());

/// The pipeline to bind for a command list: a lazy pipeline is handed to
/// the workers on its first bind, and waited for while the title's frame
/// has translator budget left (VIctory's `make::ready`). `None` while it is
/// still being made: the list drops its draws and dispatches.
/// An upgrade that is never built: a pipeline holding it stays the shared
/// placeholder and command lists drop its draws and dispatches.
pub(crate) fn never() -> Arc<Upgrade> {
    let slot = Upgrade { failed: std::sync::atomic::AtomicBool::new(true), ..Default::default() };
    Arc::new(slot)
}

/// Its worker could not make it: it stays the placeholder.
pub(crate) fn failed(slot: &Upgrade) -> bool {
    slot.built.get().is_none() && slot.failed.load(Ordering::Relaxed)
}

pub(crate) fn ready(slot: &Arc<Upgrade>) -> Option<&Built> {
    if let Some(built) = slot.built.get() {
        return Some(built);
    }
    let deferred = slot.deferred.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((device, work)) = deferred {
        let job = Job { device, work, slot: slot.clone() };
        match small(&job) && first_use::admit() {
            true => {
                let started = std::time::Instant::now();
                inline(job);
                first_use::spend(started.elapsed());
            }
            false => send(job, true),
        }
        if let Some(built) = slot.built.get() {
            return Some(built);
        }
    } else {
        promote(slot);
        hurry(slot);
    }
    let _cost = crate::diagnostics::cost::timer(crate::diagnostics::cost::Bucket::PipelineWait);
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while slot.built.get().is_none() && !slot.failed.load(Ordering::Relaxed) {
        let step = std::time::Duration::from_millis(5);
        if !crate::device::shader_cache::spend_wait(step) {
            break;
        }
        held = done.wait_timeout(held, step).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    slot.built.get()
}

/// Waits up to `limit` for a lazy pipeline's worker outside the frame's
/// translator budget (a compute pipeline whose dispatches must not be lost).
pub(crate) fn wait_for(slot: &Arc<Upgrade>, limit: std::time::Duration) -> Option<&Built> {
    if let Some(built) = ready(slot) {
        return Some(built);
    }
    promote(slot);
    if steal_on() && let Some(job) = steal(slot) {
        build_here(job, limit);
        if let Some(built) = slot.built.get() {
            return Some(built);
        }
    }
    hurry(slot);
    // The worker building it gets its translation served first.
    let worker = slot.running.load(Ordering::Acquire);
    if worker != 0 {
        crate::device::shader_cache::urge_worker(worker, true);
    }
    let deadline = std::time::Instant::now() + limit;
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while slot.built.get().is_none() && !slot.failed.load(Ordering::Relaxed) {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let step = (deadline - now).min(std::time::Duration::from_millis(5));
        held = done.wait_timeout(held, step).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    if worker != 0 {
        crate::device::shader_cache::urge_worker(worker, false);
    }
    slot.built.get()
}

/// five-star-recomp 8fa5672 (GPL-3.0): a small compute pipeline (a culling
/// or clear initialiser) is made on the title's thread at its first bind
/// instead of losing to a frame budget already spent. Its translator wait still
/// counts against that budget; one that is not made goes to the workers.
/// TUXBOX_SMALL_COMPUTE_BYTES (16384; 0 off) sets the shader size limit.
fn small(job: &Job) -> bool {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    let limit = *LIMIT.get_or_init(|| std::env::var("TUXBOX_SMALL_COMPUTE_BYTES").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(16384));
    !on_worker() && matches!(&job.work, Work::Compute(c) if !c.code.is_empty() && c.code.len() <= limit)
}

/// five-star-recomp ab94e3c (GPL-3.0, FirstUseBudget.cs): the time the
/// title's threads spend making pipelines at their first bind is shared, a
/// bucket that refills a quarter second each second up to one second. A
/// loading frame meeting hundreds of new pipelines at 0.25-0.5 s each ran
/// past the title's hang watchdog. A first bind with the bucket empty, or
/// while a compile waits for memory in the compile budget, goes to the
/// workers. TUXBOX_FIRST_USE_BUDGET=0 off.
mod first_use {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    const REFILL_PER_SECOND: f64 = 0.25;
    const CAPACITY: f64 = 1.0;

    /// (seconds left, when last refilled)
    static BUCKET: Mutex<Option<(f64, Instant)>> = Mutex::new(None);
    static OVER: AtomicU64 = AtomicU64::new(0);

    fn enabled() -> bool {
        crate::env_toggle!("TUXBOX_FIRST_USE_BUDGET") != Some("0")
    }

    fn refill(bucket: &mut Option<(f64, Instant)>) -> &mut (f64, Instant) {
        let now = Instant::now();
        let (left, at) = bucket.get_or_insert((CAPACITY, now));
        *left = (*left + now.duration_since(*at).as_secs_f64() * REFILL_PER_SECOND).min(CAPACITY);
        *at = now;
        bucket.as_mut().unwrap()
    }

    pub(super) fn admit() -> bool {
        if !enabled() {
            return true;
        }
        let left = refill(&mut BUCKET.lock().unwrap_or_else(|e| e.into_inner())).0;
        let memory = crate::device::compile_budget::waiting();
        if left > 0.0 && !memory {
            return true;
        }
        let n = OVER.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a small compute pipeline's first bind went to the workers ({}; {n} so far; TUXBOX_FIRST_USE_BUDGET=0 off)",
                if memory { "a compile waits for memory" } else { "first-use budget spent" }));
        }
        false
    }

    pub(super) fn spend(took: Duration) {
        if enabled() {
            refill(&mut BUCKET.lock().unwrap_or_else(|e| e.into_inner())).0 -= took.as_secs_f64();
        }
    }
}

fn inline(mut job: Job) {
    static INLINE: AtomicU64 = AtomicU64::new(0);
    static REQUEUED: AtomicU64 = AtomicU64::new(0);
    let made = crate::big_stack::run(|| {
        WORKER.with(|w| w.set(true));
        NULLED.with(|n| n.set(false));
        // SAFETY: a device proxy begins with its IUnknown identity, retained by the job.
        let device = unsafe { &*job.device.as_raw().cast::<Device>() };
        let iid = ID3D12PipelineState::IID;
        let mut out = ptr::null_mut();
        let Work::Compute(compute) = &mut job.work else { return None };
        let desc = compute.desc();
        let status = unsafe { create_pipeline_x(device, &desc, 0, ptr::null(), &iid, &mut out) };
        WORKER.with(|w| w.set(false));
        let nulled = NULLED.with(|n| n.replace(false));
        let made = (status.is_ok() && !out.is_null()).then(|| unsafe { IUnknown::from_raw(out) });
        made.filter(|_| !nulled)
    });
    match made {
        Some(proxy) => {
            let _ = job.slot.built.set(Built(proxy));
            let n = INLINE.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "background pipelines: a small compute pipeline made at its first bind on the title's thread ({n} so far, {} sent on to the workers; TUXBOX_SMALL_COMPUTE_BYTES=0 off)",
                    REQUEUED.load(Ordering::Relaxed)));
            }
        }
        None => {
            REQUEUED.fetch_add(1, Ordering::Relaxed);
            send(job, true);
        }
    }
}

/// xcl's idea (re-derived): a pipeline the title is waiting on that a worker
/// is already building gets that worker raised to normal priority; the
/// worker drops back after the build. TUXBOX_PIPELINE_HURRY=0 off.
fn hurry(slot: &Upgrade) {
    let id = slot.running.load(Ordering::Acquire);
    if id == 0 || slot.built.get().is_some() || crate::env_toggle!("TUXBOX_PIPELINE_HURRY") == Some("0") {
        return;
    }
    use windows::Win32::System::Threading::*;
    let Ok(thread) = (unsafe { OpenThread(THREAD_SET_LIMITED_INFORMATION, false, id) }) else { return };
    let raised = unsafe { SetThreadPriority(thread, THREAD_PRIORITY_NORMAL) }.is_ok();
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(thread) };
    static HURRIED: AtomicU64 = AtomicU64::new(0);
    if raised {
        let n = HURRIED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a worker building a pipeline the title waits on raised to normal priority ({n} so far; TUXBOX_PIPELINE_HURRY=0 off)"));
        }
    }
}

/// A pipeline the title created but no command list bound yet. Its proxy
/// holds a shared placeholder; nothing is translated or compiled until a
/// command list binds it.
pub(in crate::device) fn lazy(device: IUnknown, work: Work) -> Arc<Upgrade> {
    let slot = Arc::new(Upgrade::default());
    *slot.deferred.lock().unwrap_or_else(|e| e.into_inner()) = Some((device, work));
    LAZY.fetch_add(1, Ordering::Relaxed);
    slot
}

static LAZY: AtomicU64 = AtomicU64::new(0);

struct Placeholder(usize, ID3D12PipelineState);
// SAFETY: D3D12 objects are free-threaded; it is never mutated.
unsafe impl Send for Placeholder {}
// SAFETY: as above.
unsafe impl Sync for Placeholder {}

/// One resource-free compute pipeline on an empty root signature, shared by
/// every lazy pipeline of the (first) device until it is made.
pub(in crate::device) fn placeholder(device: &Device) -> Option<ID3D12PipelineState> {
    static PLACEHOLDER: OnceLock<Option<Placeholder>> = OnceLock::new();
    let owner = ptr::from_ref(device) as usize;
    let made = PLACEHOLDER.get_or_init(|| {
        let code = super::super::fallback::shader(super::super::fallback::Stage::Compute)?;
        let mut blob = None;
        let desc = D3D12_ROOT_SIGNATURE_DESC::default();
        unsafe { D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None) }.ok()?;
        let blob = blob?;
        let bytes = unsafe { slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) };
        let root: ID3D12RootSignature = unsafe { device.native.CreateRootSignature(0, bytes) }.ok()?;
        let compute = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: core::mem::ManuallyDrop::new(Some(root)),
            CS: super::super::fallback::bytecode(code),
            ..Default::default()
        };
        let mut compute = compute;
        let pipeline = unsafe { device.native.CreateComputePipelineState(&compute) };
        unsafe { core::mem::ManuallyDrop::drop(&mut compute.pRootSignature) };
        Some(Placeholder(owner, pipeline.ok()?))
    });
    made.as_ref().filter(|p| p.0 == owner).map(|p| p.1.clone())
}

/// A self-owned deep copy of a guest pipeline stream. Guest pointers are only
/// valid during CreatePipelineState, so every referenced array, string and
/// shader is copied, and the guest root signature is retained.
pub(in crate::device) struct OwnedStream {
    storage: Vec<MaybeUninit<usize>>,
    size: usize,
    _bytes: Vec<Vec<u8>>,
    _names: Vec<CString>,
    _elements: Vec<Vec<D3D12_INPUT_ELEMENT_DESC>>,
    _declarations: Vec<Vec<D3D12_SO_DECLARATION_ENTRY>>,
    _strides: Vec<Vec<u32>>,
    _views: Vec<Vec<D3D12_VIEW_INSTANCE_LOCATION>>,
    _root: Option<IUnknown>,
}

unsafe fn copy_array<T: Copy>(data: *const T, count: usize) -> Option<Vec<T>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if data.is_null() || count > 4096 {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(data, count) }.to_vec())
}

unsafe fn copy_name(names: &mut Vec<CString>, name: PCSTR) -> Option<PCSTR> {
    if name.is_null() {
        return Some(name);
    }
    let owned = unsafe { CStr::from_ptr(name.0.cast()) }.to_owned();
    let pointer = PCSTR(owned.as_ptr().cast());
    names.push(owned);
    Some(pointer)
}

impl OwnedStream {
    /// Copies a stream that `create` already validated; `None` for anything
    /// it cannot own (unknown or repeated subobjects).
    pub(in crate::device) unsafe fn copy(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<Self> {
        let size = desc.SizeInBytes;
        let input = desc.pPipelineStateSubobjectStream.cast::<u8>();
        if input.is_null() || size == 0 || size > 4096 || !size.is_multiple_of(size_of::<usize>()) {
            return None;
        }
        let mut storage = vec![MaybeUninit::<usize>::uninit(); size / size_of::<usize>()];
        let output = storage.as_mut_ptr().cast::<u8>();
        unsafe { ptr::copy_nonoverlapping(input, output, size) };
        let mut owned = OwnedStream {
            storage: Vec::new(),
            size,
            _bytes: Vec::new(),
            _names: Vec::new(),
            _elements: Vec::new(),
            _declarations: Vec::new(),
            _strides: Vec::new(),
            _views: Vec::new(),
            _root: None,
        };
        let mut seen = 0u32;
        let mut at = 0;
        while at < size {
            let record = unsafe { output.add(at) };
            let tag = unsafe { record.cast::<i32>().read() };
            let (offset, bytes) = layout(tag)?;
            if bytes > size - at || seen & (1 << tag) != 0 {
                return None;
            }
            seen |= 1 << tag;
            let payload = unsafe { record.add(offset) };
            match tag {
                0 => {
                    let guest = unsafe { payload.cast::<*mut c_void>().read() };
                    // SAFETY: create() verified a live guest root (or null).
                    owned._root = unsafe { IUnknown::from_raw_borrowed(&guest) }.cloned();
                }
                1..=6 | AMPLIFICATION_X | MESH_X => {
                    let shader = unsafe { &mut *payload.cast::<D3D12_SHADER_BYTECODE>() };
                    if shader.BytecodeLength != 0 {
                        if shader.pShaderBytecode.is_null() || shader.BytecodeLength > isize::MAX as usize {
                            return None;
                        }
                        let code = unsafe {
                            slice::from_raw_parts(shader.pShaderBytecode.cast::<u8>(), shader.BytecodeLength)
                        }
                        .to_vec();
                        shader.pShaderBytecode = code.as_ptr().cast();
                        owned._bytes.push(code);
                    }
                }
                7 => {
                    let so = unsafe { &mut *payload.cast::<D3D12_STREAM_OUTPUT_DESC>() };
                    let mut entries = unsafe { copy_array(so.pSODeclaration, so.NumEntries as usize) }?;
                    for entry in &mut entries {
                        entry.SemanticName = unsafe { copy_name(&mut owned._names, entry.SemanticName) }?;
                    }
                    let strides = unsafe { copy_array(so.pBufferStrides, so.NumStrides as usize) }?;
                    so.pSODeclaration = entries.as_ptr();
                    so.pBufferStrides = strides.as_ptr();
                    owned._declarations.push(entries);
                    owned._strides.push(strides);
                }
                12 => {
                    let input = unsafe { &mut *payload.cast::<D3D12_INPUT_LAYOUT_DESC>() };
                    let mut elements =
                        unsafe { copy_array(input.pInputElementDescs, input.NumElements as usize) }?;
                    for element in &mut elements {
                        element.SemanticName = unsafe { copy_name(&mut owned._names, element.SemanticName) }?;
                    }
                    input.pInputElementDescs = elements.as_ptr();
                    owned._elements.push(elements);
                }
                // create() discards console PSO caches; never keep a guest pointer.
                19 => unsafe {
                    payload.cast::<D3D12_CACHED_PIPELINE_STATE>().write(Default::default())
                },
                22 => {
                    let views = unsafe { &mut *payload.cast::<D3D12_VIEW_INSTANCING_DESC>() };
                    let locations = unsafe {
                        copy_array(views.pViewInstanceLocations, views.ViewInstanceCount as usize)
                    }?;
                    views.pViewInstanceLocations = locations.as_ptr();
                    owned._views.push(locations);
                }
                // The trailing console stencil record ends the stream.
                STENCIL_X => break,
                _ => {}
            }
            at += bytes;
        }
        owned.storage = storage;
        Some(owned)
    }

    fn desc(&mut self) -> D3D12_PIPELINE_STATE_STREAM_DESC {
        D3D12_PIPELINE_STATE_STREAM_DESC {
            SizeInBytes: self.size,
            pPipelineStateSubobjectStream: self.storage.as_mut_ptr().cast(),
        }
    }
}

/// A self-owned copy of a slot 62 compute descriptor (no extensions).
pub(in crate::device) struct OwnedCompute {
    code: Vec<u8>,
    root: Option<ID3D12RootSignature>,
    node_mask: u32,
    flags: D3D12_PIPELINE_STATE_FLAGS,
}

impl OwnedCompute {
    pub(in crate::device) fn copy(desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC, code: &[u8]) -> Self {
        OwnedCompute {
            code: code.to_vec(),
            // Retains the guest root proxy.
            root: (*desc.pRootSignature).clone(),
            node_mask: desc.NodeMask,
            flags: desc.Flags,
        }
    }

    fn desc(&self) -> D3D12_COMPUTE_PIPELINE_STATE_DESC {
        D3D12_COMPUTE_PIPELINE_STATE_DESC {
            // SAFETY: a borrowed copy; ManuallyDrop never releases it.
            pRootSignature: core::mem::ManuallyDrop::new(unsafe { core::mem::transmute_copy(&self.root) }),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: self.code.as_ptr().cast(),
                BytecodeLength: self.code.len(),
            },
            NodeMask: self.node_mask,
            Flags: self.flags,
            ..Default::default()
        }
    }
}

pub(in crate::device) enum Work {
    Stream(OwnedStream),
    Compute(OwnedCompute),
}

struct Job {
    /// The guest device, retained until the worker is done with it.
    device: IUnknown,
    work: Work,
    slot: Arc<Upgrade>,
}

// SAFETY: the stream owns all its memory; D3D12 and proxy objects are free-threaded.
unsafe impl Send for Job {}

static WAITING: AtomicU64 = AtomicU64::new(0);
static MADE: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set when a pipeline created on this thread fell back to a null pipeline.
    static NULLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread is a pipeline worker (its builds are never deferred).
pub(in crate::device) fn on_worker() -> bool {
    WORKER.with(|w| w.get())
}

/// The pipeline being created on this thread got null shaders (see `fallback`).
pub(in crate::device) fn mark_nulled() {
    NULLED.with(|n| n.set(true));
}

/// Whether a pipeline an earlier run bound is made on the title's thread
/// inside its Create call (TUXBOX_EAGER_ON_TITLE=1, the old way) instead of
/// being handed to the workers at once behind the placeholder. Made on the
/// title's thread, one pipeline whose driver compile never finished
/// (nvgpucomp64 busy for minutes on tuxbox-pipeline) held the title's loader
/// in its Create call: every run after the first, which had made it lazily,
/// stayed black after sign-in. The first bind still waits for it within the
/// frame's translator budget.
pub(in crate::device) fn eager_on_title() -> bool {
    crate::env_toggle!("TUXBOX_EAGER_ON_TITLE") == Some("1")
}

/// Hands a pipeline to the workers; the returned slot fills once its host
/// pipeline exists.
pub(in crate::device) fn queue(device: IUnknown, work: Work) -> Arc<Upgrade> {
    let slot = Arc::new(Upgrade::default());
    send(Job { device, work, slot: slot.clone() }, false);
    slot
}

/// Jobs waiting for a worker: those a command list has bound (`bound`) go
/// first. test64 queued some 1100 pipelines earlier runs had used at the
/// intro, first in first out, and the few the title bound waited behind them
/// while each bind spent the frame's budget waiting: 1.8 fps.
/// TUXBOX_PIPELINE_PRIORITY=0 keeps one queue in creation order.
#[derive(Default)]
struct Jobs {
    bound: std::collections::VecDeque<Job>,
    created: std::collections::VecDeque<Job>,
}

static JOBS: (Mutex<Option<Jobs>>, std::sync::Condvar) = (Mutex::new(None), std::sync::Condvar::new());

fn priority() -> bool {
    crate::env_toggle!("TUXBOX_PIPELINE_PRIORITY") != Some("0")
}

/// A bound pipeline still in the creation-order queue moves to the front.
fn promote(slot: &Arc<Upgrade>) {
    if slot.built.get().is_some() || slot.failed.load(Ordering::Relaxed) || !priority() {
        return;
    }
    let mut g = JOBS.0.lock().unwrap_or_else(|e| e.into_inner());
    let Some(jobs) = g.as_mut() else { return };
    if let Some(at) = jobs.created.iter().position(|j| Arc::ptr_eq(&j.slot, slot))
        && let Some(job) = jobs.created.remove(at)
    {
        jobs.bound.push_back(job);
        static PROMOTED: AtomicU64 = AtomicU64::new(0);
        let n = PROMOTED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a bound pipeline moved ahead of {at} queued ones ({n} so far; TUXBOX_PIPELINE_PRIORITY=0 off)"));
        }
    }
}

/// test108: compute pipelines the title bound waited their full 3 s while
/// queued behind workers held up by compiles waiting for memory, so their
/// dispatches (and the progress writes the title's watchdog waits on) were
/// lost. A pipeline the title waits on that no worker has started is taken
/// from the queue and made on the title's thread. TUXBOX_COMPUTE_STEAL=0 off.
fn steal_on() -> bool {
    crate::env_toggle!("TUXBOX_COMPUTE_STEAL") != Some("0")
}

fn steal(slot: &Arc<Upgrade>) -> Option<Job> {
    if slot.built.get().is_some() || slot.failed.load(Ordering::Relaxed) || slot.running.load(Ordering::Acquire) != 0 {
        return None;
    }
    let mut g = JOBS.0.lock().unwrap_or_else(|e| e.into_inner());
    let jobs = g.as_mut()?;
    if let Some(at) = jobs.bound.iter().position(|j| Arc::ptr_eq(&j.slot, slot)) {
        return jobs.bound.remove(at);
    }
    let at = jobs.created.iter().position(|j| Arc::ptr_eq(&j.slot, slot))?;
    jobs.created.remove(at)
}

/// Builds a job on this thread as a worker would, keeping its priority. Its
/// translator wait lasts up to `limit` (the title's wait) rather than the frame
/// budget: test109 lost 61 compute pipelines to E_NOTIMPL when a spent budget
/// ended the stolen build at once and marked it failed for good. A build that
/// runs out of time goes back to the front of the workers' queue.
fn build_here(job: Job, limit: std::time::Duration) {
    use windows::Win32::System::Threading::*;
    let started = std::time::Instant::now();
    let priority = unsafe { GetThreadPriority(GetCurrentThread()) };
    crate::big_stack::run(|| {
        let was = WORKER.with(|w| w.replace(true));
        let stealing = STEALING.with(|s| s.replace(true));
        let deadline = crate::device::shader_cache::wait_until(Some(started + limit));
        let _ = crate::device::shader_cache::take_missed();
        build(job);
        crate::device::shader_cache::wait_until(deadline);
        STEALING.with(|s| s.set(stealing));
        WORKER.with(|w| w.set(was));
    });
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(priority)) };
    static STOLEN: AtomicU64 = AtomicU64::new(0);
    let n = STOLEN.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "background pipelines: a queued pipeline the title waits on made on its thread in {:.1} ms ({n} so far; TUXBOX_COMPUTE_STEAL=0 off)",
            started.elapsed().as_secs_f64() * 1e3));
    }
}

fn next_job() -> Job {
    let (lock, ready) = &JOBS;
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let jobs = g.get_or_insert_with(Default::default);
        if let Some(job) = jobs.bound.pop_front().or_else(|| jobs.created.pop_front()) {
            return job;
        }
        g = ready.wait(g).unwrap_or_else(|e| e.into_inner());
    }
}

fn send(job: Job, bound: bool) {
    static STARTED: OnceLock<bool> = OnceLock::new();
    let started = *STARTED.get_or_init(|| {
        let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 8));
        let mut any = false;
        for i in 0..workers {
            let spawned = std::thread::Builder::new()
                .name(format!("tuxbox-pipelines-{i}"))
                .stack_size(crate::big_stack::STACK_BYTES)
                .spawn(move || {
                    WORKER.with(|w| w.set(true));
                    crate::big_stack::mark_big();
                    crate::device::shader_cache::wait_unbounded();
                    // Below the title's own loading and recording threads.
                    unsafe {
                        use windows::Win32::System::Threading::*;
                        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                    }
                    loop {
                        build(next_job());
                    }
                });
            any |= spawned.is_ok();
        }
        any
    });
    if !started {
        // No worker: the pipeline is never made.
        return;
    }
    WAITING.fetch_add(1, Ordering::Relaxed);
    let (lock, ready) = &JOBS;
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let jobs = g.get_or_insert_with(Default::default);
    if bound && priority() {
        jobs.bound.push_back(job);
    } else {
        jobs.created.push_back(job);
    }
    drop(g);
    ready.notify_one();
}

thread_local! {
    /// This title thread is building a stolen job (see build_here).
    static STEALING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn build(mut job: Job) {
    // The title released the pipeline before a worker reached it.
    if Arc::strong_count(&job.slot) > 1 {
        // SAFETY: a device proxy begins with its IUnknown identity, retained by the job.
        let device = unsafe { &*job.device.as_raw().cast::<Device>() };
        let iid = ID3D12PipelineState::IID;
        let mut out = ptr::null_mut();
        NULLED.with(|n| n.set(false));
        job.slot.running.store(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, Ordering::Release);
        // The ordinary entry points: on this thread nothing is deferred and
        // translator waits have no budget.
        let status = match &mut job.work {
            Work::Stream(stream) => {
                let desc = stream.desc();
                unsafe { super::create_pipeline(device, &desc, &iid, &mut out) }
            }
            Work::Compute(compute) => {
                let desc = compute.desc();
                unsafe { create_pipeline_x(device, &desc, 0, ptr::null(), &iid, &mut out) }
            }
        };
        job.slot.running.store(0, Ordering::Release);
        // Back below the title after any hurry().
        unsafe {
            use windows::Win32::System::Threading::*;
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
        let nulled = NULLED.with(|n| n.replace(false));
        let made = (status.is_ok() && !out.is_null()).then(|| unsafe { IUnknown::from_raw(out) });
        let counter = match made {
            Some(proxy) if !nulled => {
                let _ = job.slot.built.set(Built(proxy));
                &MADE
            }
            _ if STEALING.with(|s| s.get()) && crate::device::shader_cache::take_missed() => {
                // Out of time, not untranslatable: back to the workers, first in line.
                let (lock, ready) = &JOBS;
                let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                g.get_or_insert_with(Default::default).bound.push_front(job);
                drop(g);
                ready.notify_one();
                return;
            }
            _ => {
                crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::PipelinesUntranslatable);
                job.slot.failed.store(true, Ordering::Relaxed);
                &FAILED
            }
        };
        let (lock, done) = &DONE;
        drop(lock.lock().unwrap_or_else(|e| e.into_inner()));
        done.notify_all();
        let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
        let left = WAITING.fetch_sub(1, Ordering::Relaxed) - 1;
        if count.is_power_of_two() || left == 0 {
            log(left);
        }
    } else {
        WAITING.fetch_sub(1, Ordering::Relaxed);
    }
}

fn log(left: u64) {
    let made = MADE.load(Ordering::Relaxed);
    let failed = FAILED.load(Ordering::Relaxed);
    let lazy = LAZY.load(Ordering::Relaxed);
    crate::diagnostics::log_once(&std::sync::Once::new(), || {
        format!("background pipelines: {made} made, {failed} untranslatable, {left} waiting, {lazy} created lazily")
    });
}
