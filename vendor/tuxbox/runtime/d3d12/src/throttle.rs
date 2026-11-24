//! Present throttle: the title may run at most TUXBOX_PRESENT_LAG presents
//! ahead of the GPU, across every queue. Ported from five-star-recomp
//! 4cc3094 (GPL-3.0, GpuFrameThrottle.cs).
//!
//! test89 sat 4 s of GPU latency behind the title with device-local memory at
//! 121% of budget. The swap chain only waited for its own back buffer, about
//! three frames deep on the presenting queue, and compute and copy queues ran
//! ahead freely. At each present, every queue gets a fence signal (the work it
//! has so far). The present then waits, for at most TUXBOX_PRESENT_WAIT_MS per
//! queue, until each queue's mark from LAG presents back is reached.
//! TUXBOX_PRESENT_LAG=0 turns it off.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Fence, D3D12_FENCE_FLAG_NONE};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const MAX_QUEUES: usize = 32;
const MAX_LAG: usize = 8;

struct Tracked {
    queue: ID3D12CommandQueue,
    fence: Option<ID3D12Fence>,
    signaled: u64,
    /// Fence values at the last MAX_LAG presents, newest last.
    marks: [u64; MAX_LAG],
}

static QUEUES: Mutex<Vec<Tracked>> = Mutex::new(Vec::new());
static WAITS: AtomicU64 = AtomicU64::new(0);
static GAVE_UP: AtomicU64 = AtomicU64::new(0);
static PRESENTS: AtomicU64 = AtomicU64::new(0);

fn lag() -> usize {
    static LAG: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LAG.get_or_init(|| std::env::var("TUXBOX_PRESENT_LAG").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(2).min(MAX_LAG))
}

fn wait_ms() -> u32 {
    static MS: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MS.get_or_init(|| std::env::var("TUXBOX_PRESENT_WAIT_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(5000))
}

/// A new guest queue: throttled from its first present on.
pub(crate) fn follow(queue: &ID3D12CommandQueue) {
    if lag() == 0 {
        return;
    }
    let mut queues = QUEUES.lock().unwrap_or_else(|e| e.into_inner());
    if queues.len() < MAX_QUEUES && !queues.iter().any(|t| t.queue == *queue) {
        queues.push(Tracked { queue: queue.clone(), fence: None, signaled: 0, marks: [0; MAX_LAG] });
    }
}

/// Called before each present. It marks every queue and waits until each
/// queue's mark from lag() presents back is reached.
pub(crate) fn presented() {
    let lag = lag();
    if lag == 0 {
        return;
    }
    let pending: Vec<(ID3D12Fence, u64)> = {
        let mut queues = QUEUES.lock().unwrap_or_else(|e| e.into_inner());
        queues.iter_mut().filter_map(|t| {
            if t.fence.is_none() {
                let mut device: Option<ID3D12Device> = None;
                unsafe { t.queue.GetDevice(&mut device) }.ok()?;
                t.fence = unsafe { device?.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok();
            }
            let fence = t.fence.clone()?;
            if unsafe { t.queue.Signal(&fence, t.signaled + 1) }.is_ok() {
                t.signaled += 1;
            }
            t.marks.rotate_left(1);
            t.marks[MAX_LAG - 1] = t.signaled;
            let target = t.marks[MAX_LAG - 1 - lag];
            (target != 0).then_some((fence, target))
        }).collect()
    };
    let n = PRESENTS.fetch_add(1, Ordering::Relaxed) + 1;
    for (fence, value) in pending {
        if unsafe { fence.GetCompletedValue() } >= value {
            continue;
        }
        WAITS.fetch_add(1, Ordering::Relaxed);
        if !wait_on(&fence, value, wait_ms()) {
            let g = GAVE_UP.fetch_add(1, Ordering::Relaxed) + 1;
            if g <= 8 || g.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "present throttle: a queue was still behind its mark from {lag} presents back after {} ms; going on ({g} so far)", wait_ms()));
            }
        }
    }
    if n.is_power_of_two() && n >= 64 {
        crate::diagnostics::log_line(format_args!(
            "present throttle: {n} presents, {} waits, {} gave up (lag {lag}; TUXBOX_PRESENT_LAG=0 off)",
            WAITS.load(Ordering::Relaxed), GAVE_UP.load(Ordering::Relaxed)));
    }
}

fn wait_on(fence: &ID3D12Fence, value: u64, ms: u32) -> bool {
    let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else { return false };
    let reached = unsafe { fence.SetEventOnCompletion(value, event) }.is_ok()
        && unsafe { WaitForSingleObject(event, ms) } == WAIT_OBJECT_0;
    let _ = unsafe { CloseHandle(event) };
    reached
}
