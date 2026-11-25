//! Translate waits on host-visible Series fence words at a native list boundary.
//! The game can write markers and sample timestamps before waiting. Keep these
//! prefix operations ordered without disturbing bindings on the main command list.
use super::*;
use std::sync::atomic::{AtomicI32, AtomicU64};
use std::sync::mpsc;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::core::Result;

pub(super) enum Packet {
    Marker(D3D12_WRITEBUFFERIMMEDIATE_PARAMETER),
    Timestamp(ID3D12QueryHeap, u32, ID3D12Resource, u64),
}

impl Packet {
    fn emit(&self, list: &ID3D12GraphicsCommandList) -> Result<()> {
        unsafe {
            match self {
                Self::Marker(write) => {
                    // Marker-out lands after the preceding work, as the
                    // bottom-of-pipe write it stands for does.
                    list.cast::<ID3D12GraphicsCommandList2>()?.WriteBufferImmediate(
                        1,
                        write,
                        Some(&D3D12_WRITEBUFFERIMMEDIATE_MODE_MARKER_OUT),
                    );
                }
                Self::Timestamp(heap, index, dest, offset) => {
                    list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, *index);
                    list.ResolveQueryData(
                        heap,
                        D3D12_QUERY_TYPE_TIMESTAMP,
                        *index,
                        1,
                        dest,
                        *offset,
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Prefix {
    pending: Vec<Packet>,
    in_body: bool,
    pub(super) segments: Vec<Arc<Segment>>,
}

impl Prefix {
    pub(super) fn push(&mut self, list: &ID3D12GraphicsCommandList, packet: Packet) -> Result<()> {
        if self.in_body {
            return packet.emit(list);
        }
        self.pending.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
        self.pending.push(packet);
        Ok(())
    }

    pub(super) fn finish(&mut self, list: &ID3D12GraphicsCommandList) -> Result<()> {
        if self.in_body {
            return Ok(());
        }
        self.in_body = true;
        for packet in self.pending.drain(..) {
            packet.emit(list)?;
        }
        Ok(())
    }
}

struct Word {
    resource: ID3D12Resource,
    mapped: usize,
}

impl Drop for Word {
    fn drop(&mut self) {
        unsafe { self.resource.Unmap(0, Some(&D3D12_RANGE::default())) };
    }
}

impl Word {
    fn map(list: &List, address: u64) -> Result<Self> {
        if address == 0 || !address.is_multiple_of(8) {
            return Err(E_INVALIDARG.into());
        }
        let (buffer, gpu) =
            unsafe { device::command_buffer_address(parent(&list.parent), address, 8)? };
        let resource = buffer.ok_or(E_NOTIMPL)?;
        let offset = gpu
            .checked_sub(unsafe { resource.GetGPUVirtualAddress() })
            .ok_or(E_INVALIDARG)? as usize;
        let range = D3D12_RANGE {
            Begin: offset,
            End: offset.checked_add(8).ok_or(E_INVALIDARG)?,
        };
        let mut base = ptr::null_mut();
        unsafe { resource.Map(0, Some(&range), Some(&mut base))? };
        if base.is_null() {
            unsafe { resource.Unmap(0, Some(&D3D12_RANGE::default())) };
            return Err(E_FAIL.into());
        }
        Ok(Self {
            resource,
            mapped: base.addr() + offset,
        })
    }

    fn value(&self) -> u64 {
        // X64 aligned 64-bit load. The COM resource and its CPU mapping stay alive
        // until all queued waits release their segment.
        unsafe { AtomicU64::from_ptr(self.mapped as *mut u64).load(Ordering::Acquire) }
    }

    fn reached(&self, value: u64) -> bool {
        self.value() >= value
    }
}

/// How long the queue is held for a guest memory word before it is released.
const STALL: Duration = Duration::from_secs(5);

pub(super) struct Segment {
    prefix: Option<(ID3D12CommandList, ID3D12CommandAllocator)>,
    word: Word,
    address: u64,
    target: u64,
}

/// One fence label operation a list recorded.
#[derive(Clone, Copy)]
pub(super) struct Label {
    /// `None` for a write. For a wait, whether it holds the queue (`false` when
    /// it came after rendering commands and became a barrier).
    wait: Option<bool>,
    address: u64,
    value: u64,
}

impl Label {
    pub(super) fn write(address: u64, value: u64) -> Self {
        Self { wait: None, address, value }
    }
}

/// What happened to a closed list.
#[derive(Clone, Copy)]
pub(super) enum Fate {
    /// Submitted to a queue of this command list type.
    Submitted(i32),
    /// Rejected at Close with this status, so it never runs.
    Dropped(i32),
}

struct Seen {
    order: u64,
    list: usize,
    fate: Fate,
    labels: Vec<Label>,
}

/// The most recent lists that wrote or waited on a label, oldest first.
static HISTORY: std::sync::Mutex<std::collections::VecDeque<Seen>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

pub(super) fn note_list(list: usize, fate: Fate, labels: &[Label]) {
    if labels.is_empty() {
        return;
    }
    static ORDER: AtomicU64 = AtomicU64::new(0);
    let mut history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    if history.len() >= 4096 {
        history.pop_front();
    }
    history.push_back(Seen {
        order: ORDER.fetch_add(1, Ordering::Relaxed),
        list,
        fate,
        labels: labels.to_vec(),
    });
}

fn fate(seen: &Seen) -> String {
    match seen.fate {
        Fate::Submitted(kind) => format!("submitted to a type {kind} queue"),
        Fate::Dropped(status) => format!("rejected at Close ({status:#010x})"),
    }
}

/// Say which lists should have satisfied a stalled wait, and which waits stand
/// before it: the one waiting and the one writing.
fn explain(address: u64, target: u64) {
    let history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mut found = 0;
    for seen in history.iter() {
        for (at, label) in seen.labels.iter().enumerate() {
            let matches = label.address == address
                && match label.wait {
                    None => label.value >= target,
                    Some(_) => label.value == target,
                };
            if !matches || found >= 6 {
                continue;
            }
            found += 1;
            let earlier: Vec<_> = seen.labels[..at]
                .iter()
                .filter(|l| l.wait == Some(true))
                .map(|l| format!("{:#x}>={:#x}", l.address, l.value))
                .collect();
            crate::diagnostics::log(&format!(
                "stalled label {address:#x} target {target:#x}: {} by list {:#x} (order {}), {}, after waits [{}]",
                if label.wait.is_some() { "waited" } else { "written" },
                seen.list, seen.order, fate(seen), earlier.join(" "),
            ));
        }
    }
    if !history.iter().any(|seen| seen.labels.iter().any(|l| {
        l.wait.is_none() && l.address == address && l.value >= target
    })) {
        crate::diagnostics::log(&format!(
            "stalled label {address:#x} target {target:#x}: no submitted or rejected list writes it"
        ));
    }
}

/// Guest addresses the title has waited on, so writes to them can be logged.
static WAITED: std::sync::Mutex<Vec<(u64, u64)>> = std::sync::Mutex::new(Vec::new());

/// Log the first waits on each address, then each power of two.
fn trace_wait(address: u64, target: u64, value: u64, list_type: i32, mid_list: bool) {
    let mut waited = WAITED.lock().unwrap_or_else(|e| e.into_inner());
    let count = if let Some((_, count)) = waited.iter_mut().find(|(known, _)| *known == address) {
        *count += 1;
        *count
    } else if waited.len() < 256 {
        waited.push((address, 1));
        1
    } else {
        return;
    };
    drop(waited);
    if count <= 4 || count.is_power_of_two() {
        crate::diagnostics::log(&format!(
            "memory wait trace #{count} address={address:#x} target={target:#x} value={value:#x} list_type={list_type} mid_list={mid_list}"
        ));
    }
}

/// Report a guest command that writes a waited-on address.
pub(super) fn trace_write(source: &str, address: u64, bytes: u64, value: u64) {
    let hit = WAITED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|(waited, _)| address < waited.saturating_add(8) && *waited < address.saturating_add(bytes));
    if hit {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count <= 32 || count.is_power_of_two() {
            crate::diagnostics::log(&format!(
                "memory wait write #{count} {source} address={address:#x} bytes={bytes:#x} value={value:#x}"
            ));
        }
    }
}

pub(super) unsafe extern "system" fn wait_value64(
    list: &List,
    address: u64,
    comparison: u32,
    target: u64,
    flags: u32,
) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) {
        return;
    }
    let result = (|| -> Result<()> {
        if comparison != 7 || flags > 1 {
            return Err(E_NOTIMPL.into());
        }
        let word = Word::map(list, address)?;
        if target == 0 {
            return Ok(());
        }
        let list_type = unsafe { list.native.GetType() }.0;
        trace_wait(address, target, word.value(), list_type, state.prefix.in_body);
        let split = !state.prefix.in_body;
        state.labels.push(Label { wait: Some(split), address, value: target });
        if state.prefix.in_body {
            // Honoring this would split the list and replay its bound state.
            // Order the work already in this list with a global UAV barrier and
            // continue: a value produced by another queue or the CPU is not
            // waited for, so later commands can read data before it is ready.
            // Failing the list would poison the queue and fail Present.
            if unsafe { list.native.GetType() } != D3D12_COMMAND_LIST_TYPE_COPY {
                let barrier = D3D12_RESOURCE_BARRIER {
                    Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                    Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                    Anonymous: D3D12_RESOURCE_BARRIER_0 {
                        UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                            pResource: std::mem::ManuallyDrop::new(None),
                        }),
                    },
                };
                // SAFETY: the list is open, and a null resource names every UAV.
                unsafe { list.native.ResourceBarrier(&[barrier]) };
            }
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || format!(
                "GPU memory wait {address:#x} >= {target:#x} after rendering commands replaced by a barrier"
            ));
            return Ok(());
        }
        state
            .prefix
            .segments
            .try_reserve(1)
            .map_err(|_| E_OUTOFMEMORY)?;
        let prefix = if state.prefix.pending.is_empty() {
            None
        } else {
            let mut device: Option<ID3D12Device> = None;
            unsafe {
                list.native.GetDevice(&mut device)?;
            }
            let device = device.ok_or(E_FAIL)?;
            let kind = unsafe { list.native.GetType() };
            let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(kind)? };
            let native: ID3D12GraphicsCommandList =
                unsafe { device.CreateCommandList(0, kind, &allocator, None)? };
            for packet in state.prefix.pending.drain(..) {
                packet.emit(&native)?;
            }
            unsafe {
                native.Close()?;
            }
            Some((native.into(), allocator))
        };
        state.prefix.segments.push(Arc::new(Segment {
            prefix,
            word,
            address,
            target,
        }));
        Ok(())
    })();
    if let Err(error) = result {
        crate::diagnostics::recording_error(format_args!(
            "64-bit GPU memory wait {address:#x} >= {target:#x} ({:?})",
            error.code()
        ));
        state.error = error.code();
    }
}

/// Waits the queue workers are polling right now: queue type, label, target.
static ACTIVE: std::sync::Mutex<Vec<(i32, u64, u64)>> = std::sync::Mutex::new(Vec::new());

fn active() -> std::sync::MutexGuard<'static, Vec<(i32, u64, u64)>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

struct Job {
    kind: i32,
    fence: ID3D12Fence,
    segment: Arc<Segment>,
    device: ID3D12Device,
    /// Unreleased label waits of the batch this one belongs to.
    waits: Option<crate::device::placement::held::Waits>,
}

pub(super) struct Worker(mpsc::Sender<Job>);

impl Worker {
    fn new(error: Arc<AtomicI32>) -> Result<Self> {
        let (sender, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("TuxBox GPU memory waits".into())
            .spawn(move || {
                for job in jobs {
                    let result = (|| -> Result<()> {
                        unsafe { job.fence.SetEventOnCompletion(1, HANDLE::default())? };
                        let started = std::time::Instant::now();
                        let held = (job.kind, job.segment.address, job.segment.target);
                        active().push(held);
                        while !job.segment.word.reached(job.segment.target) {
                            unsafe {
                                job.device.GetDeviceRemovedReason()?;
                            }
                            if started.elapsed() >= STALL {
                                // Nothing wrote the value, for example because
                                // the work that should have was dropped. A queue
                                // held forever trips the title's GPU watchdog.
                                report_stall(job.segment.address, job.segment.target, job.segment.word.value());
                                break;
                            }
                            pause(started.elapsed());
                        }
                        let mut waiting = active();
                        if let Some(at) = waiting.iter().position(|&w| w == held) {
                            waiting.swap_remove(at);
                        }
                        drop(waiting);
                        unsafe { job.fence.Signal(2)?; }
                        if let Some(waits) = &job.waits {
                            waits.fetch_sub(1, Ordering::AcqRel);
                        }
                        static FIRST: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&FIRST, || format!(
                            "GPU memory wait completed target={:#x}", job.segment.target
                        ));
                        Ok(())
                    })();
                    if let Err(cause) = result {
                        let _ = error.compare_exchange(
                            S_OK.0,
                            cause.code().0,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        // Fail the device rather than releasing work behind a broken gate.
                        if let Ok(device) = job.device.cast::<ID3D12Device5>() {
                            unsafe { device.RemoveDevice() };
                        }
                        break;
                    }
                }
            })
            .map_err(|_| E_OUTOFMEMORY)?;
        Ok(Self(sender))
    }
}

/// How long a wait is polled before the worker starts sleeping. Most labels
/// are written within this time, and a one-millisecond sleep would then be the
/// largest part of handing work from one queue to another.
const SPIN: Duration = Duration::from_millis(2);

/// Wait a moment before reading a label again.
fn pause(elapsed: Duration) {
    if elapsed < SPIN {
        std::thread::yield_now();
    } else {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Report a released wait: the first eight, then each power of two.
fn report_stall(address: u64, target: u64, value: u64) {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 8 || count.is_power_of_two() {
        crate::diagnostics::log(&format!(
            "GPU memory wait released after {}s without reaching its target (#{count}): address={address:#x} target={target:#x} value={value:#x}",
            STALL.as_secs()
        ));
    }
    if count == 1 {
        // The first stall shows the whole knot: every held queue and, for
        // each, the lists that should release it.
        super::queue_trace::report();
        let held = active().clone();
        for (kind, address, target) in held {
            crate::diagnostics::log(&format!(
                "held queue of type {kind}: waiting for {address:#x} >= {target:#x}"
            ));
            explain(address, target);
        }
    } else if count <= 8 {
        explain(address, target);
    }
}

pub(super) type Splits = Vec<(usize, Arc<Segment>)>;

pub(super) unsafe fn submit(
    queue: &Queue,
    host: &[*mut c_void],
    splits: Splits,
    waits: Option<crate::device::placement::held::Waits>,
) -> Result<()> {
    let run = |lists: &[*mut c_void]| {
        if !lists.is_empty() {
            unsafe {
                (queue.native.vtable().ExecuteCommandLists)(
                    queue.native.as_raw(),
                    lists.len() as u32,
                    lists.as_ptr(),
                )
            };
        }
    };
    let mut start = 0;
    for (index, segment) in splits {
        run(&host[start..index]);
        start = index;
        let mut device: Option<ID3D12Device> = None;
        unsafe {
            queue.native.GetDevice(&mut device)?;
        }
        let device = device.ok_or(E_FAIL)?;
        let fence = unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE)? };
        let worker = match queue.state.wait_worker.get() {
            Some(worker) => worker,
            None => {
                let new = Worker::new(queue.state.placement.error.clone())?;
                queue.state.wait_worker.get_or_init(|| new)
            }
        };
        if let Some((prefix, _)) = &segment.prefix {
            run(&[prefix.as_raw()]);
        }
        // Fence 1 retires the entire prefix before the CPU reads the guest word.
        // Fence 2 holds the suffix on the GPU without blocking the guest submission thread.
        unsafe {
            queue.native.Signal(&fence, 1)?;
        }
        worker
            .0
            .send(Job {
                kind: queue.state.kind.0,
                fence: fence.clone(),
                segment,
                device,
                waits: waits.clone(),
            })
            .map_err(|_| E_FAIL)?;
        unsafe {
            queue.native.Wait(&fence, 2)?;
            super::queue_trace::waited(&queue.native, "hold for a title fence label", &fence, 2);
        }
    }
    run(&host[start..]);
    Ok(())
}
