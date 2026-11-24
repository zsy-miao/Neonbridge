//! Driver compiles admitted by the memory they are expected to take. Ported
//! from five-star-recomp afdd699 (GPL-3.0, CompileBudget.cs).
//!
//! The NVIDIA compiler grows a doubling arena per pipeline (1, 2, 4, 8 GB)
//! for minutes; five-star saw six large mesh-shader pipelines started together
//! take 50 GB. Each compile is expected to take 4096 bytes per byte of DXIL,
//! clamped to 2..32 GB. A new compile starts only while free physical memory
//! and commit cover the reserves (4 GB physical, 8 GB commit), its estimate
//! and what the running compiles are still expected to take (their estimates
//! less the process's private growth since compiles began). Compiles of less
//! than 256 KiB of DXIL are counted but never wait. Unlike five-star, a
//! compile goes ahead after TUXBOX_COMPILE_BUDGET_WAIT_MS (30000) so a machine
//! that never frees enough cannot hold a pipeline forever.
//! TUXBOX_COMPILE_BUDGET=0 never waits.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const TINY_SHADER_BYTES: u64 = 256 << 10;
const BYTES_PER_SHADER_BYTE: u64 = 4096;
const MINIMUM_ESTIMATE: u64 = 2 << 30;
const MAXIMUM_ESTIMATE: u64 = 32 << 30;
const PHYSICAL_RESERVE: u64 = 4 << 30;
const COMMIT_RESERVE: u64 = 8 << 30;
const PAUSE_MS: u64 = 500;

struct State {
    running: u32,
    expected: u64,
    baseline: u64,
}

static STATE: Mutex<State> = Mutex::new(State { running: 0, expected: 0, baseline: 0 });
static WAITING: AtomicU64 = AtomicU64::new(0);
static WAITED: AtomicU64 = AtomicU64::new(0);

fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_COMPILE_BUDGET") != Some("0")
}

fn max_wait() -> std::time::Duration {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    std::time::Duration::from_millis(*MS.get_or_init(|| {
        std::env::var("TUXBOX_COMPILE_BUDGET_WAIT_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(30000)
    }))
}

fn estimate(shader_bytes: u64) -> u64 {
    shader_bytes.saturating_mul(BYTES_PER_SHADER_BYTE).clamp(MINIMUM_ESTIMATE, MAXIMUM_ESTIMATE)
}

/// Free physical memory and commit headroom of the machine.
fn available() -> (u64, u64) {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return (u64::MAX, u64::MAX);
    }
    (status.ullAvailPhys, status.ullAvailPageFile)
}

fn try_enter(estimate: u64, tiny: bool) -> bool {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let private = crate::diagnostics::memory::private_bytes();
    if state.running == 0 {
        state.baseline = private;
    }
    let remaining = state.expected.saturating_sub(private.saturating_sub(state.baseline));
    if !tiny {
        let need = estimate.saturating_add(remaining);
        let (physical, commit) = available();
        if physical < PHYSICAL_RESERVE.saturating_add(need) || commit < COMMIT_RESERVE.saturating_add(need) {
            return false;
        }
    }
    state.running += 1;
    state.expected += estimate;
    true
}

/// A compile admitted by the budget; leaving it ends the compile.
pub(super) struct Admitted(u64);

impl Drop for Admitted {
    fn drop(&mut self) {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.running = state.running.saturating_sub(1);
        state.expected = state.expected.saturating_sub(self.0);
    }
}

/// Waits until a compile of `shader_bytes` of DXIL may start.
pub(super) fn enter(shader_bytes: u64) -> Option<Admitted> {
    if !enabled() {
        return None;
    }
    let estimate = estimate(shader_bytes);
    let tiny = shader_bytes < TINY_SHADER_BYTES;
    let mut waited = None;
    while !try_enter(estimate, tiny || waited.is_some_and(|s: std::time::Instant| s.elapsed() >= max_wait())) {
        if waited.is_none() {
            WAITING.fetch_add(1, Ordering::Relaxed);
            waited = Some(std::time::Instant::now());
        }
        std::thread::sleep(std::time::Duration::from_millis(PAUSE_MS));
    }
    if let Some(start) = waited {
        WAITING.fetch_sub(1, Ordering::Relaxed);
        let n = WAITED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            let (physical, commit) = available();
            crate::diagnostics::log_line(format_args!(
                "compile budget: a compile of {} KiB of DXIL (expected {} MiB) waited {} ms for memory (gives up after TUXBOX_COMPILE_BUDGET_WAIT_MS); {} MiB physical and {} MiB commit free now, {} still waiting ({n} waited so far; TUXBOX_COMPILE_BUDGET=0 off)",
                shader_bytes >> 10, estimate >> 20, start.elapsed().as_millis(), physical >> 20, commit >> 20,
                WAITING.load(Ordering::Relaxed)));
        }
    }
    Some(Admitted(estimate))
}
