//! Write32/64BitValueBottomOfPipeX into memory the GPU does not see: done by the CPU once the GPU
//! has finished the work before them.
//!
//! A console title marks its GPU progress with bottom-of-pipe writes into its own memory and
//! waits on them (a retail game: plain `VirtualAlloc` memory, outside XCL's unified memory).
//! The PC GPU cannot write there; dropped, the title waited for ever after its second frame.
//! Here each queue gets a fence of XCL's: after a submission whose lists carry such writes the
//! queue signals it, and a worker thread does the writes, in submission order, when the GPU is
//! there – what bottom of pipe means.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use windows::core::{Interface, Owned};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Threading::CreateEventW;

use crate::gpu::fence::{self, FenceError};

/// A bottom-of-pipe write into memory outside unified memory, applied by the CPU once the
/// queue's fence reaches the submission that recorded it.
#[derive(Clone)]
pub(super) enum Write {
    /// Immediate dwords to store starting at `va`.
    Immediate(usize, Vec<u32>),
    /// An immediate progress value, stored atomically after confirmed fence completion.
    ImmediateU64(usize, u64),
    /// A 64-bit value resolved by the GPU into a persistently mapped READBACK buffer slot
    /// (`src`), copied atomically to `va` once the queue's fence completes. Holding `_buf` keeps
    /// the mapping alive until the worker finishes.
    ReadbackU64 { va: usize, src: usize, _buf: ID3D12Resource },
}

struct Job {
    value: u64,
    writes: Vec<Write>,
}

/// Each queue has a fence and worker so a slow producer cannot block unrelated queues.
struct QueueFence {
    fence: ID3D12Fence,
    value: u64,
    tx: mpsc::Sender<Job>,
}

static FENCES: Mutex<Option<HashMap<usize, QueueFence>>> = Mutex::new(None);

/// The legacy fallback deadline. Progress writes still require confirmed completion.
const WAIT_MS: u32 = 10_000;

fn spawn_queue_worker(dev: ID3D12Device, fence: ID3D12Fence) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new().name("xcl-cpu-writes".into()).spawn(move || unsafe {
        let event = CreateEventW(None, false, false, None).ok().map(|event| Owned::new(event));
        let handle = event.as_deref().copied();
        for job in rx {
            let mut completed = fence::wait(&fence, job.value, &dev, handle, WAIT_MS);
            // A strict progress write holds the suffix. Only a legacy prefix can take the fallback.
            if matches!(&completed, Err(FenceError::Timeout(_)))
                && job.writes.first().is_some_and(|w| !matches!(w, Write::ImmediateU64(..)))
            {
                let value = fence.GetCompletedValue();
                if value < job.value {
                    static LATE: AtomicU64 = AtomicU64::new(0);
                    let k = LATE.fetch_add(1, Ordering::Relaxed) + 1;
                    if k.is_power_of_two() {
                        crate::xlog_warn!("D3D12.X: {k} CPU-write submissions using legacy fallback after {WAIT_MS} ms (fence at {value}, wanted {}; {} queued writes)", job.value, job.writes.len());
                    }
                }
            }
            for w in &job.writes {
                match w {
                    Write::Immediate(va, words) => write_now(*va, words),
                    Write::ImmediateU64(va, value) => {
                        if let Err(e) = &completed {
                            if !matches!(e, FenceError::Removed(_)) {
                                crate::xlog_warn!("D3D12.X: deferred 64-bit write at {va:#x}: {e}; still waiting for the producer");
                                // A timed-out event registration may still be active. Sample
                                // the fence directly, keeping later writes behind this one.
                                completed = loop {
                                    match fence::completed_value(&fence, &dev) {
                                        Ok(value) if value < job.value => std::thread::sleep(Duration::from_millis(20)),
                                        result => break result.map(|_| ()),
                                    }
                                };
                            }
                        }
                        match &completed {
                            Ok(()) => write_u64_now(*va, *value),
                            Err(e) => crate::xlog_error!("D3D12.X: deferred 64-bit write at {va:#x} did not complete: {e}"),
                        }
                    }
                    Write::ReadbackU64 { va, src, .. } => {
                        let value = (*(*src as *const AtomicU64)).load(Ordering::Acquire);
                        write_u64_now(*va, value);
                    }
                }
            }
        }
    });
    if let Err(e) = spawned {
        crate::xlog_error!("D3D12.X: no thread for the writes after the GPU: {e}");
    }
    tx
}

/// Disconnect the released queue's worker after its outstanding jobs drain.
pub(super) fn forget_queue(queue: usize) {
    if let Some(m) = FENCES.lock().unwrap().as_mut() {
        m.remove(&queue);
    }
}

/// The dwords at `va`, if the memory is still there and writable.
unsafe fn write_now(va: usize, words: &[u32]) {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_READWRITE};
    let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
    if VirtualQuery(va as *const c_void, &mut m, std::mem::size_of_val(&m)) == 0 || m.State != MEM_COMMIT {
        return;
    }
    if m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) == 0 || (m.BaseAddress as usize + m.RegionSize) < va + 4 * words.len() {
        return;
    }
    for (i, &w) in words.iter().enumerate() {
        (*((va + 4 * i) as *const AtomicU32)).store(w, Ordering::Release);
    }
}

/// An 8-byte aligned 64-bit value at `va`, if the memory is still there and writable.
unsafe fn write_u64_now(va: usize, value: u64) {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_READWRITE};
    let Some(end) = va.checked_add(8) else { return };
    let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
    if VirtualQuery(va as *const c_void, &mut m, std::mem::size_of_val(&m)) == 0 || m.State != MEM_COMMIT {
        return;
    }
    if m.Protect & PAGE_GUARD != 0 || m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) == 0 || (m.BaseAddress as usize).saturating_add(m.RegionSize) < end {
        return;
    }
    (*(va as *const AtomicU64)).store(value, Ordering::Release);
}

/// Whether `[va, va + bytes)` is memory the CPU can write for the GPU later.
pub(super) fn writable(va: usize, bytes: usize) -> bool {
    crate::hle::kernelx::lazy::ensure(va, bytes);
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_READWRITE};
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let found = unsafe { VirtualQuery(va as *const c_void, &mut m, std::mem::size_of_val(&m)) } != 0;
    found
        && m.State == MEM_COMMIT
        && m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) != 0
        && m.BaseAddress as usize + m.RegionSize >= va + bytes
}

/// `writes` of lists just submitted to `queue`: done when the GPU has finished them.
pub(super) unsafe fn after(queue: *mut c_void, writes: Vec<Write>) {
    if writes.is_empty() || queue.is_null() {
        return;
    }
    let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&queue) else { return };
    enqueue(q, writes, |fence, value| q.Signal(fence, value));
}

unsafe fn enqueue(
    q: &ID3D12CommandQueue, writes: Vec<Write>,
    signal: impl FnOnce(&ID3D12Fence, u64) -> windows::core::Result<()>,
) -> bool {
    let count = writes.len();
    let queued = {
        let mut g = FENCES.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        let entry = match m.entry(q.as_raw() as usize) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let mut dev: Option<ID3D12Device> = None;
                if q.GetDevice(&mut dev).is_err() {
                    return false;
                }
                let Some(dev) = dev else { return false };
                let Ok(fence) = dev.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) else { return false };
                let tx = spawn_queue_worker(dev, fence.clone());
                v.insert(QueueFence { fence, value: 0, tx })
            }
        };
        entry.value += 1;
        if signal(&entry.fence, entry.value).is_err() {
            return false;
        }
        // Keep fence signals and job sends in the same order under concurrent submission.
        entry.tx.send(Job { value: entry.value, writes }).is_ok()
    };
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| crate::xlog_info!("D3D12.X: bottom-of-pipe writes outside unified memory are done by the CPU after the GPU ({count} in the first submission)"));
    queued
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Dxgi::*;

    /// The value lands after the queue has done what came before it.
    #[test]
    fn writes_land_after_the_queue() {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                let factory: IDXGIFactory4 = CreateDXGIFactory1().unwrap();
                let warp: IDXGIAdapter = factory.EnumWarpAdapter().unwrap();
                D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).expect("WARP D3D12 device");
            }
            let dev = dev.unwrap();
            let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            let cell = Box::new([0u32; 2]);
            let va = cell.as_ptr() as usize;
            assert!(writable(va, 8));
            after(q.as_raw(), vec![Write::Immediate(va, vec![7, 9])]);
            let t = std::time::Instant::now();
            while (*(va as *const AtomicU32)).load(Ordering::Acquire) != 7 {
                assert!(t.elapsed().as_secs() < 5, "never written");
                std::thread::yield_now();
            }
            forget_queue(q.as_raw() as usize);
            assert_eq!(*cell, [7, 9]);
        }
    }

    #[test]
    fn write64_bottom_progress_waits_past_legacy_timeout() {
        use super::super::list_x;
        use std::time::{Duration, Instant};

        const EXPECTED: u64 = 0x1234_5678_9abc_def0;
        const DIRECT: D3D12_COMMAND_LIST_TYPE = D3D12_COMMAND_LIST_TYPE_DIRECT;
        const DRAIN: Duration = Duration::from_secs(5);
        let wait_for_marker = |cell: &AtomicU32, timeout: Duration| {
            let deadline = Instant::now() + timeout;
            while cell.load(Ordering::Acquire) != 1 {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            true
        };

        unsafe {
            let mut dev = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                crate::gpu::recomp::runner::skip_test("no GPU");
                return;
            }
            let dev: ID3D12Device = dev.unwrap();
            let queue = || -> ID3D12CommandQueue {
                dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: DIRECT, ..Default::default() }).unwrap()
            };
            let (producer, unrelated) = (queue(), queue());
            let alloc: ID3D12CommandAllocator = dev.CreateCommandAllocator(DIRECT).unwrap();
            let list: ID3D12GraphicsCommandList = dev.CreateCommandList(0, DIRECT, &alloc, None).unwrap();
            let cells = Box::into_raw(Box::new((AtomicU32::new(0), AtomicU64::new(0), AtomicU32::new(0), AtomicU32::new(0))));
            let (prefix, progress, tail, unrelated_marker) = &*cells;
            let progress_va = progress.as_ptr() as u64;
            list_x::on_create(list.as_raw(), alloc.as_raw() as usize, 0, DIRECT);
            list_x::write32_bottom_x(list.as_raw(), prefix.as_ptr() as u64, 1, 0, 0);
            list_x::write64_bottom_x(list.as_raw(), progress_va, EXPECTED, 0, 0);
            list_x::write32_bottom_x(list.as_raw(), tail.as_ptr() as u64, 1, 0, 0);
            let writes = list_x::take_cpu_writes_for_test(list.as_raw(), progress_va);

            // Replace only the queue's completion signal: this fence is held by the CPU, so
            // the real worker can hit its timeout without stalling a GPU queue.
            let mut pending = None;
            let queued = enqueue(&producer, writes, |fence, value| {
                pending = Some((fence.clone(), value));
                Ok(())
            });
            let timed_out = queued && wait_for_marker(prefix, Duration::from_millis(WAIT_MS.into()) + DRAIN);
            let unrelated_queued = enqueue(&unrelated, vec![Write::Immediate(unrelated_marker.as_ptr() as usize, vec![1])], |fence, value| unrelated.Signal(fence, value));
            let isolated = unrelated_queued && wait_for_marker(unrelated_marker, DRAIN);
            // The legacy path writes two dwords. Acquire the following marker before reading
            // them as u64, so the observation cannot race those differently sized stores.
            let before = (timed_out && wait_for_marker(tail, Duration::from_secs(1)))
                .then(|| progress.load(Ordering::Acquire));
            let held = pending.as_ref().map(|(f, _)| fence::completed_value(f, &dev));
            let released = pending.map(|(f, value)| f.Signal(value));
            let drained = queued && wait_for_marker(tail, DRAIN);
            let unrelated_drained = isolated || (unrelated_queued && wait_for_marker(unrelated_marker, DRAIN));
            let after = drained.then(|| progress.load(Ordering::Acquire));
            forget_queue(producer.as_raw() as usize);
            forget_queue(unrelated.as_raw() as usize);
            // After a failed drain, a worker may still hold these addresses.
            if (!queued || drained) && (!unrelated_queued || unrelated_drained) {
                drop(Box::from_raw(cells));
            }

            assert!(queued, "queue deferred writes");
            assert!(timed_out, "legacy prefix write did not witness the timeout fallback");
            assert!(matches!(held, Some(Ok(0))), "producer fence did not stay pending: {held:?}");
            released.expect("producer fence was captured").expect("release the producer fence");
            assert_eq!(before, None, "progress or its following write landed before fence completion");
            assert_eq!(after, Some(EXPECTED), "progress did not land in order after the fence signaled");
            assert!(isolated, "an unrelated queue's write did not land while the producer was held");
        }
    }

    /// A ReadbackU64 write copies the 64-bit value from the mapped readback buffer after the queue finishes.
    #[test]
    fn readback_u64_lands_after_the_queue() {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory4>() else {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                };
                let Ok(warp) = factory.EnumWarpAdapter::<IDXGIAdapter>() else {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                };
                if D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                }
            }
            let dev = dev.unwrap();
            let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            let hp = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
            let rd = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: 8,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            };
            let mut rb: Option<ID3D12Resource> = None;
            dev.CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &rd, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut rb).unwrap();
            let rb = rb.unwrap();
            let mut ptr: *mut c_void = std::ptr::null_mut();
            rb.Map(0, None, Some(&mut ptr)).unwrap();
            let expected = 0x1234_5678_9ABC_DEF0u64;
            (*(ptr as *const AtomicU64)).store(expected, Ordering::Release);

            let dst = Box::new(0u64);
            let va = dst.as_ref() as *const u64 as usize;
            assert!(writable(va, 8));
            after(q.as_raw(), vec![Write::ReadbackU64 { va, src: ptr as usize, _buf: rb }]);
            let t = std::time::Instant::now();
            while (*(va as *const AtomicU64)).load(Ordering::Acquire) != expected {
                assert!(t.elapsed().as_secs() < 5, "readback value never written");
                std::thread::yield_now();
            }
            forget_queue(q.as_raw() as usize);
            assert_eq!(*dst, expected);
        }
    }
}
