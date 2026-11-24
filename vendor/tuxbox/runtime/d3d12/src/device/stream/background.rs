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
    let slot = Upgrade { built: OnceLock::new(), deferred: Mutex::new(None), failed: std::sync::atomic::AtomicBool::new(true) };
    Arc::new(slot)
}

pub(crate) fn ready(slot: &Arc<Upgrade>) -> Option<&Built> {
    if let Some(built) = slot.built.get() {
        return Some(built);
    }
    let deferred = slot.deferred.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((device, work)) = deferred {
        send(Job { device, work, slot: slot.clone() });
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

/// Hands a pipeline to the workers; the returned slot fills once its host
/// pipeline exists.
pub(in crate::device) fn queue(device: IUnknown, work: Work) -> Arc<Upgrade> {
    let slot = Arc::new(Upgrade::default());
    send(Job { device, work, slot: slot.clone() });
    slot
}

fn send(job: Job) {
    static SENDER: OnceLock<Option<Mutex<std::sync::mpsc::Sender<Job>>>> = OnceLock::new();
    let sender = SENDER.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 8));
        let mut any = false;
        for i in 0..workers {
            let receiver = receiver.clone();
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
                        let next = receiver.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok(job) = next else { return };
                        build(job);
                    }
                });
            any |= spawned.is_ok();
        }
        any.then(|| Mutex::new(sender))
    });
    WAITING.fetch_add(1, Ordering::Relaxed);
    let sent = sender
        .as_ref()
        .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).send(job));
    if !matches!(sent, Some(Ok(()))) {
        // No worker: the pipeline is never made.
        WAITING.fetch_sub(1, Ordering::Relaxed);
    }
}

fn build(mut job: Job) {
    // The title released the pipeline before a worker reached it.
    if Arc::strong_count(&job.slot) > 1 {
        // SAFETY: a device proxy begins with its IUnknown identity, retained by the job.
        let device = unsafe { &*job.device.as_raw().cast::<Device>() };
        let iid = ID3D12PipelineState::IID;
        let mut out = ptr::null_mut();
        NULLED.with(|n| n.set(false));
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
        let nulled = NULLED.with(|n| n.replace(false));
        let made = (status.is_ok() && !out.is_null()).then(|| unsafe { IUnknown::from_raw(out) });
        let counter = match made {
            Some(proxy) if !nulled => {
                let _ = job.slot.built.set(Built(proxy));
                &MADE
            }
            _ => {
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
