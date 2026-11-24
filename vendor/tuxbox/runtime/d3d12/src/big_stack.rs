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
