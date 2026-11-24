//! Pipelines whose translation missed the title's wait budget are rebuilt by
//! workers (VIctory's "pipelines made off the title's threads"). The title gets
//! a null pipeline at once; its draws are dropped only until a worker, which
//! waits on the translator without a budget, has made the real host pipeline
//! and published it in the proxy's upgrade slot. Command lists read that slot
//! whenever they bind the pipeline, so the render thread never waits on the
//! translator or on the driver's compile.
use super::super::compute::PipelineData;
use super::super::Device;
use super::{AMPLIFICATION_X, MESH_X, STENCIL_X, layout};
use core::{ffi::c_void, mem::MaybeUninit, ptr, slice};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Interface, PCSTR};

/// The host pipeline a worker made for a pipeline created without its translation.
pub(crate) struct Built {
    pub(crate) native: ID3D12PipelineState,
    pub(crate) state: PipelineData,
}

/// Written once by a worker, then only read by command lists.
#[derive(Default)]
pub(crate) struct Upgrade {
    built: OnceLock<Built>,
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

/// A self-owned deep copy of a guest pipeline stream. Guest pointers are only
/// valid during CreatePipelineState, so every referenced array, string and
/// shader is copied, and the guest root signature is retained.
pub(super) struct OwnedStream {
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
    pub(super) unsafe fn copy(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<Self> {
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

struct Job {
    /// The guest device, retained until the worker is done with it.
    device: IUnknown,
    stream: OwnedStream,
    slot: Arc<Upgrade>,
}

// SAFETY: the stream owns all its memory; D3D12 and proxy objects are free-threaded.
unsafe impl Send for Job {}

static WAITING: AtomicU64 = AtomicU64::new(0);
static MADE: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread is a pipeline worker (its builds are never deferred).
pub(super) fn on_worker() -> bool {
    WORKER.with(|w| w.get())
}

/// Hands a pipeline to the workers; the returned slot fills once its host
/// pipeline exists.
pub(super) fn queue(device: IUnknown, stream: OwnedStream) -> Arc<Upgrade> {
    let slot = Arc::new(Upgrade::default());
    let job = Job { device, stream, slot: slot.clone() };
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
                .spawn(move || {
                    WORKER.with(|w| w.set(true));
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
        // No worker: the pipeline keeps its null host pipeline.
        WAITING.fetch_sub(1, Ordering::Relaxed);
    }
    slot
}

fn build(mut job: Job) {
    // The title released the pipeline before a worker reached it.
    if Arc::strong_count(&job.slot) > 1 {
        // SAFETY: a device proxy begins with its IUnknown identity, retained by the job.
        let device = unsafe { &*job.device.as_raw().cast::<Device>() };
        let desc = job.stream.desc();
        let mut state = PipelineData::default();
        super::NULLED.with(|n| n.set(false));
        let made = unsafe { super::create(device, &desc, &mut state) };
        let nulled = super::NULLED.with(|n| n.replace(false));
        let counter = match made {
            Ok(native) if !nulled => {
                let _ = job.slot.built.set(Built { native, state });
                &MADE
            }
            _ => &FAILED,
        };
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
    crate::diagnostics::log_once(&std::sync::Once::new(), || {
        format!("background pipelines: {made} made, {failed} untranslatable, {left} waiting")
    });
}
