//! Queue-ordered changes to the Series GPU virtual aperture. XMem owns the
//! underlying physical page tokens and creates real aliases of their sections.
use super::{Queue, reject};
use core::{ptr, slice};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak, mpsc};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, HANDLE, S_OK};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::core::{HRESULT, Result, s, w};

const PAGE: u64 = 65536;
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct PageRange {
    kind: u32,
    source: u32,
    count: u32,
}
#[repr(C)]
pub(super) struct Batch {
    destination: u64,
    count: u32,
    ranges: *const PageRange,
}
const _: () = assert!(size_of::<PageRange>() == 12 && size_of::<Batch>() == 24);
type Apply = unsafe extern "system" fn(u32, *const Batch, u64, u32) -> HRESULT;

type ValidatePinned = unsafe extern "system" fn(u32, *const Batch, u64, u32, u64, u64) -> HRESULT;

struct Mappings {
    source: u64,
    pages: u32,
    batches: Vec<(u64, Vec<PageRange>)>,
    spans: Arc<[Range<u64>]>,
    apply: Apply,
}

impl Mappings {
    unsafe fn read(count: u32, batches: *const Batch, source: u64, pages: u32) -> Result<Self> {
        if count == 0 || batches.is_null() || !source.is_multiple_of(PAGE) {
            return Err(E_INVALIDARG.into());
        }
        let module = unsafe { GetModuleHandleW(w!("xmem.dll"))? };
        let apply: Apply = unsafe {
            std::mem::transmute(
                GetProcAddress(module, s!("TuxBoxCopyPageMappings")).ok_or(E_NOTIMPL)?,
            )
        };
        let mut copied = Vec::new();
        let mut spans = Vec::new();
        copied
            .try_reserve_exact(count as usize)
            .map_err(|_| E_OUTOFMEMORY)?;
        spans
            .try_reserve_exact(count as usize)
            .map_err(|_| E_OUTOFMEMORY)?;
        let pool_end = source
            .checked_add(u64::from(pages) * PAGE)
            .ok_or(E_INVALIDARG)?;
        for batch in unsafe { slice::from_raw_parts(batches, count as usize) } {
            if batch.destination == 0
                || !batch.destination.is_multiple_of(PAGE)
                || (batch.count == 0) != batch.ranges.is_null()
            {
                return Err(E_INVALIDARG.into());
            }
            let ranges = if batch.count == 0 {
                &[]
            } else {
                unsafe { slice::from_raw_parts(batch.ranges, batch.count as usize) }
            };
            let mut end = batch.destination;
            for range in ranges {
                match range.kind {
                    0 if range
                        .source
                        .checked_add(range.count)
                        .is_some_and(|v| v <= pages)
                        && source != 0 =>
                    {
                        ()
                    }
                    1 if range.source < pages && source != 0 => (),
                    2 => (),
                    3 => continue,
                    _ => return Err(E_INVALIDARG.into()),
                }
                end = end
                    .checked_add(u64::from(range.count) * PAGE)
                    .ok_or(E_INVALIDARG)?;
            }
            if batch.destination < pool_end && source < end {
                return Err(E_INVALIDARG.into());
            }
            if end > batch.destination {
                spans.push(batch.destination..end);
            }
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(ranges.len())
                .map_err(|_| E_OUTOFMEMORY)?;
            owned.extend_from_slice(ranges);
            copied.push((batch.destination, owned));
        }
        Ok(Self {
            source,
            pages,
            batches: copied,
            spans: spans.into(),
            apply,
        })
    }

    fn apply(&self, imports: &HashMap<usize, Range<u64>>) -> Result<()> {
        let mut batches = Vec::new();
        batches
            .try_reserve_exact(self.batches.len())
            .map_err(|_| E_OUTOFMEMORY)?;
        for (destination, ranges) in &self.batches {
            batches.push(Batch {
                destination: *destination,
                count: ranges.len() as u32,
                ranges: if ranges.is_empty() {
                    ptr::null()
                } else {
                    ranges.as_ptr()
                },
            });
        }
        let mut overlaps = imports
            .values()
            .filter(|import| {
                self.spans
                    .iter()
                    .any(|r| r.start < import.end && import.start < r.end)
            })
            .peekable();
        if overlaps.peek().is_some() {
            // Once Vulkan has imported a page, changing the CPU view cannot
            // retarget that existing GPU allocation. Identical mappings are safe.
            let module = unsafe { GetModuleHandleW(w!("xmem.dll"))? };
            let validate: ValidatePinned = unsafe {
                std::mem::transmute(
                    GetProcAddress(module, s!("TuxBoxValidatePinnedPageMappings"))
                        .ok_or(E_NOTIMPL)?,
                )
            };
            for import in overlaps {
                unsafe {
                    validate(
                        batches.len() as u32,
                        batches.as_ptr(),
                        self.source,
                        self.pages,
                        import.start,
                        import.end,
                    )
                }
                .ok()?;
            }
        }
        unsafe {
            (self.apply)(
                batches.len() as u32,
                batches.as_ptr(),
                self.source,
                self.pages,
            )
        }
        .ok()
    }
}

struct Pending {
    spans: Arc<[Range<u64>]>,
    fence: ID3D12Fence,
    result: AtomicI32,
}
#[derive(Default)]
struct State {
    pending: Vec<Weak<Pending>>,
    imports: HashMap<usize, Range<u64>>,
    next_import: usize,
}

#[derive(Default)]
pub(crate) struct Registry(Mutex<State>);

// Every device in the guest shares the same XMem address space.
pub(crate) static GPU_APERTURE: LazyLock<Registry> = LazyLock::new(Registry::default);

pub(crate) struct ImportPin {
    key: usize,
}
impl Drop for ImportPin {
    fn drop(&mut self) {
        GPU_APERTURE
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .imports
            .remove(&self.key);
    }
}

impl Registry {
    fn after_pending<T>(
        &self,
        address: u64,
        bytes: u64,
        ready: impl FnOnce(&mut State) -> Result<T>,
    ) -> Result<T> {
        let end = address.checked_add(bytes).ok_or(E_INVALIDARG)?;
        loop {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let pending = state
                .pending
                .iter()
                .filter_map(Weak::upgrade)
                .find(|p| p.spans.iter().any(|r| address < r.end && r.start < end));
            let Some(item) = pending else {
                return ready(&mut state);
            };
            if unsafe { item.fence.GetCompletedValue() } == 2 {
                HRESULT(item.result.load(Ordering::Acquire)).ok()?;
                state
                    .pending
                    .retain(|weak| !weak.ptr_eq(&Arc::downgrade(&item)));
                continue;
            }
            drop(state);
            unsafe {
                item.fence.SetEventOnCompletion(2, HANDLE::default())?;
            }
            HRESULT(item.result.load(Ordering::Acquire)).ok()?;
            if unsafe { item.fence.GetCompletedValue() } == u64::MAX {
                return Err(E_FAIL.into());
            }
        }
    }

    pub(crate) fn wait(&self, address: u64, bytes: u64) -> Result<()> {
        self.after_pending(address, bytes, |_| Ok(()))
    }
    pub(crate) fn pin(&self, address: u64, bytes: u64) -> Result<ImportPin> {
        self.after_pending(address, bytes, |state| {
            let key = state.next_import;
            state.next_import = key.checked_add(1).ok_or(E_OUTOFMEMORY)?;
            state.imports.insert(key, address..address + bytes);
            Ok(ImportPin { key })
        })
    }
    fn register(&self, mapping: &Arc<Pending>) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.retain(|p| p.strong_count() != 0);
        state.pending.push(Arc::downgrade(mapping));
    }
    fn apply(&self, mappings: &Mappings) -> Result<()> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        mappings.apply(&state.imports)
    }
}

struct Job {
    mappings: Mappings,
    pending: Arc<Pending>,
}
pub(super) struct Worker(mpsc::Sender<Job>);
impl Worker {
    fn new(error: Arc<AtomicI32>) -> Result<Self> {
        let (sender, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("TuxBox page mappings".into())
            .spawn(move || {
                for job in jobs {
                    let fence = &job.pending.fence;
                    let result = unsafe { fence.SetEventOnCompletion(1, HANDLE::default()) }
                        .and_then(|()| {
                            if unsafe { fence.GetCompletedValue() } == u64::MAX {
                                Err(E_FAIL.into())
                            } else {
                                GPU_APERTURE.apply(&job.mappings)
                            }
                        });
                    if let Err(e) = result {
                        job.pending.result.store(e.code().0, Ordering::Release);
                        error.store(e.code().0, Ordering::Release);
                        crate::diagnostics::recording_error(format_args!(
                            "GPU page mapping failed: {:?}",
                            e.code()
                        ));
                    }
                    let _ = unsafe { fence.Signal(2) };
                }
            })
            .map_err(|_| E_OUTOFMEMORY)?;
        Ok(Self(sender))
    }
}

pub(super) unsafe extern "system" fn copy(
    queue: &Queue,
    count: u32,
    batches: *const Batch,
    source: u64,
    pages: u32,
    flags: u32,
) -> HRESULT {
    let submitted = queue
        .state
        .submission
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let result = (|| -> Result<()> {
        HRESULT(queue.state.placement.error.load(Ordering::Acquire)).ok()?;
        if flags != 0 {
            return Err(E_NOTIMPL.into());
        }
        let mappings = unsafe { Mappings::read(count, batches, source, pages)? };
        if !*submitted {
            // A queue used exclusively for page mappings has no GPU work to
            // drain. Bind the new pages before a caller can create a resource.
            return GPU_APERTURE.apply(&mappings);
        }
        let device: ID3D12Device = {
            let mut object = None;
            unsafe {
                queue.native.GetDevice(&mut object)?;
            }
            object.ok_or(E_FAIL)?
        };
        let worker = match queue.state.page_worker.get() {
            Some(worker) => worker,
            None => {
                let new = Worker::new(queue.state.placement.error.clone())?;
                queue.state.page_worker.get_or_init(|| new)
            }
        };
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        unsafe {
            queue.native.Signal(&fence, 1)?;
        }
        if unsafe { fence.GetCompletedValue() } == 1 {
            return GPU_APERTURE.apply(&mappings);
        }
        let pending = Arc::new(Pending {
            spans: mappings.spans.clone(),
            fence: fence.clone(),
            result: AtomicI32::new(S_OK.0),
        });
        GPU_APERTURE.register(&pending);
        if worker
            .0
            .send(Job {
                mappings,
                pending: pending.clone(),
            })
            .is_err()
        {
            pending.result.store(E_FAIL.0, Ordering::Release);
            let _ = unsafe { fence.Signal(2) };
            return Err(E_FAIL.into());
        }
        unsafe { queue.native.Wait(&fence, 2) }
    })();
    if let Err(e) = result {
        crate::diagnostics::recording_error(format_args!(
            "CopyPageMappingsBatchX failed {:?}, batches={count} pool={source:#x} pages={pages} flags={flags:#x}",
            e.code()
        ));
        reject(queue, e.code());
        e.code()
    } else {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || {
            format!("CopyPageMappingsBatchX mapped {count} batches from {pages} registered pages")
        });
        S_OK
    }
}
