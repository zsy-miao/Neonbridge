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

/// Run `job` on the large-compile lane and return its result; on the
/// calling thread when already on the lane or the lane cannot start.
pub(crate) fn run_on_lane<R, F: FnOnce() -> R>(job: F) -> R {
    use std::sync::{Mutex, OnceLock, mpsc};
    static LANE: OnceLock<Option<Mutex<mpsc::Sender<LaneJob>>>> = OnceLock::new();
    if ON_LANE.with(Cell::get) {
        return job();
    }
    let lane = LANE.get_or_init(|| {
        let (send, receive) = mpsc::channel::<LaneJob>();
        std::thread::Builder::new()
            .name("tuxbox-compile-lane".into())
            .stack_size(STACK_BYTES)
            .spawn(move || {
                mark_big();
                ON_LANE.with(|l| l.set(true));
                for job in receive {
                    job();
                }
            })
            .ok()
            .map(|_| Mutex::new(send))
    });
    let Some(lane) = lane else { return run(job) };
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
    let sent = lane.lock().unwrap_or_else(|e| e.into_inner()).send(task).is_ok();
    if !sent || finished.recv().is_err() {
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
