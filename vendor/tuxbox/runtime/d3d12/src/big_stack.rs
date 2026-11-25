//! Host pipeline creation on a thread with a large stack.
//!
//! Proton's vkd3d-proton compiles DXIL to SPIR-V synchronously inside
//! Create*PipelineState, on the calling thread. Its control-flow analysis
//! recurses deeply on large translated shaders. Xbox titles create pipelines
//! on worker threads with 1 MiB stacks, so those threads overflowed their stack
//! inside the compiler and died (ProjectScarlett stayed on a black screen).
//!
//! A cold NVIDIA disk cache then spends minutes inside that same call, turning
//! DXIL into a GPU binary. An untranslated compute shader waits a bounded time
//! and, if the driver is still compiling, returns so the title can substitute
//! a null pipeline. A translated shader waits for the real pipeline: a null
//! stand-in has no descriptor-heap layout, and the title's dispatch is then
//! dropped. The compile keeps the device, root signature, and shader bytes.

/// Reserved, not committed: untouched pages cost only address space.
const STACK_BYTES: usize = 64 << 20;

/// `0` means the cap has not been chosen yet. Tests store a small cap here.
static MAX_LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct AssertSend<T>(T);
// SAFETY: D3D12 devices and the descriptors they read are free-threaded, and
// the caller blocks until the helper thread has finished with them.
unsafe impl<T> Send for AssertSend<T> {}

/// Run `job` on a scoped helper thread with a 64 MiB stack and return its
/// result. Falls back to the current thread if the helper cannot be created.
///
/// `label` is reported while `CreatePipelineState` is still inside the driver.
/// Cached creates return in milliseconds and stay quiet. A compile that is
/// still going after five seconds is logged, then again every fifteen seconds,
/// because that call produces no other log line.
pub(crate) fn run_logged<R, F: FnOnce() -> R>(label: &str, job: F) -> R {
    let _watch = PipelineWatch::start(label);
    let mut slot = Some(job);
    let pointer = AssertSend(&mut slot as *mut Option<F>);
    let result = std::thread::scope(|scope| {
        let spawned = std::thread::Builder::new()
            .name("tuxbox-pipeline".into())
            .stack_size(STACK_BYTES)
            .spawn_scoped(scope, move || {
                let pointer = pointer;
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

/// The compile finished, or it is still running after the deadline.
pub(crate) enum PipelineWait<T> {
    Ready(T),
    /// The title can substitute a null pipeline. The driver compile continues
    /// on a helper thread and still records its GPU binary.
    Deferred,
}

/// Wait up to `TUXBOX_PIPELINE_DEADLINE` seconds (default 20). `0` waits until
/// the driver returns. Past the deadline the helper is detached, not cancelled.
pub(crate) fn run_deadline<R, F>(label: &str, job: F) -> PipelineWait<R>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let deadline = pipeline_deadline();
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();
    if deadline.is_zero() {
        crate::diagnostics::log_once(&ANNOUNCED, || {
            "compute pipelines wait for the driver compile".to_string()
        });
    } else {
        crate::diagnostics::log_once(&ANNOUNCED, || {
            format!(
                "compute pipelines wait {}s, then return a null pipeline while the driver compile continues ({} at a time)",
                deadline.as_secs(),
                max_live()
            )
        });
    }
    run_for(label, deadline, job)
}

fn pipeline_deadline() -> std::time::Duration {
    static DEADLINE: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *DEADLINE.get_or_init(|| parse_deadline(std::env::var("TUXBOX_PIPELINE_DEADLINE").ok().as_deref()))
}

fn parse_deadline(text: Option<&str>) -> std::time::Duration {
    match text {
        None => std::time::Duration::from_secs(20),
        Some(text) => match text.parse::<u64>() {
            Ok(seconds) => std::time::Duration::from_secs(seconds),
            Err(_) => std::time::Duration::from_secs(20),
        },
    }
}

fn max_live() -> usize {
    let current = MAX_LIVE.load(std::sync::atomic::Ordering::Relaxed);
    if current != 0 {
        return current;
    }
    // Each live compile reserves a 64 MiB stack and occupies a driver thread.
    // One per CPU, capped, so a title that keeps submitting cannot pile up
    // hundreds of detached compiles.
    let chosen = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).clamp(2, 16);
    match MAX_LIVE.compare_exchange(0, chosen, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire) {
        Ok(_) => chosen,
        Err(existing) => existing,
    }
}

fn run_for<R, F>(label: &str, deadline: std::time::Duration, job: F) -> PipelineWait<R>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    if deadline.is_zero() {
        return PipelineWait::Ready(run_logged(label, job));
    }
    if !reserve_slot() {
        enqueue(label, job);
        // A slot may have freed between the reservation attempt and the push.
        on_slot_released();
        note_deferred(label, None);
        return PipelineWait::Deferred;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(Some(job)));
    let for_thread = std::sync::Arc::clone(&shared);
    let label_owned = label.to_string();
    // Owned by the closure so a failed spawn releases the reservation.
    let slot = Slot;
    let spawned = spawn_pipeline(move || {
        let _slot = slot;
        let _watch = PipelineWatch::start(&label_owned);
        let job = take_job(&for_thread);
        let value = job();
        // The caller may already have dropped the receiver. That is the deadline.
        let _ = tx.send(value);
    });
    let handle = match spawned {
        Ok(handle) => handle,
        Err(_) => return PipelineWait::Ready(run_logged(label, take_job(&shared))),
    };
    match rx.recv_timeout(deadline) {
        Ok(value) => {
            join_or_resume(handle);
            PipelineWait::Ready(value)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => match rx.try_recv() {
            Ok(value) => {
                join_or_resume(handle);
                PipelineWait::Ready(value)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                drop(rx);
                note_deferred(label, Some(deadline));
                // Detach. The helper keeps running until the driver returns.
                drop(handle);
                PipelineWait::Deferred
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                join_or_resume(handle);
                panic!("pipeline compile ended without a result");
            }
        },
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            join_or_resume(handle);
            panic!("pipeline compile ended without a result");
        }
    }
}

fn note_deferred(label: &str, waited: Option<std::time::Duration>) {
    match waited {
        None => crate::diagnostics::log(&format!("CreatePipelineState {label} queued; compile continues")),
        Some(waited) => crate::diagnostics::log(&format!(
            "CreatePipelineState {label} deferred after {}s; compile continues",
            waited.as_secs()
        )),
    }
}

fn enqueue<R, F>(label: &str, job: F)
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let label = label.to_string();
    queue().push_back(Queued {
        label,
        job: Box::new(move || {
            let _ = job();
        }),
    });
}

fn spawn_pipeline<F>(job: F) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .name("tuxbox-pipeline".into())
        .stack_size(STACK_BYTES)
        .spawn(job)
}

fn take_job<T>(slot: &std::sync::Mutex<Option<T>>) -> T {
    slot.lock().unwrap_or_else(|error| error.into_inner()).take().expect("pipeline job already taken")
}

fn join_or_resume(handle: std::thread::JoinHandle<()>) {
    if let Err(panic) = handle.join() {
        std::panic::resume_unwind(panic);
    }
}

struct Queued {
    label: String,
    job: Box<dyn FnOnce() + Send>,
}

fn queue() -> std::sync::MutexGuard<'static, std::collections::VecDeque<Queued>> {
    static QUEUE: std::sync::Mutex<std::collections::VecDeque<Queued>> =
        std::sync::Mutex::new(std::collections::VecDeque::new());
    QUEUE.lock().unwrap_or_else(|error| error.into_inner())
}

fn reserve_slot() -> bool {
    let live = live_counter();
    let mut current = live.load(std::sync::atomic::Ordering::Relaxed);
    loop {
        if current >= max_live() {
            return false;
        }
        match live.compare_exchange_weak(
            current,
            current + 1,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

fn live_counter() -> &'static std::sync::atomic::AtomicUsize {
    static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    &LIVE
}

#[cfg(test)]
fn inflight() -> usize {
    live_counter().load(std::sync::atomic::Ordering::Acquire)
}

/// Releases one reserved compile slot and starts a queued compile if one fits.
struct Slot;

impl Drop for Slot {
    fn drop(&mut self) {
        live_counter().fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        on_slot_released();
    }
}

/// Stops a failed `spawn` from recursively trying to spawn again.
static PUMP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn on_slot_released() {
    if PUMP.fetch_add(1, std::sync::atomic::Ordering::AcqRel) != 0 {
        PUMP.fetch_sub(1, std::sync::atomic::Ordering::Release);
        return;
    }
    let look_again = start_one_queued();
    PUMP.fetch_sub(1, std::sync::atomic::Ordering::Release);
    // `start_one_queued` may have observed an empty queue just before a push.
    if look_again && live_counter().load(std::sync::atomic::Ordering::Acquire) < max_live() && !queue().is_empty() {
        on_slot_released();
    }
}

/// `true` when the caller should check the queue again. `false` when a helper
/// thread could not be created, so retrying would recurse.
fn start_one_queued() -> bool {
    if !reserve_slot() {
        return true;
    }
    let Some(queued) = queue().pop_front() else {
        live_counter().fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        return true;
    };
    let shared = std::sync::Arc::new(std::sync::Mutex::new(Some(queued)));
    let for_thread = std::sync::Arc::clone(&shared);
    let slot = Slot;
    match spawn_pipeline(move || {
        let _slot = slot;
        let queued = take_job(&for_thread);
        let _watch = PipelineWatch::start(&queued.label);
        (queued.job)();
    }) {
        Ok(handle) => {
            drop(handle);
            true
        }
        Err(error) => {
            crate::diagnostics::log(&format!("pipeline compile thread was not created: {error}"));
            if let Some(queued) = shared.lock().unwrap_or_else(|error| error.into_inner()).take() {
                queue().push_front(queued);
            }
            false
        }
    }
}

struct Flight {
    id: u64,
    label: String,
    started: std::time::Instant,
    /// Seconds from the start at which the next progress line is due.
    next_due: u64,
}

struct PipelineWatch {
    id: Option<u64>,
    label: String,
    started: std::time::Instant,
}

impl PipelineWatch {
    fn start(label: &str) -> Self {
        if !crate::diagnostics::logging() {
            return Self { id: None, label: String::new(), started: std::time::Instant::now() };
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        static WATCH: std::sync::Once = std::sync::Once::new();
        WATCH.call_once(|| {
            let _ = std::thread::Builder::new().name("tuxbox-pso-log".into()).spawn(watch);
        });
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let started = std::time::Instant::now();
        flights().push(Flight { id, label: label.to_string(), started, next_due: 5 });
        Self { id: Some(id), label: label.to_string(), started }
    }
}

impl Drop for PipelineWatch {
    fn drop(&mut self) {
        let Some(id) = self.id else { return };
        let elapsed = self.started.elapsed().as_secs();
        flights().retain(|flight| flight.id != id);
        if elapsed >= 5 {
            crate::diagnostics::log(&format!(
                "CreatePipelineState #{id} {} finished in {elapsed}s",
                self.label
            ));
        }
    }
}

fn flights() -> std::sync::MutexGuard<'static, Vec<Flight>> {
    static FLIGHTS: std::sync::Mutex<Vec<Flight>> = std::sync::Mutex::new(Vec::new());
    FLIGHTS.lock().unwrap_or_else(|error| error.into_inner())
}

fn watch() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let mut notes = Vec::new();
        {
            let mut open = flights();
            for flight in open.iter_mut() {
                let elapsed = flight.started.elapsed().as_secs();
                if elapsed < flight.next_due {
                    continue;
                }
                notes.push((flight.id, format!(
                    "CreatePipelineState #{} {} still running {elapsed}s",
                    flight.id, flight.label
                )));
                flight.next_due = elapsed.saturating_add(15);
            }
        }
        for (id, note) in notes {
            // The compile can finish between the snapshot and this write.
            if flights().iter().any(|flight| flight.id == id) {
                crate::diagnostics::log(&note);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/big_stack.rs"]
mod tests;
