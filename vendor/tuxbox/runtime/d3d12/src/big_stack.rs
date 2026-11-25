//! Host pipeline creation on a thread with a large stack (TuxBox c679f13).
//!
//! Proton's vkd3d-proton compiles DXIL to SPIR-V synchronously inside
//! Create*PipelineState, on the calling thread. Its control-flow analysis
//! recurses deeply on large translated shaders. Xbox titles create pipelines
//! on worker threads with 1 MiB stacks, so those threads overflowed their stack
//! inside the compiler and died.
use std::cell::Cell;

/// Reserved, not committed: untouched pages cost only address space.
pub(crate) const STACK_BYTES: usize = 64 << 20;

thread_local! {
    /// This thread already has a stack of `STACK_BYTES`.
    static BIG: Cell<bool> = const { Cell::new(false) };
}

/// Marks the calling thread, made with a `STACK_BYTES` stack, as needing no helper.
pub(crate) fn mark_big() {
    BIG.with(|big| big.set(true));
}

struct AssertSend<T>(T);
// SAFETY: D3D12 devices and the descriptors they read are free-threaded, and
// the caller blocks until the helper thread has finished with them.
unsafe impl<T> Send for AssertSend<T> {}

/// Run `job` on a scoped helper thread with a 64 MiB stack and return its
/// result. Runs on the current thread when it already has such a stack, or
/// if the helper cannot be created.
pub(crate) fn run<R, F: FnOnce() -> R>(job: F) -> R {
    if BIG.with(Cell::get) {
        return job();
    }
    let mut slot = Some(job);
    let pointer = AssertSend(&mut slot as *mut Option<F>);
    let result = std::thread::scope(|scope| {
        let spawned = std::thread::Builder::new()
            .name("tuxbox-pipeline".into())
            .stack_size(STACK_BYTES)
            .spawn_scoped(scope, move || {
                let pointer = pointer;
                mark_big();
                // SAFETY: the spawning thread waits for this scope to end
                // before reading `slot` again.
                let job = unsafe { (*pointer.0).take() }.expect("pipeline job already taken");
                AssertSend(job())
            });
        match spawned {
            Ok(handle) => Some(handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)).0),
            Err(_) => None,
        }
    });
    match result {
        Some(value) => value,
        None => (slot.take().expect("pipeline job already taken"))(),
    }
}

thread_local! {
    /// This thread is the large-compile lane.
    static ON_LANE: Cell<bool> = const { Cell::new(false) };
}

type LaneJob = Box<dyn FnOnce() + Send>;

/// Large driver compiles (256 KiB of DXIL or more) run on one long-lived
/// thread with a `STACK_BYTES` stack. The NVIDIA compiler keeps the arena
/// it grows for a compile (1-4 GB) with the thread that compiled: test156
/// measured +1258..+3761 MiB still held after each large compile on 12
/// pipeline and prefetch workers, and +0 MiB for one on a thread that
/// already held an arena, so private commit grew with every worker that
/// ever compiled a large program (free commit 628 MiB). One lane keeps
/// one arena. TUXBOX_COMPILE_LANE=0 compiles on the calling worker.
pub(crate) fn lane_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_COMPILE_LANE") != Some("0"))
}

/// Lanes for large compiles (TUXBOX_COMPILE_LANES, default min(4, cores/4),
/// at least 1; =1 is the single lane as before). test177: the one lane was
/// the top CPU thread from 56 s on (82-124 ms a frame) while 2772 pipelines
/// waited. Each lane keeps its own driver arena, so lanes stay few; 1 MiB
/// programs still compile one at a time (pipeline_cache::heavy_turn) and the
/// compile budget still admits each. A lane takes jobs a title waits on
/// before prefetches.
fn lanes() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        crate::env_toggle!("TUXBOX_COMPILE_LANES").and_then(|v| v.trim().parse().ok())
            .unwrap_or((cores / 4).min(4)).max(1)
    })
}

#[derive(Default)]
struct LaneQueue {
    urgent: std::collections::VecDeque<LaneJob>,
    prefetch: std::collections::VecDeque<LaneJob>,
}

/// Run `job` on a large-compile lane and return its result; on the
/// calling thread when already on a lane or no lane can start. `urgent`:
/// a title binds or waits on the pipeline (taken before prefetches).
pub(crate) fn run_on_lane<R, F: FnOnce() -> R>(job: F, urgent: bool) -> R {
    use std::sync::{Condvar, Mutex, OnceLock, mpsc};
    static QUEUE: (Mutex<LaneQueue>, Condvar) = (Mutex::new(LaneQueue { urgent: std::collections::VecDeque::new(), prefetch: std::collections::VecDeque::new() }), Condvar::new());
    static STARTED: OnceLock<bool> = OnceLock::new();
    if ON_LANE.with(Cell::get) {
        return job();
    }
    let started = *STARTED.get_or_init(|| {
        let n = lanes();
        let mut any = false;
        for i in 0..n {
            any |= std::thread::Builder::new()
                .name(if n == 1 { "tuxbox-compile-lane".into() } else { format!("tuxbox-compile-lane-{i}") })
                .stack_size(STACK_BYTES)
                .spawn(|| {
                    mark_big();
                    ON_LANE.with(|l| l.set(true));
                    loop {
                        let job = {
                            let (lock, ready) = &QUEUE;
                            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                            loop {
                                if let Some(job) = q.urgent.pop_front().or_else(|| q.prefetch.pop_front()) {
                                    break job;
                                }
                                q = ready.wait(q).unwrap_or_else(|e| e.into_inner());
                            }
                        };
                        job();
                    }
                })
                .is_ok();
        }
        crate::diagnostics::log_line(format_args!(
            "compile lanes: {n} for programs of 256 KiB or more, title-waited first (TUXBOX_COMPILE_LANES=N; 1 as before)"));
        any
    });
    if !started {
        return run(job);
    }
    let mut slot = Some(job);
    let mut result: Option<std::thread::Result<R>> = None;
    let pointers = AssertSend((&mut slot as *mut Option<F>, &mut result as *mut Option<std::thread::Result<R>>));
    let (done, finished) = mpsc::sync_channel::<()>(1);
    let task: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
        let pointers = pointers;
        // SAFETY: the caller blocks on `finished` until this has run.
        let job = unsafe { (*pointers.0.0).take() }.expect("lane job already taken");
        let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        unsafe { *pointers.0.1 = Some(value) };
        let _ = done.send(());
    });
    // SAFETY: the caller waits for the task to finish before `slot` and
    // `result` go out of scope, so the erased lifetime is never outlived.
    let task: LaneJob = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, LaneJob>(task) };
    {
        let (lock, ready) = &QUEUE;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if urgent { q.urgent.push_back(task) } else { q.prefetch.push_back(task) }
        drop(q);
        ready.notify_one();
    }
    if finished.recv().is_err() {
        if let Some(job) = slot.take() {
            return run(job);
        }
    }
    match result.take() {
        Some(Ok(value)) => value,
        Some(Err(panic)) => std::panic::resume_unwind(panic),
        None => panic!("compile lane dropped a job"),
    }
}
