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
            return Err(invalid("no batches, or an unaligned pool"));
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
            // An empty batch maps nothing: its destination and stale ranges
            // pointer are not read (xmem ignores them too), so it is skipped
            // instead of refusing the whole call. TUXBOX_PAGE_MAPPING_STRICT=1
            // refuses it as before.
            if batch.count == 0 && !strict() {
                continue;
            }
            if batch.destination == 0
                || !batch.destination.is_multiple_of(PAGE)
                || (batch.count == 0) != batch.ranges.is_null()
            {
                return Err(invalid("null or unaligned destination, or a range count without ranges"));
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
                    _ => return Err(invalid("range kind unknown, or its pages outside the registered pool")),
                }
                end = end
                    .checked_add(u64::from(range.count) * PAGE)
                    .ok_or(E_INVALIDARG)?;
            }
            if batch.destination < pool_end && source < end {
                return Err(invalid("destination overlaps the registered pool"));
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
        note_remap_targets(&spans);
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
                let checked = unsafe {
                    validate(
                        batches.len() as u32,
                        batches.as_ptr(),
                        self.source,
                        self.pages,
                        import.start,
                        import.end,
                    )
                };
                // A real replacement inside a GPU-imported range: the imported
                // heap keeps the pages it saw. Refusing it (E_NOTIMPL) also
                // rejected the queue, so every later CopyPageMappingsBatchX
                // and submission failed (test36). Apply the CPU remap anyway,
                // logged; TUXBOX_PINNED_REMAP=0 refuses as before.
                if checked == E_NOTIMPL && pinned_remap() {
                    pinned_remapped(import);
                    // Later imports of this range must open the new pages.
                    crate::device::retire_imports_over(import.start, import.end);
                    continue;
                }
                checked.ok()?;
            }
        }
        let whole = unsafe {
            (self.apply)(
                batches.len() as u32,
                batches.as_ptr(),
                self.source,
                self.pages,
            )
        };
        if whole != E_INVALIDARG || batches.len() < 2 || strict() {
            return whole.ok();
        }
        // test43-76 refused about 30 calls a run with E_INVALIDARG (up to 881
        // batches into the title's 6236 MiB pool) and xmem gave no reason; the
        // whole call mapped nothing. Each batch is applied on its own, in
        // order, so only the refused ones stay unmapped; they are logged.
        // TUXBOX_PAGE_MAPPING_STRICT=1 refuses the whole call as before.
        let mut refused = Vec::new();
        for (i, batch) in batches.iter().enumerate() {
            let one = unsafe { (self.apply)(1, batch, self.source, self.pages) };
            if one.is_err() {
                refused.push((i, batch.destination, batch.count, one));
            }
        }
        static SPLITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SPLITS.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            let shown: Vec<String> = refused.iter().take(4)
                .map(|(i, at, count, hr)| format!("#{i} at {at:#x} ({count} ranges): {:#x}", hr.0 as u32))
                .collect();
            crate::diagnostics::log_line(format_args!(
                "CopyPageMappingsBatchX refused whole: applied its {} batches one by one, {} refused [{}] ({n} so far; TUXBOX_PAGE_MAPPING_STRICT=1 refuses)",
                batches.len(), refused.len(), shown.join(", ")));
        }
        if refused.len() == batches.len() {
            return Err(E_INVALIDARG.into());
        }
        Ok(())
    }
}

/// Whether a page-mapping failure poisons the queue. Latching it made every
/// later CopyPageMappingsBatchX and submission return the first failure
/// (test38: 0x80070057 on every batch). Now only device loss latches and a
/// bad batch fails alone; TUXBOX_PAGE_MAPPING_LATCH=1 latches every failure.
fn latches(code: HRESULT) -> bool {
    use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
    crate::env_toggle!("TUXBOX_PAGE_MAPPING_LATCH") == Some("1")
        || matches!(code, DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_HUNG | DXGI_ERROR_DEVICE_RESET)
}

/// E_INVALIDARG with the reason logged (first eight, then powers of two).
fn invalid(reason: &str) -> windows::core::Error {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if remap_logged(n) {
        crate::diagnostics::log_line(format_args!("CopyPageMappingsBatchX refused: {reason} ({n} so far)"));
    }
    E_INVALIDARG.into()
}

#[cfg(test)]
#[test]
fn only_device_loss_latches_by_default() {
    use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED;
    if crate::env_toggle!("TUXBOX_PAGE_MAPPING_LATCH").is_none() {
        assert!(!latches(E_INVALIDARG));
        assert!(latches(DXGI_ERROR_DEVICE_REMOVED));
    }
}

fn strict() -> bool {
    crate::env_toggle!("TUXBOX_PAGE_MAPPING_STRICT") == Some("1")
}

fn pinned_remap() -> bool {
    crate::env_toggle!("TUXBOX_PINNED_REMAP") != Some("0")
}

/// GPU-imported ranges whose CPU pages were remapped while imported (the
/// last 256), so a GPU memory wait can say its word has two backings.
static PINNED: std::sync::Mutex<std::collections::VecDeque<(Range<u64>, std::time::Instant)>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// "; imported range X..Y had its CPU pages remapped N ms ago" when `address`
/// lies in a range remapped while GPU-imported.
pub(crate) fn remapped_under_import(address: u64) -> String {
    let pinned = PINNED.lock().unwrap_or_else(|e| e.into_inner());
    pinned.iter().rev().find(|(r, _)| r.contains(&address)).map_or_else(String::new, |(r, at)| format!(
        "; the GPU-imported range {:#x}..{:#x} holding it had its CPU pages remapped {} ms ago (the GPU writes the old pages)",
        r.start, r.end, at.elapsed().as_millis()))
}

static ANY_PINNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether pages under a GPU-imported range overlapping [lo, hi) were
/// remapped (the import keeps the old ones; see device views cbv_snapshot).
/// Title ranges CopyPageMappingsBatchX has ever mapped pages into, merged
/// (start -> end). heap.rs never opens these as heaps: a heap opened over
/// them pins their views, and the next remap there fails (test145-147).
static REMAP_TARGETS: Mutex<std::collections::BTreeMap<u64, u64>> = Mutex::new(std::collections::BTreeMap::new());

fn note_remap_targets(spans: &[Range<u64>]) {
    let mut targets = REMAP_TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    for r in spans {
        let (mut lo, mut hi) = (r.start, r.end);
        let touching: Vec<u64> = targets.range(..=hi).rev()
            .take_while(|&(_, &end)| end >= lo).map(|(&start, _)| start).collect();
        for start in touching {
            let end = targets.remove(&start).unwrap_or(start);
            lo = lo.min(start);
            hi = hi.max(end);
        }
        targets.insert(lo, hi);
    }
}

/// Whether [lo, hi) overlaps a range CopyPageMappingsBatchX remaps.
pub(crate) fn remap_target_over(lo: u64, hi: u64) -> bool {
    let targets = REMAP_TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    targets.range(..hi).next_back().is_some_and(|(_, &end)| end > lo)
}

pub(crate) fn remapped_import_over(lo: u64, hi: u64) -> bool {
    if !ANY_PINNED.load(Ordering::Relaxed) {
        return false;
    }
    let pinned = PINNED.lock().unwrap_or_else(|e| e.into_inner());
    pinned.iter().any(|(r, _)| r.start < hi && lo < r.end)
}

fn pinned_remapped(import: &Range<u64>) {
    ANY_PINNED.store(true, Ordering::Relaxed);
    {
        let mut pinned = PINNED.lock().unwrap_or_else(|e| e.into_inner());
        if pinned.len() >= 256 {
            pinned.pop_front();
        }
        pinned.push_back((import.clone(), std::time::Instant::now()));
    }
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if remap_logged(n) {
        crate::diagnostics::log_line(format_args!(
            "CopyPageMappingsBatchX remaps pages inside GPU-imported range {:#x}..{:#x}: applied on the CPU, the imported heap keeps its old pages ({n} so far; TUXBOX_PINNED_REMAP=0 refuses)",
            import.start, import.end));
    }
}

fn remap_logged(n: u64) -> bool {
    n <= 8 || n.is_power_of_two()
}

#[cfg(test)]
#[test]
fn pinned_remaps_log_first_eight_then_powers_of_two() {
    assert!(remap_logged(1) && remap_logged(8) && remap_logged(64));
    assert!(!remap_logged(9) && !remap_logged(100));
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
    range: Range<u64>,
}

/// Tells xmem a title range is (or no longer is) a GPU-imported heap, so a
/// physical page remapped inside it is reported (TuxBoxNoteGpuImport).
fn note_import(range: &Range<u64>, add: bool) {
    type Note = unsafe extern "system" fn(usize, usize, i32);
    static NOTE: std::sync::OnceLock<Option<Note>> = std::sync::OnceLock::new();
    let note = NOTE.get_or_init(|| unsafe {
        let module = GetModuleHandleW(w!("xmem.dll")).ok()?;
        GetProcAddress(module, s!("TuxBoxNoteGpuImport")).map(|f| std::mem::transmute::<_, Note>(f))
    });
    if let Some(note) = note {
        unsafe { note(range.start as usize, (range.end - range.start) as usize, i32::from(add)) };
    }
}

impl Drop for ImportPin {
    fn drop(&mut self) {
        note_import(&self.range, false);
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
            let range = address..address.saturating_add(bytes);
            state.imports.insert(key, range.clone());
            note_import(&range, true);
            Ok(ImportPin { key, range })
        })
    }
    fn register(&self, mapping: &Arc<Pending>) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.retain(|p| p.strong_count() != 0);
        state.pending.push(Arc::downgrade(mapping));
    }
    fn apply(&self, mappings: &Mappings) -> Result<()> {
        // Released before taking the registry lock: an import's pin drop takes it.
        let pinned: Vec<Range<u64>> = {
            let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.imports.values()
                .filter(|import| mappings.spans.iter().any(|r| r.start < import.end && import.start < r.end))
                .cloned().collect()
        };
        if !pinned.is_empty() {
            let released: usize = mappings.spans.iter().map(|r| crate::device::release_shadows_over(r.start, r.end)).sum();
            if released != 0 {
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = N.fetch_add(1, Ordering::Relaxed) + 1;
                if remap_logged(n) {
                    crate::diagnostics::log_line(format_args!(
                        "CopyPageMappingsBatchX over {} GPU-imported ranges: released {released} dead shadows and cached imports first ({n} so far; TUXBOX_SHADOW_RELEASE_ON_REMAP=0 keeps them)",
                        pinned.len()));
                }
            }
        }
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
                    let latch = |code: HRESULT| {
                        if latches(code) {
                            error.store(code.0, Ordering::Release);
                        }
                    };
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
                        latch(e.code());
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
        crate::diagnostics::queue_waits::note(windows::core::Interface::as_raw(&queue.native) as usize, "page mapping", &fence, 2);
        unsafe { queue.native.Wait(&fence, 2) }
    })();
    if let Err(e) = result {
        crate::diagnostics::recording_error(format_args!(
            "CopyPageMappingsBatchX failed {:?}, batches={count} pool={source:#x} pages={pages} flags={flags:#x}",
            e.code()
        ));
        if latches(e.code()) {
            reject(queue, e.code());
        }
        e.code()
    } else {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || {
            format!("CopyPageMappingsBatchX mapped {count} batches from {pages} registered pages")
        });
        S_OK
    }
}
