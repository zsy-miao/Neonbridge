//! Host pipelines are made by workers, not by the title thread that first
//! records a draw with them. Translating a shader and the host driver's
//! compile take from milliseconds to seconds each (structurisation of the
//! biggest compute programs ~0.4 s, some driver compiles 3.4 s), and
//! the title's first world frame wants over 1,200 new pipelines: made
//! on its recording threads, that frame took more than 30 s and the
//! title's GPU watchdog ended the run (decision 0007, "Pipelines made off
//! the title's threads").
//!
//! A first use queues the pipeline and waits for it within a budget per
//! frame, so the cheap ones (menus, one-off passes such as a colour-grading
//! bake) are there at once as before; past the budget a draw or dispatch
//! whose pipeline is not ready is dropped, as an untranslated one is.
//!
//! The default budget is half a second per title frame for the first
//! [`WARMUP_FRAMES`] (boot and one-shot init passes compile reliably), then
//! [`STEADY_MS`]: half a second in every frame that met a new pipeline made
//! the intro and menus stutter while a cold library was filled. A pass the
//! title runs only once and whose pipeline takes longer is lost.
//!
//! A dispatch waits within the same budget and is dropped past it, as a
//! draw is. Waiting for every compute pipeline at its first dispatch (up to
//! 20 s each, made on the recording thread if no worker had taken it) held
//! the world's first frame until hundreds of them were made: a black screen
//! for minutes with a stale library (made ~170 per 20 s). Every compute
//! pipeline is still started when the title creates it, so most are made
//! during loading; a dispatch promotes its own ahead of those.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::Shader;
use super::host::{GraphicsState, HostPipeline};

/// What a host pipeline is made from, and the host pipeline once made.
pub(crate) struct Pending {
    shaders: Vec<Arc<Shader>>,
    pub(crate) graphics: Option<Box<GraphicsState>>,
    queue_state: AtomicU8,
    made: OnceLock<Option<HostPipeline>>,
    /// What identifies it across runs (shader containers and state), and
    /// whether a draw or dispatch has requested it yet.
    key: String,
    used: AtomicBool,
    /// When a worker began it ([`slow`]).
    begun: OnceLock<Instant>,
    /// First title frame in which a recording thread waited on this pipeline
    /// (`u64::MAX`: never waited yet). Caps waiting to one frame per pipeline
    /// so a slow or hung shader compiles asynchronously without stalling every
    /// subsequent frame for the full wait budget.
    waited_frame: AtomicU64,
}

// SAFETY: D3D12 objects are free-threaded; the graphics state (whose input
// elements point into its own names) is only read after creation.
unsafe impl Send for Pending {}
// SAFETY: as above.
unsafe impl Sync for Pending {}

impl Pending {
    pub(crate) fn new(shaders: Vec<Arc<Shader>>, graphics: Option<Box<GraphicsState>>) -> Pending {
        let mut key = blake3::Hasher::new();
        for s in &shaders {
            key.update(s.stage.as_bytes());
            key.update(s.hash.as_bytes());
        }
        key.update(graphics.as_ref().map(|g| g.describe()).unwrap_or_default().as_bytes());
        Pending {
            shaders,
            graphics,
            queue_state: AtomicU8::new(NOT_QUEUED),
            made: OnceLock::new(),
            key: key.finalize().to_hex()[..32].to_owned(),
            used: AtomicBool::new(false),
            begun: OnceLock::new(),
            waited_frame: AtomicU64::new(u64::MAX),
        }
    }

    fn make(&self) {
        let _ = self.begun.set(Instant::now());
        self.made
            .get_or_init(|| HostPipeline::new(&self.shaders, self.graphics.as_deref()));
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    }

    fn compute(&self) -> bool {
        matches!(self.shaders.as_slice(), [cs] if cs.stage == "compute")
    }
}

/// Why a pipeline has no host pipeline (diagnostics): its shaders that do
/// not translate, else "refused" or "being made"; `None` if it has one.
#[cfg(feature = "diagnostics")]
pub(crate) fn missing(p: &Pending) -> Option<String> {
    match p.made.get() {
        Some(Some(_)) => None,
        None => Some("being made".into()),
        Some(None) => {
            let failed: Vec<String> = p
                .shaders
                .iter()
                .filter(|s| matches!(s.translated.get(), Some(None)))
                .map(|s| format!("{} {}", s.stage, s.hash))
                .collect();
            Some(if failed.is_empty() {
                format!("refused {:?}", p.shaders)
            } else {
                failed.join(" + ")
            })
        }
    }
}

/// Signalled whenever a worker has made a pipeline.
static DONE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

/// Title frames that get [`WARMUP_MS`] of waiting.
const WARMUP_FRAMES: u64 = 60;
const WARMUP_MS: u64 = 500;
/// Waiting per frame after the warm-up frames.
const STEADY_MS: u64 = 8;

/// Waiting allowed in title frame `frame`, over all threads;
/// `VICTORY_PIPELINE_WAIT_MS` sets it for every frame.
fn wait_budget(frame: u64) -> Duration {
    static BUDGET: OnceLock<Option<Duration>> = OnceLock::new();
    let set = *BUDGET.get_or_init(|| {
        let millis = std::env::var("VICTORY_PIPELINE_WAIT_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok());
        tracing::info!(?millis, "host pipeline wait budget per frame (none: {WARMUP_MS} ms, then {STEADY_MS} ms)");
        millis.map(Duration::from_millis)
    });
    set.unwrap_or(Duration::from_millis(if frame < WARMUP_FRAMES { WARMUP_MS } else { STEADY_MS }))
}

/// Whether the host's driver has been at compute pipeline `p` for longer
/// than most take (one of the title's takes 85 s): no frame waits for it.
fn slow(p: &Pending) -> bool {
    p.begun.get().is_some_and(|at| at.elapsed() > Duration::from_secs(10))
}

/// What a frame may wait for a compute pipeline while a world loads: the
/// passes that unpack a world's models are dispatched once, and in a run
/// whose pipelines were all still to be made 130 of them were dropped, and
/// with them walls, floor, ceiling and faces (they never came back). A draw
/// comes again next frame, so graphics pipelines keep the frame's budget.
const LOADING_FRAME_COMPUTE: Duration = Duration::from_secs(12);

/// What is left of the bound on all such waits of a run, past the frames'
/// budgets, after `spent`: `VICTORY_PIPELINE_GATE_S` seconds (victory-amd
/// uses 120). Off by default: here every compute pipeline is started at
/// its creation, `victory warm` puts them in the library ahead of a run,
/// and waits past the frame's budget once made the world's first frame
/// take minutes.
fn loading_left(spent: Duration) -> Duration {
    static TOTAL: OnceLock<Duration> = OnceLock::new();
    let total = *TOTAL.get_or_init(|| {
        let seconds = std::env::var("VICTORY_PIPELINE_GATE_S")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0);
        Duration::from_secs(seconds)
    });
    total.saturating_sub(spent)
}

/// Pipelines queued and not made yet.
static WAITING: AtomicU64 = AtomicU64::new(0);

/// Pipelines queued and not made yet (for the frame line).
pub(crate) fn waiting() -> u64 {
    WAITING.load(Ordering::Relaxed)
}

/// The host pipeline if it is made (or is made within the frame's budget);
/// queues it on first use.
pub(crate) fn ready(p: &Arc<Pending>) -> Option<&HostPipeline> {
    // A still-pending pipeline was requested by the title too. Prewarm it
    // on the next launch, even if this run never got to use it.
    if !p.used.load(Ordering::Relaxed) && !p.used.swap(true, Ordering::Relaxed) {
        super::library::note_used(&p.key);
    }
    if p.made.get().is_none() {
        start(p, REQUESTED);
        let frame = crate::frame::current();
        let first = match p.waited_frame.compare_exchange(
            u64::MAX,
            frame,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => frame,
            Err(f) => f,
        };
        if first == frame {
            wait(p, frame);
        }
    }
    p.made.get()?.as_ref()
}

/// Makes `p` on this thread if it is not made yet (waiting for a worker
/// that is making it); whether it has a host pipeline.
pub(crate) fn now(p: &Arc<Pending>) -> bool {
    p.make();
    p.made.get().is_some_and(|made| made.is_some())
}

/// Makes the host pipeline of a compute program for the driver's library
/// (`victory warm`); whether it has one.
pub(crate) fn warm(shaders: Vec<Arc<Shader>>) -> bool {
    HostPipeline::new(&shaders, None).is_some()
}

/// At the title's creation of a pipeline: starts it at once if an earlier
/// run drew with it (during loading, so the world's first frames find it),
/// and every compute pipeline: what a dropped dispatch writes is missing
/// until its pipeline is made, so the world's are made while it loads.
pub(crate) fn early(p: &Arc<Pending>) {
    if !p.shaders.is_empty() && (p.compute() || super::library::wanted_early(&p.key)) {
        super::library::EARLY.fetch_add(1, Ordering::Relaxed);
        start(p, EARLY);
    }
}

/// Whether the frame's budget is one for all threads together: a thread
/// that waits takes what is left of it and gives back what it did not
/// use. Counted only when a wait had ended, the budget was one per thread:
/// each of the title's five recording threads began its first wait of a
/// frame with nothing counted yet and waited the whole half second, 2.5 s
/// of waiting in every frame in which pipelines were being made (runs
/// 34-49: "2504 ms waiting for pipelines" in each such frame).
/// On by default; `VICTORY_PIPELINE_WAIT_SHARED=0` counts as before.
fn shared_budget() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_WAIT_SHARED").is_none_or(|v| v != "0"))
}

/// Waits for `p` while frame `frame` has budget left.
fn wait(p: &Pending, frame: u64) {
    let budget = wait_budget(frame);
    let compute = p.compute();
    if budget.is_zero() || compute && slow(p) {
        return;
    }
    // What compute waits took past the frames' budgets since the start, in
    // milliseconds; they have a budget of their own (ported from myokex's
    // victory-amd).
    static PAST: AtomicU64 = AtomicU64::new(0);
    let budget = if compute {
        budget.max(LOADING_FRAME_COMPUTE.min(loading_left(Duration::from_millis(PAST.load(Ordering::Relaxed)))))
    } else {
        budget
    };
    // The frame the budget belongs to, and what was waited of it (or is
    // taken by a thread waiting now); compute and graphics apart.
    static SPENTS: [Mutex<(u64, Duration)>; 2] = [const { Mutex::new((u64::MAX, Duration::ZERO)) }; 2];
    let frame_spent = &SPENTS[usize::from(compute)];
    let shared = shared_budget();
    let left = {
        let mut spent = frame_spent.lock().unwrap_or_else(|e| e.into_inner());
        if spent.0 != frame {
            *spent = (frame, Duration::ZERO);
        }
        let left = budget.saturating_sub(spent.1);
        if shared {
            spent.1 += left;
        }
        left
    };
    if left.is_zero() {
        return;
    }
    let started = Instant::now();
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while p.made.get().is_none() && !(compute && slow(p)) {
        let rest = left.saturating_sub(started.elapsed());
        if rest.is_zero() {
            break;
        }
        // Woken now and then to see whether it has become a slow one.
        let rest = rest.min(Duration::from_millis(250));
        held = done.wait_timeout(held, rest).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    let waited = started.elapsed().min(left);
    if compute {
        PAST.fetch_add(waited.saturating_sub(wait_budget(frame)).as_millis() as u64, Ordering::Relaxed);
    }
    {
        let mut spent = frame_spent.lock().unwrap_or_else(|e| e.into_inner());
        // A frame that began meanwhile has its own budget.
        if spent.0 == frame {
            if shared {
                spent.1 = spent.1.saturating_sub(left - waited);
            } else {
                spent.1 += waited;
            }
        }
    }
    crate::framelog::add(crate::framelog::T::PipelineWaitUs, started.elapsed().as_micros() as u64);
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(
        crate::copystat::C::PipelineWaitUs,
        started.elapsed().as_micros() as u64,
    );
}

const NOT_QUEUED: u8 = 0;
const EARLY: u8 = 1;
const REQUESTED: u8 = 2;
const MAKING: u8 = 3;

struct Queue {
    early: VecDeque<Arc<Pending>>,
    requested: VecDeque<Arc<Pending>>,
}

impl Queue {
    const fn new() -> Self {
        Self {
            early: VecDeque::new(),
            requested: VecDeque::new(),
        }
    }

    /// Queues an unrequested pipeline or promotes one queued at creation.
    /// Returns whether it is new to the queue, or `None` if nothing changed.
    fn push(&mut self, p: &Arc<Pending>, priority: u8) -> Option<bool> {
        let prev = p.queue_state.load(Ordering::Relaxed);
        if prev >= priority {
            return None;
        }
        p.queue_state.store(priority, Ordering::Relaxed);
        if priority == REQUESTED {
            self.requested.push_back(p.clone());
        } else {
            self.early.push_back(p.clone());
        }
        Some(prev == NOT_QUEUED)
    }

    /// The next pipeline to make: a requested one first, unless only early
    /// ones are taken (a spare worker, which can starve).
    fn pop(&mut self, early_only: bool) -> Option<Arc<Pending>> {
        let requested = if early_only { None } else { self.requested.pop_front() };
        let p = requested.or_else(|| {
            // An early entry can have been promoted and claimed already.
            std::iter::from_fn(|| self.early.pop_front())
                .find(|p| p.queue_state.load(Ordering::Relaxed) == EARLY)
        })?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
        Some(p)
    }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static AVAILABLE: Condvar = Condvar::new();

/// Start or promote work for a draw without waiting behind the entire early
/// list. Queue changes and worker claims happen under the same lock.
fn start(p: &Arc<Pending>, priority: u8) {
    if p.queue_state.load(Ordering::Relaxed) >= priority {
        return;
    }
    if !workers() {
        if p.queue_state.swap(MAKING, Ordering::Relaxed) == NOT_QUEUED {
            p.make();
        }
        return;
    }
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(new) = queue.push(p, priority) {
        if new {
            PEAK.fetch_max(WAITING.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        // All: a spare worker woken for a requested one would not take it.
        AVAILABLE.notify_all();
    }
}

fn next(early_only: bool) -> Arc<Pending> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(p) = queue.pop(early_only) {
            return p;
        }
        queue = AVAILABLE.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// Stack of each pipeline worker: room for the driver's shader compiler,
/// which runs on it (reserved address space; committed only as used).
const PIPELINE_STACK: usize = 64 << 20;

/// Workers at the title's priority (half the logical processors, 2 to 8)
/// and spare ones below it, up to all processors but two (at most 16
/// workers in all), for `logical` processors. The spare ones run while the
/// title waits (loading, a world whose pipelines are not made yet: 8
/// workers made ~8 a second of a cold library while the rest of a 24-thread
/// processor stood idle) and starve when it is busy, as all workers did
/// below its priority (PR #1505), so they cost the world's frames nothing;
/// they take only early pipelines, so none a frame waits for starves there.
/// shortcut: 16 caps the driver compiles' memory unmeasured; raise it after
/// a cold run shows the commit charge with all of them compiling.
fn worker_counts(logical: usize) -> (usize, usize) {
    let normal = (logical / 2).clamp(2, 8);
    (normal, logical.saturating_sub(2).min(16).saturating_sub(normal))
}

/// Starts the workers ([`worker_counts`]) on first use.
fn workers() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        let mut running = false;
        let (normal, spare) = worker_counts(std::thread::available_parallelism().map_or(4, |n| n.get()));
        tracing::info!(normal, spare, "host pipeline workers");
        for i in 0..normal + spare {
            let spawned = std::thread::Builder::new()
                .name(format!("victory-pipelines-{i}"))
                // The driver compiles pipelines on this thread, recursively:
                // NVIDIA's compiler (nvgpucomp64) overflowed the default 2 MB
                // on a world compute program and refused it as out of memory.
                .stack_size(PIPELINE_STACK)
                .spawn(move || {
                    if i >= normal {
                        // SAFETY: a plain call on our own thread.
                        unsafe {
                            use windows::Win32::System::Threading::*;
                            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                        }
                    }
                    // The others at the title's own priority: below it,
                    // the world's busy threads starved the workers (236
                    // pipelines a minute at the start screen, 7 a minute in
                    // the world on a 16-thread laptop, PR #1505), and draws
                    // stay dropped until their pipelines exist.
                    loop {
                        next(i >= normal).make();
                        if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
                            drained();
                        } else {
                            super::library::save(false);
                        }
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "no pipeline worker");
            } else {
                running = true;
            }
        }
        running
    })
}

/// The most pipelines waiting at once since the workers last ran out.
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The workers ran out of work: logged after a batch of 10 or more, and
/// the cache saved (at most every 20 s).
fn drained() {
    use super::library::{EARLY, LOADED, MADE};
    if PEAK.swap(0, Ordering::Relaxed) >= 10 {
        tracing::info!(
            made = MADE.load(Ordering::Relaxed),
            loaded = LOADED.load(Ordering::Relaxed),
            started_early = EARLY.load(Ordering::Relaxed),
            "host pipelines all made"
        );
    }
    super::library::save(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first use queues the pipeline and, within the budget, gets the
    /// worker's answer (here: no host pipeline for no shaders).
    #[test]
    fn first_use_is_made_by_a_worker() {
        let p = Arc::new(Pending::new(Vec::new(), None));
        assert!(ready(&p).is_none());
        assert_ne!(p.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
        let started = Instant::now();
        while p.made.get().is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(matches!(p.made.get(), Some(None)));
    }

    /// A compute pipeline is started at its creation, and a dispatch whose
    /// pipeline a worker is still making waits only within the frame's
    /// budget (it waited up to 20 s per pipeline, and the world's first
    /// frame minutes), then is dropped.
    #[test]
    fn a_compute_dispatch_waits_only_within_the_frame_budget() {
        let cs = Arc::new(super::super::Shader {
            stage: "compute",
            hash: "c0c0c0c0c0c0c0c0".to_owned(),
            container: Vec::new(),
            translated: OnceLock::new(),
            walks: OnceLock::new(),
            resolves_light: false,
            draws_sky: false,
            ends_reflections: false,
            bakes: false,
        });
        let early_one = Arc::new(Pending::new(vec![cs.clone()], None));
        early(&early_one);
        assert_ne!(early_one.queue_state.load(Ordering::Relaxed), NOT_QUEUED);

        let p = Arc::new(Pending::new(vec![cs], None));
        // A worker has taken it and cannot finish it yet.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let started = Instant::now();
        assert!(ready(&p).is_none());
        assert!(p.made.get().is_none(), "not made on the recording thread");
        assert!(started.elapsed() <= wait_budget(0) + Duration::from_millis(250));
    }

    #[test]
    fn spare_workers_fill_all_processors_but_two() {
        assert_eq!(worker_counts(1), (2, 0));
        assert_eq!(worker_counts(4), (2, 0));
        assert_eq!(worker_counts(8), (4, 2));
        assert_eq!(worker_counts(16), (8, 6));
        assert_eq!(worker_counts(24), (8, 8));
        assert_eq!(worker_counts(64), (8, 8));
    }

    #[test]
    fn pending_pipeline_is_recorded_for_the_next_launch() {
        let mut p = Pending::new(Vec::new(), None);
        p.key = "pending-pipeline-test".to_owned();
        // Simulate a worker that has taken the job but cannot finish it yet.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let p = Arc::new(p);

        assert!(!super::super::library::wanted_early(&p.key));
        assert!(ready(&p).is_none());
        assert!(p.made.get().is_none());
        assert!(super::super::library::wanted_early(&p.key));
    }

    #[test]
    fn draws_promote_early_pipelines_without_claiming_them_twice() {
        let earlier = Arc::new(Pending::new(Vec::new(), None));
        let promoted = Arc::new(Pending::new(Vec::new(), None));
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();

        assert_eq!(queue.push(&earlier, EARLY), Some(true));
        assert_eq!(queue.push(&promoted, EARLY), Some(true));
        assert_eq!(queue.push(&requested, REQUESTED), Some(true));
        assert_eq!(queue.push(&promoted, REQUESTED), Some(false));
        assert_eq!(queue.push(&promoted, REQUESTED), None);

        assert!(Arc::ptr_eq(&queue.pop(false).unwrap(), &requested));
        assert!(Arc::ptr_eq(&queue.pop(false).unwrap(), &promoted));
        assert_eq!(queue.push(&promoted, REQUESTED), None);
        assert!(Arc::ptr_eq(&queue.pop(false).unwrap(), &earlier));
        assert!(queue.pop(false).is_none());
    }

    #[test]
    fn spare_workers_take_only_early_pipelines() {
        let early_one = Arc::new(Pending::new(Vec::new(), None));
        let promoted = Arc::new(Pending::new(Vec::new(), None));
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();
        queue.push(&early_one, EARLY);
        queue.push(&promoted, EARLY);
        queue.push(&requested, REQUESTED);
        queue.push(&promoted, REQUESTED);

        // The promoted one's early entry is skipped: it is requested now.
        assert!(Arc::ptr_eq(&queue.pop(true).unwrap(), &early_one));
        assert!(queue.pop(true).is_none());
        assert!(Arc::ptr_eq(&queue.pop(false).unwrap(), &requested));
        assert!(Arc::ptr_eq(&queue.pop(false).unwrap(), &promoted));
    }

    #[test]
    fn slow_pipeline_waits_at_most_one_frame() {
        let mut p = Pending::new(Vec::new(), None);
        p.key = "slow-pipeline-one-frame-test".to_owned();
        // Simulate a pipeline already claimed by a worker that has not finished.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let first_frame = crate::frame::current();
        // Pretend it already exhausted its wait on another frame (avoid u64::MAX sentinel).
        let earlier_frame = if first_frame == 0 { 1 } else { first_frame - 1 };
        p.waited_frame.store(earlier_frame, Ordering::Relaxed);
        let p = Arc::new(p);
        let started = Instant::now();
        assert!(ready(&p).is_none());
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "subsequent frames must not block for the pipeline wait budget"
        );
    }
}
