//! Fence #12 of the newer console layout (a retail game): the address of the fence's completed value,
//! which the title reads and sleeps on (`monitorx` / `mwaitx`). On the console the GPU writes
//! that memory; a PC fence's value is only behind GetCompletedValue, so each fence asked gets a
//! cell of its own that a thread refreshes on queue-signal events and on a timer for rewinds.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::core::Interface;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::ID3D12Fence;
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading as th;

const REWIND_POLL_MS: u32 = 10;

/// a cache line of its own: the title's monitors wake on writes to the line
#[repr(C, align(64))]
struct Cell(AtomicU64);

#[derive(Default)]
struct Fence {
    cell: Option<&'static Cell>,
    /// the highest value a queue was told to signal
    signalled: u64,
    /// the highest value a queue was told to wait for
    waited: u64,
}

/// (fences are never freed: XCL holds a reference to each)
static FENCES: Mutex<Option<HashMap<usize, Fence>>> = Mutex::new(None);

/// the event a completion sets (auto-reset); the first use starts the thread that follows it
fn event() -> Option<usize> {
    static E: OnceLock<Option<usize>> = OnceLock::new();
    *E.get_or_init(|| unsafe {
        let e = th::CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) as usize;
        if e == 0 {
            crate::xlog_error!("D3D12.X: could not create the fence completion event");
            return None;
        }
        let spawned = std::thread::Builder::new().name("xcl-fences".into()).spawn(move || loop {
            // SetEventOnCompletion is a threshold notification. A lower queued value
            // may already satisfy it before the GPU has reached that signal.
            match th::WaitForSingleObject(e as _, REWIND_POLL_MS) {
                WAIT_OBJECT_0 => refresh(true),
                WAIT_TIMEOUT => refresh(false),
                status => {
                    crate::xlog_error!("D3D12.X: fence completion wait failed ({status:#x})");
                    break;
                }
            }
        });
        if let Err(err) = spawned {
            crate::xlog_error!("D3D12.X: no thread for the fences' completed values: {err}");
            CloseHandle(e as _);
            return None;
        }
        Some(e)
    })
}

unsafe fn borrowed<R>(fence: usize, f: impl FnOnce(&ID3D12Fence) -> R) -> Option<R> {
    let p = fence as *mut c_void;
    ID3D12Fence::from_raw_borrowed(&p).map(f)
}

fn refresh(write_unchanged: bool) {
    let g = FENCES.lock().unwrap_or_else(|e| e.into_inner());
    for (&p, f) in g.iter().flatten() {
        if let Some(c) = f.cell {
            if let Some(v) = unsafe { borrowed(p, |f| f.GetCompletedValue()) } {
                // Preserve wakes on a signal. An idle timer tick must only wake the
                // title's cache-line monitor if the fence has actually changed.
                if write_unchanged || c.0.load(Ordering::Relaxed) != v {
                    c.0.store(v, Ordering::Release);
                }
            }
        }
    }
}

unsafe fn arm(fence: usize, value: u64) {
    let Some(event) = event() else {
        return;
    };
    if let Some(Err(e)) = borrowed(fence, |f| f.SetEventOnCompletion(value, HANDLE(event as *mut c_void))) {
        crate::xlog_warn!("D3D12.X: fence {fence:#x}: SetEventOnCompletion({value:#x}): {e}");
    }
}

/// A queue was told to signal `fence` (one of the title's) with `value`.
pub(super) unsafe fn note_queue_signal(fence: usize, value: u64) {
    let cell = {
        let mut g = FENCES.lock().unwrap_or_else(|e| e.into_inner());
        let f = g.get_or_insert_with(HashMap::new).entry(fence).or_default();
        f.signalled = f.signalled.max(value);
        f.cell.is_some()
    };
    if cell {
        arm(fence, value);
    }
}

/// A queue was told to wait for `fence` (one of the title's) to reach `value`.
pub(super) fn note_queue_wait(fence: usize, value: u64) {
    let mut g = FENCES.lock().unwrap_or_else(|e| e.into_inner());
    let f = g.get_or_insert_with(HashMap::new).entry(fence).or_default();
    f.waited = f.waited.max(value);
}

/// The CPU signalled `fence`.
pub(super) fn note_cpu_signal(fence: usize) {
    let known = FENCES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.get(&fence).is_some_and(|f| f.cell.is_some()));
    if known {
        refresh(true);
    }
}

/// Diagnostic summary of title fences whose completed value lags their signalled or waited value.
pub(super) fn pending_summary() -> String {
    let g = FENCES.lock().unwrap_or_else(|e| e.into_inner());
    let mut parts = Vec::new();
    for (&p, f) in g.iter().flatten() {
        let done = unsafe { borrowed(p, |f| f.GetCompletedValue()) }.unwrap_or(0);
        if done < f.signalled || done < f.waited {
            parts.push(format!("{p:#x}(done={done:#x}/sig={:#x}/wait={:#x},cell={})", f.signalled, f.waited, f.cell.is_some()));
        }
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(", ")
    }
}

/// fence #12: the address of its completed value
pub(super) unsafe extern "system" fn completed_value_address(this: *mut c_void) -> *const u64 {
    let (cell, pending) = {
        let mut g = FENCES.lock().unwrap_or_else(|e| e.into_inner());
        let f = g.get_or_insert_with(HashMap::new).entry(this as usize).or_default();
        match f.cell {
            Some(c) => return c.0.as_ptr(),
            None => {
                let c: &'static Cell = Box::leak(Box::new(Cell(AtomicU64::new(0))));
                f.cell = Some(c);
                (c, f.signalled)
            }
        }
    };
    if pending > 0 {
        arm(this as usize, pending);
    } else {
        // A signal to zero before the title asked for the address still needs polling.
        event();
    }
    refresh(true);
    cell.0.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::*;

    unsafe fn device() -> Option<ID3D12Device> {
        let mut dev = None;
        if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_ok() {
            return dev;
        }
        let factory: IDXGIFactory4 = CreateDXGIFactory1().ok()?;
        let warp: IDXGIAdapter = factory.EnumWarpAdapter().ok()?;
        D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).ok()?;
        dev
    }

    /// The cell follows the fence: CPU signals at once, queue signals when the GPU is there –
    /// also one queued before the address was asked for.
    #[test]
    fn the_cell_follows_the_fence() {
        unsafe {
            let Some(dev) = device() else {
                crate::gpu::recomp::runner::skip_test("no D3D12 device");
                return;
            };
            let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            let fence: ID3D12Fence = dev.CreateFence(5, D3D12_FENCE_FLAG_NONE).unwrap();
            let p = fence.as_raw();
            let wait = |cell: *const u64, v: u64| {
                let t = std::time::Instant::now();
                while (*(cell as *const AtomicU64)).load(Ordering::Acquire) != v {
                    assert!(t.elapsed().as_secs() < 5, "cell never became {v}");
                    std::thread::yield_now();
                }
            };
            queue.Signal(&fence, 6).unwrap();
            note_queue_signal(p as usize, 6);
            let cell = completed_value_address(p);
            assert_eq!(cell, completed_value_address(p));
            wait(cell, 6);
            queue.Signal(&fence, 7).unwrap();
            note_queue_signal(p as usize, 7);
            wait(cell, 7);
            fence.Signal(9).unwrap();
            note_cpu_signal(p as usize);
            assert_eq!((*cell.cast::<AtomicU64>()).load(Ordering::Acquire), 9);
            std::mem::forget(fence); // the map keeps its address
        }
    }

    #[test]
    fn the_cell_follows_a_delayed_queue_rewind() {
        unsafe {
            let Some(dev) = device() else {
                crate::gpu::recomp::runner::skip_test("no D3D12 device");
                return;
            };
            let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            }).unwrap();
            let gate: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
            let fence: &ID3D12Fence = Box::leak(Box::new(
                dev.CreateFence(9, D3D12_FENCE_FLAG_NONE).unwrap(),
            ));
            let cell = &*completed_value_address(fence.as_raw()).cast::<AtomicU64>();

            // Change the native fence without refreshing the cell here. The cell reaching
            // 40 tells us the worker refreshed while the queue was still held at the gate.
            fence.Signal(40).unwrap();
            queue.Wait(&gate, 1).unwrap();
            let signal = queue.Signal(fence, 3);
            if signal.is_ok() {
                note_queue_signal(fence.as_raw() as usize, 3);
            }
            let wait = |expected| {
                let start = std::time::Instant::now();
                loop {
                    let value = cell.load(Ordering::Acquire);
                    if value == expected || start.elapsed().as_secs() >= 5 {
                        return value;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            };
            let before_rewind = wait(40);
            gate.Signal(1).unwrap();
            signal.unwrap();
            assert_eq!(before_rewind, 40, "the worker did not refresh before the queue rewind");
            assert_eq!(wait(3), 3, "stale cell after queue rewind, native fence {}", fence.GetCompletedValue());
        }
    }
}
