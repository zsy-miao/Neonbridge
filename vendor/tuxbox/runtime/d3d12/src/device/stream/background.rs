//! Pipelines whose translation missed the title's wait budget are rebuilt by
//! workers (VIctory's "pipelines made off the title's threads"). The title gets
//! a null pipeline at once; its draws are dropped only until a worker, which
//! waits on the translator without a budget, has made the real host pipeline
//! and published it in the proxy's upgrade slot. Command lists read that slot
//! whenever they bind the pipeline, so the render thread never waits on the
//! translator or on the driver's compile.
//!
//! Pipelines no earlier run bound are lazy (VIctory makes every host pipeline
//! on first use: the title creates some 60,000 in its first 20 s and draws
//! with a few dozen). Their proxies hold a shared placeholder; the first bind
//! queues them and waits within the frame's budget, and command lists drop
//! draws and dispatches until they exist.
use super::super::compute::{Pipeline, create_pipeline_x};
use super::super::Device;
use super::{AMPLIFICATION_X, MESH_X, STENCIL_X, layout};
use core::{ffi::c_void, mem::MaybeUninit, ptr, slice};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Interface, PCSTR};

pub(in crate::device) mod journal;

/// The pipeline a worker made for one created without its translation: an
/// ordinary pipeline proxy of the same device, owned by the slot.
pub(crate) struct Built(IUnknown);

impl Built {
    pub(crate) fn pipeline(&self) -> &Pipeline {
        // SAFETY: made by create_pipeline or create_pipeline_x, retained by self.
        unsafe { &*self.0.as_raw().cast::<Pipeline>() }
    }
}

/// Written once by a worker, then only read by command lists.
#[derive(Default)]
pub(crate) struct Upgrade {
    built: OnceLock<Built>,
    /// A lazy pipeline's description until its first bind hands it to the workers.
    deferred: Mutex<Option<(IUnknown, Work)>>,
    /// A worker found no translation for it: it stays a null pipeline.
    failed: std::sync::atomic::AtomicBool,
    /// The thread id of the worker building it, 0 when none is.
    running: std::sync::atomic::AtomicU32,
    /// Why its worker could not make it: the shader keys that got no
    /// translation and the reasons (see `failed_bind`).
    why: OnceLock<String>,
    /// A title thread bound it or waits on it (see `waited_on`).
    wanted: std::sync::atomic::AtomicBool,
    /// A compute pipeline (set when its job is made; see `compute_waits`).
    compute: std::sync::atomic::AtomicBool,
}

// SAFETY: D3D12 objects are free-threaded. The built state is published once
// through the OnceLock and is immutable afterwards.
unsafe impl Send for Upgrade {}
// SAFETY: as above.
unsafe impl Sync for Upgrade {}

impl Upgrade {
    pub(crate) fn get(&self) -> Option<&Built> {
        self.built.get()
    }
}

/// Signalled whenever a worker finishes a pipeline.
static DONE: (Mutex<()>, std::sync::Condvar) = (Mutex::new(()), std::sync::Condvar::new());

/// The pipeline to bind for a command list: a lazy pipeline is handed to
/// the workers on its first bind, and waited for while the title's frame
/// has translator budget left (VIctory's `make::ready`). `None` while it is
/// still being made: the list drops its draws and dispatches.
/// An upgrade that is never built: a pipeline holding it stays the shared
/// placeholder and command lists drop its draws and dispatches.
pub(crate) fn never() -> Arc<Upgrade> {
    let slot = Upgrade { failed: std::sync::atomic::AtomicBool::new(true), ..Default::default() };
    Arc::new(slot)
}

/// leonida-ce 83bf1f60 (IAmNotSiob, GPL-3): a pipeline the title failed to
/// create left a null record in its definition tables, and a later walk over
/// them faulted on it (their 120-140 s stop; our test120 write AV at 0).
/// A refused create now hands the title the shared placeholder holding a
/// never-built upgrade, so its binds drop their work and are logged with
/// `why`. TUXBOX_REFUSED_PLACEHOLDER=0 returns the failure, as before.
pub(crate) fn refused_placeholder_on() -> bool {
    crate::env_toggle!("TUXBOX_REFUSED_PLACEHOLDER") != Some("0")
}

pub(crate) fn refused(kind: &str, status: windows::core::HRESULT) -> Arc<Upgrade> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let misses = super::super::shader_cache::take_misses();
    let reasons = misses.iter().map(|(key, why)| format!("{key}: {why}")).collect::<Vec<_>>().join("; ");
    let why = format!("{kind} create refused ({:#x}){}", status.0 as u32,
        if reasons.is_empty() { String::new() } else { format!(": {reasons}") });
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "{why}; the title gets the placeholder ({n} so far; TUXBOX_REFUSED_PLACEHOLDER=0 returns the failure)"));
    }
    let slot = never();
    let _ = slot.why.set(why);
    slot
}

/// test116 counted 84 binds of untranslatable pipelines with nothing saying
/// which shaders or why. Each distinct pipeline bound after its worker gave
/// up is logged with the shader keys and refusal reasons, the first
/// TUXBOX_UNTRANSLATABLE_BINDS (32; 0 off), then every power-of-two bind.
pub(crate) fn failed_bind(slot: &Upgrade) {
    static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let limit = *LIMIT.get_or_init(|| crate::env_toggle!("TUXBOX_UNTRANSLATABLE_BINDS").and_then(|v| v.trim().parse().ok()).unwrap_or(32));
    if limit == 0 {
        return;
    }
    static SEEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    static BINDS: AtomicU64 = AtomicU64::new(0);
    let binds = BINDS.fetch_add(1, Ordering::Relaxed) + 1;
    let id = slot as *const Upgrade as usize;
    let first = {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let new = seen.len() < limit && !seen.contains(&id);
        if new {
            seen.push(id);
        }
        new.then_some(seen.len())
    };
    if first.is_some() || binds.is_power_of_two() {
        let why = slot.why.get().map_or("never handed to a worker", String::as_str);
        crate::diagnostics::log_line(format_args!(
            "untranslatable pipeline bound{}: {why} ({binds} such binds so far; TUXBOX_UNTRANSLATABLE_BINDS=0 off)",
            first.map_or(String::new(), |n| format!(" (#{n})"))));
    }
}

/// Its worker could not make it: it stays the placeholder.
pub(crate) fn failed(slot: &Upgrade) -> bool {
    slot.built.get().is_none() && slot.failed.load(Ordering::Relaxed)
}

/// Title threads now held in a bind or draw waiting for a pipeline (built
/// inline or by a worker). xmem's Sleep does not count the time they are held
/// (tuxbox_title_held).
pub(crate) static TITLE_HELD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct Held;

impl Held {
    fn new() -> Held {
        TITLE_HELD.fetch_add(1, Ordering::Relaxed);
        Held
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        TITLE_HELD.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) fn ready(slot: &Arc<Upgrade>) -> Option<&Built> {
    if let Some(built) = slot.built.get() {
        return Some(built);
    }
    let _held = Held::new();
    let deferred = slot.deferred.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((device, mut work)) = deferred {
        let graphics = match &mut work {
            Work::Stream(stream) => unsafe { super::is_graphics(&stream.desc()) },
            Work::Compute(_) => false,
        };
        slot.compute.store(!graphics, Ordering::Release);
        let job = Job { device, work, graphics, slot: slot.clone() };
        // five-star-recomp 9d61a3c (GPL-3.0, credit to its authors): a small
        // compute first use made inline waited on programs still being made
        // on demand, and a loading frame met hundreds of them, past the
        // title's hang watchdog. Only a translation already cached is made
        // inline; the rest go to the workers. TUXBOX_INLINE_UNCACHED=1 makes
        // them inline as before.
        let cached = crate::env_toggle!("TUXBOX_INLINE_UNCACHED") == Some("1")
            || matches!(&job.work, Work::Compute(c) if crate::device::shader_cache::translation_ready(&c.code));
        match small(&job) && cached && first_use::admit() {
            true => {
                let started = std::time::Instant::now();
                inline(job);
                first_use::spend(started.elapsed());
            }
            false => send(job, true),
        }
        if let Some(built) = slot.built.get() {
            return Some(built);
        }
    } else {
        promote(slot);
        hurry(slot);
    }
    let _cost = crate::diagnostics::cost::timer(crate::diagnostics::cost::Bucket::PipelineWait);
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while slot.built.get().is_none() && !slot.failed.load(Ordering::Relaxed) {
        let step = std::time::Duration::from_millis(5);
        if !crate::device::shader_cache::spend_wait(step) {
            break;
        }
        held = done.wait_timeout(held, step).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    slot.built.get()
}

/// Waits up to `limit` for a lazy pipeline's worker outside the frame's
/// translator budget (a compute pipeline whose dispatches must not be lost).
pub(crate) fn wait_for(slot: &Arc<Upgrade>, limit: std::time::Duration) -> Option<&Built> {
    if let Some(built) = ready(slot) {
        return Some(built);
    }
    let _held = Held::new();
    promote(slot);
    // VIctory-fxceless draw.rs prepare_for (GPL-3.0, credit to its authors)
    // never drops a dispatch: a compute pipeline not yet made is built on
    // the title's thread, or waited for, as long as it takes. Ours dropped
    // bursts of 32 to 1024 dispatches as "still being made" (test147) when
    // the first-use bucket or the bind's limit ran out, and a dispatch the
    // title needs (one that writes a counter or a label) left it waiting
    // for work that never ran. TUXBOX_COMPUTE_WAIT_BUILD=0 keeps the limit.
    if compute_waits() && slot.compute.load(Ordering::Acquire) {
        static N: AtomicU64 = AtomicU64::new(0);
        let started = std::time::Instant::now();
        let built = wait_granted(slot, std::time::Duration::from_secs(3600));
        first_use::spend(started.elapsed());
        let n = N.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a compute bind waited {:.1} ms for its build, {} ({n} so far; TUXBOX_COMPUTE_WAIT_BUILD=0 keeps the bind limit)",
                started.elapsed().as_secs_f64() * 1e3, if built.is_some() { "made" } else { "failed" }));
        }
        return built;
    }
    let Some(limit) = first_use::grant(limit) else { return None };
    let started = std::time::Instant::now();
    let built = wait_granted(slot, limit);
    first_use::spend(started.elapsed());
    built
}

fn compute_waits() -> bool {
    crate::env_toggle!("TUXBOX_COMPUTE_WAIT_BUILD") != Some("0")
}

fn wait_granted(slot: &Arc<Upgrade>, limit: std::time::Duration) -> Option<&Built> {
    // test111: a steal that ran out of time was followed by a full new wait,
    // so one bind could block 2x its limit. Both now share one deadline.
    let deadline = std::time::Instant::now() + limit;
    slot.wanted.store(true, Ordering::Release);
    if steal_on() && let Some(job) = steal(slot) {
        match job.graphics && display_now() {
            // Only display-sized binds wait on a graphics pipeline here
            // (compute::wait_pending_display).
            true => build_display(job),
            false => {
                build_here(job, limit);
                if let Some(built) = slot.built.get() {
                    return Some(built);
                }
            }
        }
    }
    hurry(slot);
    // The worker building it gets its translation served first.
    let worker = slot.running.load(Ordering::Acquire);
    if worker != 0 {
        crate::device::shader_cache::urge_worker(worker, true);
    }
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while slot.built.get().is_none() && !slot.failed.load(Ordering::Relaxed) {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let step = (deadline - now).min(std::time::Duration::from_millis(5));
        held = done.wait_timeout(held, step).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    if worker != 0 {
        crate::device::shader_cache::urge_worker(worker, false);
    }
    slot.built.get()
}

/// five-star-recomp 8fa5672 (GPL-3.0): a small compute pipeline (a culling
/// or clear initialiser) is made on the title's thread at its first bind
/// instead of losing to a frame budget already spent. Its translator wait still
/// counts against that budget; one that is not made goes to the workers.
/// TUXBOX_SMALL_COMPUTE_BYTES (16384; 0 off) sets the shader size limit.
fn small(job: &Job) -> bool {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    let limit = *LIMIT.get_or_init(|| std::env::var("TUXBOX_SMALL_COMPUTE_BYTES").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(16384));
    !on_worker() && matches!(&job.work, Work::Compute(c) if !c.code.is_empty() && c.code.len() <= limit)
}

/// five-star-recomp ab94e3c (GPL-3.0, FirstUseBudget.cs): the time the
/// title's threads spend making pipelines at their first bind is shared, a
/// bucket that refills a quarter second each second up to one second. A
/// loading frame meeting hundreds of new pipelines at 0.25-0.5 s each ran
/// past the title's hang watchdog. A first bind with the bucket empty, or
/// while a compile waits for memory in the compile budget, goes to the
/// workers. TUXBOX_FIRST_USE_BUDGET=0 off.
mod first_use {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    const REFILL_PER_SECOND: f64 = 0.25;
    const CAPACITY: f64 = 1.0;

    /// (seconds left, when last refilled)
    static BUCKET: Mutex<Option<(f64, Instant)>> = Mutex::new(None);
    static OVER: AtomicU64 = AtomicU64::new(0);

    fn enabled() -> bool {
        crate::env_toggle!("TUXBOX_FIRST_USE_BUDGET") != Some("0")
    }

    fn refill(bucket: &mut Option<(f64, Instant)>) -> &mut (f64, Instant) {
        let now = Instant::now();
        let (left, at) = bucket.get_or_insert((CAPACITY, now));
        *left = (*left + now.duration_since(*at).as_secs_f64() * REFILL_PER_SECOND).min(CAPACITY);
        *at = now;
        bucket.as_mut().unwrap()
    }

    pub(super) fn admit() -> bool {
        if !enabled() {
            return true;
        }
        let left = refill(&mut BUCKET.lock().unwrap_or_else(|e| e.into_inner())).0;
        let memory = crate::device::compile_budget::waiting();
        if left > 0.0 && !memory {
            return true;
        }
        let n = OVER.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a small compute pipeline's first bind went to the workers ({}; {n} so far; TUXBOX_FIRST_USE_BUDGET=0 off)",
                if memory { "a compile waits for memory" } else { "first-use budget spent" }));
        }
        false
    }

    /// A bind wait on a title thread: `limit` cut to the bucket's time left,
    /// or `None` with the bucket empty (the bind keeps its stand-in). test130
    /// and test131: the five Subrender job threads each waited up to 750 ms
    /// per pipeline outside the bucket, the loading frame never ended, and the
    /// title's 30 s watchdog (game+0x2215a60, handler game+0x23cf620 "GPU Loc
    /// Sig", five-star acd5260) wrote 0x4d8bd421 to address 0.
    /// TUXBOX_BIND_WAIT_BUDGET=0 waits outside the bucket as before.
    pub(super) fn grant(limit: Duration) -> Option<Duration> {
        if !enabled() || crate::env_toggle!("TUXBOX_BIND_WAIT_BUDGET") == Some("0") {
            return Some(limit);
        }
        let left = refill(&mut BUCKET.lock().unwrap_or_else(|e| e.into_inner())).0;
        if left > 0.0 {
            return Some(limit.min(Duration::from_secs_f64(left)));
        }
        static EMPTY: AtomicU64 = AtomicU64::new(0);
        let n = EMPTY.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a bind did not wait for its pipeline, the title threads' first-use budget is spent ({n} so far; TUXBOX_BIND_WAIT_BUDGET=0 off)"));
        }
        None
    }

    pub(super) fn spend(took: Duration) {
        if enabled() {
            refill(&mut BUCKET.lock().unwrap_or_else(|e| e.into_inner())).0 -= took.as_secs_f64();
        }
    }
}

fn inline(mut job: Job) {
    static INLINE: AtomicU64 = AtomicU64::new(0);
    static REQUEUED: AtomicU64 = AtomicU64::new(0);
    let made = crate::big_stack::run(|| {
        WORKER.with(|w| w.set(true));
        NULLED.with(|n| n.set(false));
        // SAFETY: a device proxy begins with its IUnknown identity, retained by the job.
        let device = unsafe { &*job.device.as_raw().cast::<Device>() };
        let iid = ID3D12PipelineState::IID;
        let mut out = ptr::null_mut();
        let Work::Compute(compute) = &mut job.work else { return None };
        let desc = compute.desc();
        let status = unsafe { create_pipeline_x(device, &desc, 0, ptr::null(), &iid, &mut out) };
        WORKER.with(|w| w.set(false));
        let nulled = NULLED.with(|n| n.replace(false));
        let made = (status.is_ok() && !out.is_null()).then(|| unsafe { IUnknown::from_raw(out) });
        made.filter(|_| !nulled)
    });
    match made {
        Some(proxy) => {
            let _ = job.slot.built.set(Built(proxy));
            let n = INLINE.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "background pipelines: a small compute pipeline made at its first bind on the title's thread ({n} so far, {} sent on to the workers; TUXBOX_SMALL_COMPUTE_BYTES=0 off)",
                    REQUEUED.load(Ordering::Relaxed)));
            }
        }
        None => {
            REQUEUED.fetch_add(1, Ordering::Relaxed);
            send(job, true);
        }
    }
}

/// Workers run below normal priority (VIctory 35b054e), raised by hurry() while
/// the title waits on their build. five-star-recomp 0566186 went back to normal
/// priority for its compile threads (reverting its 24cd32d, which we never
/// had); TUXBOX_PIPELINE_PRIORITY=normal does the same here.
fn worker_priority() {
    use windows::Win32::System::Threading::*;
    let priority = if PREFETCHER.with(|p| p.get()) {
        THREAD_PRIORITY_IDLE
    } else if crate::env_toggle!("TUXBOX_PIPELINE_PRIORITY") == Some("normal") {
        THREAD_PRIORITY_NORMAL
    } else {
        THREAD_PRIORITY_BELOW_NORMAL
    };
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), priority) };
}

/// Microseconds between the last two presents (smoothed), and when the last
/// one came (ms since START).
static FRAME_US: AtomicU64 = AtomicU64::new(0);
static PRESENT_AT_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

pub(crate) fn frame_presented() {
    let now = now_ms();
    let last = PRESENT_AT_MS.swap(now, Ordering::Relaxed);
    if last != 0 {
        let us = (now - last).saturating_mul(1000);
        let old = FRAME_US.load(Ordering::Relaxed);
        FRAME_US.store(if old == 0 { us } else { (old * 7 + us) / 8 }, Ordering::Relaxed);
    }
}

/// test145 (as test117a before it): eight pipeline workers and four
/// prefetchers compiling in the driver (nvgpucomp64, ~3,700 samples each)
/// took the intro to 12.6-13.7 fps with Bink at 5.8 frames a second and the
/// audio glitching. Below-normal priority does not keep them off the title:
/// the title's threads are pinned to the cores xmem maps the console's onto,
/// and inside the driver the workers hold its locks. So while the title
/// presents slower than TUXBOX_PIPELINE_FPS_FLOOR (30) frames a second, only
/// the first two pipeline workers take jobs; the others and the prefetchers
/// wait. A title thread waiting on a pipeline (TITLE_HELD) or a stretch with
/// no presents (a loading screen that does not draw) lets them all build.
/// VIctory-fxceless runs n/2 workers below normal with no prefetchers or
/// compile pool beside them (make.rs:295-310). TUXBOX_PIPELINE_FPS_FLOOR=0
/// never holds them.
fn throttled() -> bool {
    static FLOOR: OnceLock<u64> = OnceLock::new();
    let floor = *FLOOR.get_or_init(|| crate::env_toggle!("TUXBOX_PIPELINE_FPS_FLOOR").and_then(|v| v.trim().parse().ok()).unwrap_or(30));
    if floor == 0 || TITLE_HELD.load(Ordering::Relaxed) != 0 {
        return false;
    }
    let at = PRESENT_AT_MS.load(Ordering::Relaxed);
    let frame = FRAME_US.load(Ordering::Relaxed);
    at != 0 && now_ms().saturating_sub(at) < 1000 && frame > 1_000_000 / floor
}

/// Pipeline workers that keep building while the title is throttled
/// (TUXBOX_PIPELINE_THROTTLE_KEEP, default 2 as before). test152: the two
/// kept workers spent 73 ms CPU a frame each inside nvgpucomp64 working the
/// eager backlog at 11 fps while the title's submit thread was the limit; 0
/// holds them all (a title wait or a bind steal still builds on demand).
fn throttle_keep() -> usize {
    static KEEP: OnceLock<usize> = OnceLock::new();
    *KEEP.get_or_init(|| crate::env_toggle!("TUXBOX_PIPELINE_THROTTLE_KEEP").and_then(|v| v.trim().parse().ok()).unwrap_or(2))
}

/// A throttled worker waits here before taking its next job.
fn fxceless_shape() -> bool {
    crate::env_toggle!("TUXBOX_PIPELINE_SHAPE") == Some("fxceless")
}

/// test153: in a world load (152 queued, the title at 12 fps) the held
/// workers stayed held while pipelines the title had bound queued behind the
/// two kept ones' eager backlog, and the title waited 4.09 ms a frame on them
/// or dropped their draws. A held pipeline worker now takes a job while one
/// is queued as bound (`Jobs::bound`, taken first): that is work the title
/// waits on, which the hold exists to make room for. Prefetchers stay held.
/// TUXBOX_THROTTLE_RELEASE_BOUND=0 holds them as before.
fn bound_waiting() -> bool {
    if crate::env_toggle!("TUXBOX_THROTTLE_RELEASE_BOUND") == Some("0") {
        return false;
    }
    let g = JOBS.0.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().is_some_and(|jobs| !jobs.bound.is_empty())
}

fn hold_while_throttled(release_bound: bool) {
    if fxceless_shape() {
        return;
    }
    static HELD: AtomicU64 = AtomicU64::new(0);
    let mut noted = false;
    while throttled() && !(release_bound && bound_waiting()) {
        if !noted {
            noted = true;
            let n = HELD.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "background pipelines: a worker holds while the title presents at {:.1} fps, under TUXBOX_PIPELINE_FPS_FLOOR ({n} so far; 0 never holds)",
                    1e6 / FRAME_US.load(Ordering::Relaxed).max(1) as f64));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// xcl's idea (re-derived): a pipeline the title is waiting on that a worker
/// is already building gets that worker raised to normal priority; the
/// worker drops back after the build. TUXBOX_PIPELINE_HURRY=0 off.
fn hurry(slot: &Upgrade) {
    let id = slot.running.load(Ordering::Acquire);
    if id == 0 || slot.built.get().is_some() || crate::env_toggle!("TUXBOX_PIPELINE_HURRY") == Some("0") {
        return;
    }
    use windows::Win32::System::Threading::*;
    let Ok(thread) = (unsafe { OpenThread(THREAD_SET_LIMITED_INFORMATION, false, id) }) else { return };
    let raised = unsafe { SetThreadPriority(thread, THREAD_PRIORITY_NORMAL) }.is_ok();
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(thread) };
    static HURRIED: AtomicU64 = AtomicU64::new(0);
    if raised {
        let n = HURRIED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a worker building a pipeline the title waits on raised to normal priority ({n} so far; TUXBOX_PIPELINE_HURRY=0 off)"));
        }
    }
}

/// A pipeline the title created but no command list bound yet. Its proxy
/// holds a shared placeholder; nothing is translated or compiled until a
/// command list binds it.
pub(in crate::device) fn lazy(device: IUnknown, work: Work) -> Arc<Upgrade> {
    let slot = Arc::new(Upgrade::default());
    *slot.deferred.lock().unwrap_or_else(|e| e.into_inner()) = Some((device, work));
    LAZY.fetch_add(1, Ordering::Relaxed);
    slot
}

static LAZY: AtomicU64 = AtomicU64::new(0);

struct Placeholder(usize, ID3D12PipelineState);
// SAFETY: D3D12 objects are free-threaded; it is never mutated.
unsafe impl Send for Placeholder {}
// SAFETY: as above.
unsafe impl Sync for Placeholder {}

/// One resource-free compute pipeline on an empty root signature, shared by
/// every lazy pipeline of the (first) device until it is made.
pub(in crate::device) fn placeholder(device: &Device) -> Option<ID3D12PipelineState> {
    static PLACEHOLDER: OnceLock<Option<Placeholder>> = OnceLock::new();
    let owner = ptr::from_ref(device) as usize;
    let made = PLACEHOLDER.get_or_init(|| {
        let code = super::super::fallback::shader(super::super::fallback::Stage::Compute)?;
        let mut blob = None;
        let desc = D3D12_ROOT_SIGNATURE_DESC::default();
        unsafe { D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None) }.ok()?;
        let blob = blob?;
        let bytes = unsafe { slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) };
        let root: ID3D12RootSignature = unsafe { device.native.CreateRootSignature(0, bytes) }.ok()?;
        let compute = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: core::mem::ManuallyDrop::new(Some(root)),
            CS: super::super::fallback::bytecode(code),
            ..Default::default()
        };
        let mut compute = compute;
        let pipeline = unsafe { device.native.CreateComputePipelineState(&compute) };
        unsafe { core::mem::ManuallyDrop::drop(&mut compute.pRootSignature) };
        Some(Placeholder(owner, pipeline.ok()?))
    });
    made.as_ref().filter(|p| p.0 == owner).map(|p| p.1.clone())
}

/// A self-owned deep copy of a guest pipeline stream. Guest pointers are only
/// valid during CreatePipelineState, so every referenced array, string and
/// shader is copied, and the guest root signature is retained.
pub(in crate::device) struct OwnedStream {
    storage: Vec<MaybeUninit<usize>>,
    size: usize,
    _bytes: Vec<Vec<u8>>,
    _names: Vec<CString>,
    _elements: Vec<Vec<D3D12_INPUT_ELEMENT_DESC>>,
    _declarations: Vec<Vec<D3D12_SO_DECLARATION_ENTRY>>,
    _strides: Vec<Vec<u32>>,
    _views: Vec<Vec<D3D12_VIEW_INSTANCE_LOCATION>>,
    _root: Option<IUnknown>,
}

unsafe fn copy_array<T: Copy>(data: *const T, count: usize) -> Option<Vec<T>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if data.is_null() || count > 4096 {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(data, count) }.to_vec())
}

unsafe fn copy_name(names: &mut Vec<CString>, name: PCSTR) -> Option<PCSTR> {
    if name.is_null() {
        return Some(name);
    }
    let owned = unsafe { CStr::from_ptr(name.0.cast()) }.to_owned();
    let pointer = PCSTR(owned.as_ptr().cast());
    names.push(owned);
    Some(pointer)
}

impl OwnedStream {
    /// Copies a stream that `create` already validated; `None` for anything
    /// it cannot own (unknown or repeated subobjects).
    pub(in crate::device) unsafe fn copy(desc: &D3D12_PIPELINE_STATE_STREAM_DESC) -> Option<Self> {
        let size = desc.SizeInBytes;
        let input = desc.pPipelineStateSubobjectStream.cast::<u8>();
        if input.is_null() || size == 0 || size > 4096 || !size.is_multiple_of(size_of::<usize>()) {
            return None;
        }
        let mut storage = vec![MaybeUninit::<usize>::uninit(); size / size_of::<usize>()];
        let output = storage.as_mut_ptr().cast::<u8>();
        unsafe { ptr::copy_nonoverlapping(input, output, size) };
        let mut owned = OwnedStream {
            storage: Vec::new(),
            size,
            _bytes: Vec::new(),
            _names: Vec::new(),
            _elements: Vec::new(),
            _declarations: Vec::new(),
            _strides: Vec::new(),
            _views: Vec::new(),
            _root: None,
        };
        let mut seen = 0u32;
        let mut at = 0;
        while at < size {
            let record = unsafe { output.add(at) };
            let tag = unsafe { record.cast::<i32>().read() };
            let (offset, bytes) = layout(tag)?;
            if bytes > size - at || seen & (1 << tag) != 0 {
                return None;
            }
            seen |= 1 << tag;
            let payload = unsafe { record.add(offset) };
            match tag {
                0 => {
                    let guest = unsafe { payload.cast::<*mut c_void>().read() };
                    // SAFETY: create() verified a live guest root (or null).
                    owned._root = unsafe { IUnknown::from_raw_borrowed(&guest) }.cloned();
                }
                1..=6 | AMPLIFICATION_X | MESH_X => {
                    let shader = unsafe { &mut *payload.cast::<D3D12_SHADER_BYTECODE>() };
                    if shader.BytecodeLength != 0 {
                        if shader.pShaderBytecode.is_null() || shader.BytecodeLength > isize::MAX as usize {
                            return None;
                        }
                        let code = unsafe {
                            slice::from_raw_parts(shader.pShaderBytecode.cast::<u8>(), shader.BytecodeLength)
                        }
                        .to_vec();
                        shader.pShaderBytecode = code.as_ptr().cast();
                        owned._bytes.push(code);
                    }
                }
                7 => {
                    let so = unsafe { &mut *payload.cast::<D3D12_STREAM_OUTPUT_DESC>() };
                    let mut entries = unsafe { copy_array(so.pSODeclaration, so.NumEntries as usize) }?;
                    for entry in &mut entries {
                        entry.SemanticName = unsafe { copy_name(&mut owned._names, entry.SemanticName) }?;
                    }
                    let strides = unsafe { copy_array(so.pBufferStrides, so.NumStrides as usize) }?;
                    so.pSODeclaration = entries.as_ptr();
                    so.pBufferStrides = strides.as_ptr();
                    owned._declarations.push(entries);
                    owned._strides.push(strides);
                }
                12 => {
                    let input = unsafe { &mut *payload.cast::<D3D12_INPUT_LAYOUT_DESC>() };
                    let mut elements =
                        unsafe { copy_array(input.pInputElementDescs, input.NumElements as usize) }?;
                    for element in &mut elements {
                        element.SemanticName = unsafe { copy_name(&mut owned._names, element.SemanticName) }?;
                    }
                    input.pInputElementDescs = elements.as_ptr();
                    owned._elements.push(elements);
                }
                // create() discards console PSO caches; never keep a guest pointer.
                19 => unsafe {
                    payload.cast::<D3D12_CACHED_PIPELINE_STATE>().write(Default::default())
                },
                22 => {
                    let views = unsafe { &mut *payload.cast::<D3D12_VIEW_INSTANCING_DESC>() };
                    let locations = unsafe {
                        copy_array(views.pViewInstanceLocations, views.ViewInstanceCount as usize)
                    }?;
                    views.pViewInstanceLocations = locations.as_ptr();
                    owned._views.push(locations);
                }
                // The trailing console stencil record ends the stream.
                STENCIL_X => break,
                _ => {}
            }
            at += bytes;
        }
        owned.storage = storage;
        Some(owned)
    }

    fn desc(&mut self) -> D3D12_PIPELINE_STATE_STREAM_DESC {
        D3D12_PIPELINE_STATE_STREAM_DESC {
            SizeInBytes: self.size,
            pPipelineStateSubobjectStream: self.storage.as_mut_ptr().cast(),
        }
    }

    /// Bytes this copy owns (the derived-pipeline store's budget).
    pub(in crate::device) fn owned_bytes(&self) -> usize {
        self.size + self._bytes.iter().map(Vec::len).sum::<usize>()
    }

    /// This stream with each `(tags, payload)` written over the first record
    /// whose tag is listed (the payload's leading bytes), else added as a new
    /// record of the first tag before the console stencil record. That record
    /// is rewritten to agree with a patched depth-stencil state. The result's
    /// pointers still name this copy's arrays, so it lives no longer than
    /// `self`. For CreateDerivedPipelineStateX (palmetto b00248ca).
    pub(in crate::device) fn patched(&self, patches: &[(&[i32], &[u8])]) -> Option<Vec<MaybeUninit<usize>>> {
        let input = unsafe { slice::from_raw_parts(self.storage.as_ptr().cast::<u8>(), self.size) };
        let mut out: Vec<u8> = Vec::with_capacity(self.size + 512);
        let mut applied = vec![false; patches.len()];
        let mut stencil: Option<D3D12_DEPTH_STENCIL_DESC1> = None;
        let mut at = 0;
        // A depth part added where the stream had none also sets the closing
        // STENCIL_X record, as one already in the stream does (gap in cefdfdb;
        // TUXBOX_DERIVED_STENCIL=0 leaves the record as it was).
        let added_stencil = crate::env_toggle!("TUXBOX_DERIVED_STENCIL") != Some("0");
        let add_missing = |out: &mut Vec<u8>, applied: &mut [bool], stencil: &mut Option<D3D12_DEPTH_STENCIL_DESC1>| -> Option<()> {
            for (i, (tags, payload)) in patches.iter().enumerate() {
                if applied[i] {
                    continue;
                }
                let (offset, bytes) = super::layout(tags[0])?;
                let start = out.len();
                out.resize(start + bytes, 0);
                out[start..start + 4].copy_from_slice(&tags[0].to_le_bytes());
                let n = payload.len().min(bytes - offset);
                out[start + offset..start + offset + n].copy_from_slice(&payload[..n]);
                applied[i] = true;
                if added_stencil && matches!(tags[0], 11 | 21) {
                    let mut depth = D3D12_DEPTH_STENCIL_DESC1::default();
                    let n = (bytes - offset).min(size_of::<D3D12_DEPTH_STENCIL_DESC1>());
                    unsafe { ptr::copy_nonoverlapping(out[start + offset..].as_ptr(), (&raw mut depth).cast::<u8>(), n) };
                    *stencil = Some(depth);
                }
            }
            Some(())
        };
        while at < self.size {
            let tag = i32::from_le_bytes(input[at..at + 4].try_into().ok()?);
            // Tags 27 and 28 (console records after STENCIL_X) have no known
            // layout: refused, so the base pipeline is returned (cefdfdb).
            if matches!(tag, 27 | 28) {
                return None;
            }
            let (offset, bytes) = super::layout(tag)?;
            let start = out.len();
            out.extend_from_slice(&input[at..at + bytes]);
            if tag == super::STENCIL_X {
                // It ends the stream: a new record goes before it.
                out.truncate(start);
                add_missing(&mut out, &mut applied, &mut stencil)?;
                let start = out.len();
                out.extend_from_slice(&input[at..at + bytes]);
                if let Some(depth) = stencil {
                    let enabled = u8::from(depth.StencilEnable.as_bool());
                    let payload = &mut out[start + offset..start + offset + 18];
                    payload.fill(0);
                    payload[12..18].copy_from_slice(&[depth.StencilReadMask, depth.StencilWriteMask, 0, 0, enabled, enabled]);
                }
                break;
            }
            if let Some(i) = patches.iter().position(|(tags, _)| tags.contains(&tag)) {
                if !applied[i] {
                    let payload = patches[i].1;
                    let n = payload.len().min(bytes - offset);
                    out[start + offset..start + offset + n].copy_from_slice(&payload[..n]);
                    applied[i] = true;
                }
            }
            if matches!(tag, 11 | 21) {
                let mut depth = D3D12_DEPTH_STENCIL_DESC1::default();
                let n = (bytes - offset).min(size_of::<D3D12_DEPTH_STENCIL_DESC1>());
                unsafe { ptr::copy_nonoverlapping(out[start + offset..].as_ptr(), (&raw mut depth).cast::<u8>(), n) };
                stencil = Some(depth);
            }
            at += bytes;
        }
        add_missing(&mut out, &mut applied, &mut stencil)?;
        if out.len() > 4096 || !out.len().is_multiple_of(size_of::<usize>()) {
            return None;
        }
        let mut storage = vec![MaybeUninit::<usize>::uninit(); out.len() / size_of::<usize>()];
        unsafe { ptr::copy_nonoverlapping(out.as_ptr(), storage.as_mut_ptr().cast::<u8>(), out.len()) };
        Some(storage)
    }
}

/// A self-owned copy of a slot 62 compute descriptor (no extensions).
pub(in crate::device) struct OwnedCompute {
    code: Vec<u8>,
    root: Option<ID3D12RootSignature>,
    node_mask: u32,
    flags: D3D12_PIPELINE_STATE_FLAGS,
}

impl OwnedCompute {
    pub(in crate::device) fn copy(desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC, code: &[u8]) -> Self {
        OwnedCompute {
            code: code.to_vec(),
            // Retains the guest root proxy.
            root: (*desc.pRootSignature).clone(),
            node_mask: desc.NodeMask,
            flags: desc.Flags,
        }
    }

    fn desc(&self) -> D3D12_COMPUTE_PIPELINE_STATE_DESC {
        D3D12_COMPUTE_PIPELINE_STATE_DESC {
            // SAFETY: a borrowed copy; ManuallyDrop never releases it.
            pRootSignature: core::mem::ManuallyDrop::new(unsafe { core::mem::transmute_copy(&self.root) }),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: self.code.as_ptr().cast(),
                BytecodeLength: self.code.len(),
            },
            NodeMask: self.node_mask,
            Flags: self.flags,
            ..Default::default()
        }
    }
}

pub(in crate::device) enum Work {
    Stream(OwnedStream),
    Compute(OwnedCompute),
}

struct Job {
    /// The guest device, retained until the worker is done with it.
    device: IUnknown,
    work: Work,
    /// A graphics pipeline: prefetched ahead of compute ones (see `next_job`).
    graphics: bool,
    slot: Arc<Upgrade>,
}

// SAFETY: the stream owns all its memory; D3D12 and proxy objects are free-threaded.
unsafe impl Send for Job {}

static WAITING: AtomicU64 = AtomicU64::new(0);
static MADE: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A prefetch worker: idle priority, prefetches only.
    static PREFETCHER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set when a pipeline created on this thread fell back to a null pipeline.
    static NULLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set while a dedicated worker builds a job nothing has bound yet.
    static PREFETCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The slot of the job this worker builds.
    static CURRENT: std::cell::RefCell<Option<Arc<Upgrade>>> = const { std::cell::RefCell::new(None) };
}

/// Whether the title has since bound or waited on the job this worker
/// started as a prefetch: `compile_budget` re-asks at each pause, so a
/// prefetch the title turns to wait on stops waiting for memory (xcl
/// 724b9e2 `big_compile_turn`, GPL-2.0-only, credit to its authors).
/// TUXBOX_BUDGET_RECHECK=0 decides once, at entry.
pub(in crate::device) fn waited_on() -> bool {
    crate::env_toggle!("TUXBOX_BUDGET_RECHECK") != Some("0")
        && CURRENT.with(|c| c.borrow().as_ref().is_some_and(|s| s.wanted.load(Ordering::Acquire)))
}

/// Whether this thread's driver compile is a prefetch no title thread waits
/// on: a dedicated pipeline worker building a created, not yet bound, job.
/// Only these wait in `compile_budget` (five-star-recomp and xcl 724b9e2:
/// a pipeline a game thread waits for is never gated).
pub(in crate::device) fn prefetching() -> bool {
    PREFETCH.with(|p| p.get()) && !STEALING.with(|s| s.get())
}

/// Whether this thread is a pipeline worker (its builds are never deferred).
pub(in crate::device) fn on_worker() -> bool {
    WORKER.with(|w| w.get())
}

/// The pipeline being created on this thread got null shaders (see `fallback`).
pub(in crate::device) fn mark_nulled() {
    NULLED.with(|n| n.set(true));
}

/// Whether a pipeline an earlier run bound is made on the title's thread
/// inside its Create call (TUXBOX_EAGER_ON_TITLE=1, the old way) instead of
/// being handed to the workers at once behind the placeholder. Made on the
/// title's thread, one pipeline whose driver compile never finished
/// (nvgpucomp64 busy for minutes on tuxbox-pipeline) held the title's loader
/// in its Create call: every run after the first, which had made it lazily,
/// stayed black after sign-in. The first bind still waits for it within the
/// frame's translator budget.
pub(in crate::device) fn eager_on_title() -> bool {
    crate::env_toggle!("TUXBOX_EAGER_ON_TITLE") == Some("1")
}

/// Hands a pipeline to the workers; the returned slot fills once its host
/// pipeline exists.
pub(in crate::device) fn queue(device: IUnknown, mut work: Work) -> Arc<Upgrade> {
    let slot = Arc::new(Upgrade::default());
    let graphics = match &mut work {
        Work::Stream(stream) => unsafe { super::is_graphics(&stream.desc()) },
        Work::Compute(_) => false,
    };
    slot.compute.store(!graphics, Ordering::Release);
    send(Job { device, work, graphics, slot: slot.clone() }, false);
    slot
}

/// Jobs waiting for a worker: those a command list has bound (`bound`) go
/// first. test64 queued some 1100 pipelines earlier runs had used at the
/// intro, first in first out, and the few the title bound waited behind them
/// while each bind spent the frame's budget waiting: 1.8 fps.
/// TUXBOX_PIPELINE_PRIORITY=0 keeps one queue in creation order.
#[derive(Default)]
struct Jobs {
    bound: std::collections::VecDeque<Job>,
    created: std::collections::VecDeque<Job>,
}

static JOBS: (Mutex<Option<Jobs>>, std::sync::Condvar) = (Mutex::new(None), std::sync::Condvar::new());

fn priority() -> bool {
    crate::env_toggle!("TUXBOX_PIPELINE_PRIORITY") != Some("0")
}

/// A bound pipeline still in the creation-order queue moves to the front.
fn promote(slot: &Arc<Upgrade>) {
    slot.wanted.store(true, Ordering::Release);
    if slot.built.get().is_some() || slot.failed.load(Ordering::Relaxed) || !priority() {
        return;
    }
    {
        // In the compile pool: ahead of the prefetches there, and no longer one.
        let mut q = COMPILE.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = q.iter().position(|(j, p)| *p && Arc::ptr_eq(&j.slot, slot))
            && let Some((job, _)) = q.remove(at)
        {
            let first = q.iter().position(|(_, p)| *p).unwrap_or(q.len());
            q.insert(first, (job, false));
            return;
        }
    }
    let mut g = JOBS.0.lock().unwrap_or_else(|e| e.into_inner());
    let Some(jobs) = g.as_mut() else { return };
    if let Some(at) = jobs.created.iter().position(|j| Arc::ptr_eq(&j.slot, slot))
        && let Some(job) = jobs.created.remove(at)
    {
        jobs.bound.push_back(job);
        static PROMOTED: AtomicU64 = AtomicU64::new(0);
        let n = PROMOTED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "background pipelines: a bound pipeline moved ahead of {at} queued ones ({n} so far; TUXBOX_PIPELINE_PRIORITY=0 off)"));
        }
    }
}

/// test108: compute pipelines the title bound waited their full 3 s while
/// queued behind workers held up by compiles waiting for memory, so their
/// dispatches (and the progress writes the title's watchdog waits on) were
/// lost. A pipeline the title waits on that no worker has started is taken
/// from the queue and made on the title's thread. TUXBOX_COMPUTE_STEAL=0 off.
fn steal_on() -> bool {
    crate::env_toggle!("TUXBOX_COMPUTE_STEAL") != Some("0")
}

fn steal(slot: &Arc<Upgrade>) -> Option<Job> {
    if slot.built.get().is_some() || slot.failed.load(Ordering::Relaxed) || slot.running.load(Ordering::Acquire) != 0 {
        return None;
    }
    let mut g = JOBS.0.lock().unwrap_or_else(|e| e.into_inner());
    let jobs = g.as_mut()?;
    if let Some(at) = jobs.bound.iter().position(|j| Arc::ptr_eq(&j.slot, slot)) {
        return jobs.bound.remove(at);
    }
    let at = jobs.created.iter().position(|j| Arc::ptr_eq(&j.slot, slot))?;
    jobs.created.remove(at)
}

/// rebox c78d9f4/f0bcabe (GPL-2.0, credit to its authors): a scene queues
/// hundreds of first compiles at once, and stages drawing into display-sized
/// targets (the UI, text, the composite) waited behind them; on the title's
/// thread a stolen build was cut at the bind's wait, so a loading screen's
/// layer went black whenever it needed a new pipeline. Such a pipeline is
/// now built at once on a thread of its own, as long as it takes, at most
/// 2 at a time (else it goes to the front of the workers' queue); the bind
/// still waits only up to its limit. TUXBOX_DISPLAY_BUILD_NOW=0 builds it on
/// the title's thread within the bind's wait, as before.
fn display_now() -> bool {
    crate::env_toggle!("TUXBOX_DISPLAY_BUILD_NOW") != Some("0")
}

fn build_display(job: Job) {
    static RUNNING: AtomicU64 = AtomicU64::new(0);
    static BUILT: AtomicU64 = AtomicU64::new(0);
    let front = |job: Job| {
        let (lock, ready) = &JOBS;
        lock.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).bound.push_front(job);
        ready.notify_one();
    };
    if RUNNING.fetch_add(1, Ordering::AcqRel) >= 2 {
        RUNNING.fetch_sub(1, Ordering::AcqRel);
        return front(job);
    }
    // Shared so a thread that could not start hands the job back.
    let shared = Arc::new(Mutex::new(Some(job)));
    let spawned = std::thread::Builder::new()
        .name("tuxbox-display-pipeline".into())
        .stack_size(crate::big_stack::STACK_BYTES)
        .spawn({
            let shared = shared.clone();
            move || {
                WORKER.with(|w| w.set(true));
                crate::big_stack::mark_big();
                crate::device::shader_cache::wait_unbounded();
                let job = shared.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(job) = job {
                    build(job);
                }
                RUNNING.fetch_sub(1, Ordering::AcqRel);
                let n = BUILT.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= 8 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "background pipelines: a display-sized draw's pipeline built on its own thread ({n} so far; TUXBOX_DISPLAY_BUILD_NOW=0 off)"));
                }
            }
        });
    if spawned.is_err() {
        RUNNING.fetch_sub(1, Ordering::AcqRel);
        if let Some(job) = shared.lock().unwrap_or_else(|e| e.into_inner()).take() {
            front(job);
        }
    }
}

/// Builds a job on this thread as a worker would, keeping its priority. Its
/// translator wait lasts up to `limit` (the title's wait) rather than the frame
/// budget: test109 lost 61 compute pipelines to E_NOTIMPL when a spent budget
/// ended the stolen build at once and marked it failed for good. A build that
/// runs out of time goes back to the front of the workers' queue.
fn build_here(job: Job, limit: std::time::Duration) {
    use windows::Win32::System::Threading::*;
    let started = std::time::Instant::now();
    let priority = unsafe { GetThreadPriority(GetCurrentThread()) };
    crate::big_stack::run(|| {
        let was = WORKER.with(|w| w.replace(true));
        let stealing = STEALING.with(|s| s.replace(true));
        let deadline = crate::device::shader_cache::wait_until(Some(started + limit));
        let _ = crate::device::shader_cache::take_missed();
        build(job);
        crate::device::shader_cache::wait_until(deadline);
        STEALING.with(|s| s.set(stealing));
        WORKER.with(|w| w.set(was));
    });
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(priority)) };
    static STOLEN: AtomicU64 = AtomicU64::new(0);
    let n = STOLEN.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "background pipelines: a queued pipeline the title waits on made on its thread in {:.1} ms ({n} so far; TUXBOX_COMPUTE_STEAL=0 off)",
            started.elapsed().as_secs_f64() * 1e3));
    }
}

/// The next job, and whether it is a prefetch (created, not yet bound). A
/// prefetch worker takes only prefetches.
fn next_job(prefetch_only: bool) -> (Job, bool) {
    let (lock, ready) = &JOBS;
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let jobs = g.get_or_insert_with(Default::default);
        if !prefetch_only && let Some(job) = jobs.bound.pop_front() {
            return (job, false);
        }
        // XCL pso_async.rs (f14d3ff, GPL-2.0-only, via ZigZagBoi1/ScarlettPort):
        // after the builds a title waits on, graphics before compute. test140's
        // last 10 s recorded 2 of 2291 draws and 398 of 2537 dispatches.
        // Looks at the first 512 queued; TUXBOX_GRAPHICS_FIRST=0 keeps
        // creation order.
        let first = if crate::env_toggle!("TUXBOX_GRAPHICS_FIRST") != Some("0") {
            jobs.created.iter().take(512).position(|j| j.graphics).unwrap_or(0)
        } else { 0 };
        if let Some(job) = jobs.created.remove(first) {
            return (job, true);
        }
        g = ready.wait(g).unwrap_or_else(|e| e.into_inner());
    }
}

fn send(job: Job, bound: bool) {
    static STARTED: OnceLock<bool> = OnceLock::new();
    let started = *STARTED.get_or_init(|| {
        // test117a: with every translation prewarmed, eight workers created
        // pipelines at once (two of them 375 ms of CPU a frame each, in the
        // driver's DXIL-to-ISA compile) and the intro fell from 39 to 2.6-7.8
        // fps, so the count went down to 2. But two workers cannot drain the
        // ~80,000 pipelines the title creates during loading, and their draws
        // are dropped meanwhile: VIctory-fxceless (make.rs:295-300, GPL-3.0,
        // credit to its authors) builds with half the cores, 2 to 8, below
        // normal priority, and draws where we do not. Ours run below normal
        // too (worker_priority), and a frame now waits for its pipelines
        // within the pipeline budget instead of dropping their draws, so half
        // the cores is the default again. TUXBOX_PIPELINE_WORKERS=N sets the
        // count (2 is test117a's); =auto is the default.
        // test156: 8 workers and 4 prefetchers each spent about 3.6 s of CPU
        // per frame in nvgpucomp64 (256 pipelines made all run, each a long
        // driver compile), taking 12 of the title's cores. The cap keeps every
        // compile thread together to a quarter of the cores (2 to 4 workers,
        // up to 2 prefetchers). TUXBOX_PIPELINE_THREAD_CAP=0 keeps half the
        // cores (2 to 8) and a quarter for prefetch, as before.
        let capped = crate::env_toggle!("TUXBOX_PIPELINE_THREAD_CAP") != Some("0");
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let auto = || if capped { (cores / 4).clamp(2, 4) } else { (cores / 2).clamp(2, 8) };
        let workers = match crate::env_toggle!("TUXBOX_PIPELINE_WORKERS") {
            Some(v) => v.trim().parse::<usize>().ok().filter(|&n| n > 0).map_or_else(auto, |n| n.min(16)),
            None => auto(),
        };
        // test139: the title creates some 84,000 pipelines at load and the
        // cutscene after it binds about 1,200 of them; the ones earlier runs
        // bound are queued as prefetches at creation (pipeline_cache), but two
        // workers were still compiling them in the driver when the cutscene
        // started, and every frame waited. Idle-priority prefetch workers
        // build them on the cores the title leaves free during its loading
        // screens; a bound job never waits behind them (they take only
        // prefetches) and one the title then waits on is raised by `hurry`.
        // TUXBOX_PREFETCH_WORKERS=N sets the count (default a quarter of the
        // cores, up to 4; 0 off).
        // VIctory-fxceless's shape (make.rs:295-300, GPL-3.0, credit to its
        // authors): half the cores, 2 to 8, and no prefetch workers, with no
        // fps hold. The hold (bb35fd2) treats a backlog's symptom; this tests
        // the shape that plays the intro smoothly. TUXBOX_PIPELINE_SHAPE=fxceless
        // turns it on (TUXBOX_PIPELINE_WORKERS still sets the count).
        let fxceless = fxceless_shape();
        let prefetchers = if fxceless { Some("0".into()) } else { crate::env_toggle!("TUXBOX_PREFETCH_WORKERS") }
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or_else(|| if capped { (cores / 8).min(2) } else { (cores / 4).min(4) })
            .min(16);
        crate::diagnostics::log_line(format_args!(
            "pipeline workers: {workers} (TUXBOX_PIPELINE_WORKERS=N or auto), prefetch workers {prefetchers} (TUXBOX_PREFETCH_WORKERS=N), of {cores} cores (TUXBOX_PIPELINE_THREAD_CAP=0 uncapped){}", if fxceless { ", fxceless shape: no fps hold (TUXBOX_PIPELINE_SHAPE)" } else { "" }));
        let mut any = false;
        for i in 0..workers + prefetchers {
            let prefetch_only = i >= workers;
            let spawned = std::thread::Builder::new()
                .name(if prefetch_only { format!("tuxbox-prefetch-{}", i - workers) } else { format!("tuxbox-pipelines-{i}") })
                .stack_size(crate::big_stack::STACK_BYTES)
                .spawn(move || {
                    WORKER.with(|w| w.set(true));
                    crate::big_stack::mark_big();
                    crate::device::shader_cache::wait_unbounded();
                    // Below the title's own loading and recording threads.
                    PREFETCHER.with(|p| p.set(prefetch_only));
                    worker_priority();
                    // Ready workers wait only briefly on the translator.
                    crate::device::shader_cache::patience(handoff_wait());
                    loop {
                        if prefetch_only || i >= throttle_keep() {
                            hold_while_throttled(!prefetch_only);
                        }
                        let (job, prefetch) = next_job(prefetch_only);
                        PREFETCH.with(|p| p.set(prefetch));
                        HANDING.with(|h| h.set(handoff_wait().map(|_| prefetch)));
                        let _ = crate::device::shader_cache::take_missed();
                        build(job);
                        HANDING.with(|h| h.set(None));
                        PREFETCH.with(|p| p.set(false));
                    }
                });
            any |= spawned.is_ok() && !prefetch_only;
        }
        any
    });
    if !started {
        // No worker: the pipeline is never made.
        return;
    }
    WAITING.fetch_add(1, Ordering::Relaxed);
    let (lock, ready) = &JOBS;
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let jobs = g.get_or_insert_with(Default::default);
    if bound && priority() {
        jobs.bound.push_back(job);
    } else {
        jobs.created.push_back(job);
    }
    drop(g);
    // Prefetch workers skip bound jobs: wake every worker so one that takes it wakes.
    ready.notify_all();
}

/// OpenScarlett-OCE f8f5b79/e301ed9's tiered pools (the idea, re-derived;
/// AGPL, nothing copied): our ready workers waited unbounded on the
/// translator, so two uncached big programs held both workers for tens of
/// seconds and every pipeline behind them, cached or not, waited too (the
/// title's watchdog kills). A ready worker now waits on one translator
/// request at most TUXBOX_PIPELINE_HANDOFF_MS (default 150) and then hands
/// the pipeline to a below-normal compile pool (TUXBOX_COMPILE_WORKERS,
/// default 2) that waits as long as the translation takes. The translation
/// keeps running in the launcher meanwhile. A build a title thread waits on
/// stays on its worker. TUXBOX_PIPELINE_HANDOFF_MS=0 off.
fn handoff_wait() -> Option<std::time::Duration> {
    let ms = crate::env_toggle!("TUXBOX_PIPELINE_HANDOFF_MS").map_or(Some(150), |v| v.trim().parse::<u64>().ok());
    ms.filter(|&ms| ms > 0).map(std::time::Duration::from_millis)
}

/// Pipelines whose translations were not ready, and whether each is a prefetch.
static COMPILE: (Mutex<std::collections::VecDeque<(Job, bool)>>, std::sync::Condvar) =
    (Mutex::new(std::collections::VecDeque::new()), std::sync::Condvar::new());

fn hand_off(job: Job, prefetch: bool) {
    static STARTED: OnceLock<bool> = OnceLock::new();
    let started = *STARTED.get_or_init(|| {
        let workers = crate::env_toggle!("TUXBOX_COMPILE_WORKERS")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .map_or(2, |n| n.min(16));
        crate::diagnostics::log_line(format_args!(
            "compile pool: {workers} workers for pipelines whose translation is not ready (TUXBOX_COMPILE_WORKERS=N; TUXBOX_PIPELINE_HANDOFF_MS=0 off)"));
        let mut any = false;
        for i in 0..workers {
            let spawned = std::thread::Builder::new()
                .name(format!("tuxbox-compile-{i}"))
                .stack_size(crate::big_stack::STACK_BYTES)
                .spawn(move || {
                    WORKER.with(|w| w.set(true));
                    crate::big_stack::mark_big();
                    crate::device::shader_cache::wait_unbounded();
                    worker_priority();
                    loop {
                        let (lock, ready) = &COMPILE;
                        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let (job, prefetch) = loop {
                            if let Some(next) = q.pop_front() {
                                break next;
                            }
                            q = ready.wait(q).unwrap_or_else(|e| e.into_inner());
                        };
                        drop(q);
                        PREFETCH.with(|p| p.set(prefetch));
                        build(job);
                        PREFETCH.with(|p| p.set(false));
                    }
                });
            any |= spawned.is_ok();
        }
        any
    });
    if !started {
        // No compile pool: this worker keeps the job, first in line, and
        // waits on it unbounded from now on, as before.
        crate::device::shader_cache::patience(None);
        let (lock, ready) = &JOBS;
        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(Default::default).bound.push_front(job);
        drop(g);
        ready.notify_one();
        return;
    }
    let (lock, ready) = &COMPILE;
    let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
    // A bound pipeline goes ahead of prefetches.
    if prefetch {
        q.push_back((job, true));
    } else {
        let at = q.iter().position(|(_, p)| *p).unwrap_or(q.len());
        q.insert(at, (job, false));
    }
    let queued = q.len();
    drop(q);
    ready.notify_one();
    static HANDED: AtomicU64 = AtomicU64::new(0);
    let n = HANDED.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "background pipelines: a {} pipeline whose translation is not ready went to the compile pool ({queued} there, {n} so far; TUXBOX_PIPELINE_HANDOFF_MS=0 off)",
            if prefetch { "prefetched" } else { "bound" }));
    }
}

thread_local! {
    /// Set on a ready worker while it builds a job: Some(prefetch) when a
    /// translator miss hands the job to the compile pool.
    static HANDING: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// This title thread is building a stolen job (see build_here).
    static STEALING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn build(mut job: Job) {
    // The title released the pipeline before a worker reached it.
    if Arc::strong_count(&job.slot) > 1 {
        // SAFETY: a device proxy begins with its IUnknown identity, retained by the job.
        let device = unsafe { &*job.device.as_raw().cast::<Device>() };
        let iid = ID3D12PipelineState::IID;
        let mut out = ptr::null_mut();
        NULLED.with(|n| n.set(false));
        let _ = crate::device::shader_cache::take_misses();
        job.slot.running.store(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, Ordering::Release);
        CURRENT.with(|c| *c.borrow_mut() = Some(job.slot.clone()));
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                CURRENT.with(|c| *c.borrow_mut() = None);
            }
        }
        let _clear = Clear;
        // The ordinary entry points: on this thread nothing is deferred and
        // translator waits have no budget.
        let status = match &mut job.work {
            Work::Stream(stream) => {
                let desc = stream.desc();
                unsafe { super::create_pipeline(device, &desc, &iid, &mut out) }
            }
            Work::Compute(compute) => {
                let desc = compute.desc();
                unsafe { create_pipeline_x(device, &desc, 0, ptr::null(), &iid, &mut out) }
            }
        };
        job.slot.running.store(0, Ordering::Release);
        // Back below the title after any hurry().
        worker_priority();
        let nulled = NULLED.with(|n| n.replace(false));
        let made = (status.is_ok() && !out.is_null()).then(|| unsafe { IUnknown::from_raw(out) });
        let counter = match made {
            Some(proxy) if !nulled => {
                let _ = job.slot.built.set(Built(proxy));
                &MADE
            }
            _ if let Some(prefetch) = HANDING.with(|h| h.get())
                && crate::device::shader_cache::take_missed() =>
            {
                // Its translation is still being made: the compile pool waits
                // for it, and this worker goes on to pipelines that are ready.
                hand_off(job, prefetch);
                return;
            }
            _ if STEALING.with(|s| s.get()) && crate::device::shader_cache::take_missed() => {
                // Out of time, not untranslatable: back to the workers, first in line.
                let (lock, ready) = &JOBS;
                let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                g.get_or_insert_with(Default::default).bound.push_front(job);
                drop(g);
                ready.notify_one();
                return;
            }
            _ => {
                crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::PipelinesUntranslatable);
                let misses = crate::device::shader_cache::take_misses();
                let kind = match &job.work { Work::Stream(_) => "stream", Work::Compute(_) => "compute" };
                let why = if misses.is_empty() {
                    format!("{kind}: no shader request failed; create returned {:#x}{}", status.0 as u32, if nulled { " (null shaders)" } else { "" })
                } else {
                    let list: Vec<String> = misses.iter().map(|(k, w)| format!("{} ({w})", if k.is_empty() { "?" } else { k.as_str() })).collect();
                    format!("{kind}: {}", list.join("; "))
                };
                let _ = job.slot.why.set(why);
                job.slot.failed.store(true, Ordering::Relaxed);
                &FAILED
            }
        };
        let (lock, done) = &DONE;
        drop(lock.lock().unwrap_or_else(|e| e.into_inner()));
        done.notify_all();
        let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
        let left = WAITING.fetch_sub(1, Ordering::Relaxed) - 1;
        if count.is_power_of_two() || left == 0 {
            log(left);
        }
    } else {
        WAITING.fetch_sub(1, Ordering::Relaxed);
    }
}

fn log(left: u64) {
    let made = MADE.load(Ordering::Relaxed);
    let failed = FAILED.load(Ordering::Relaxed);
    let lazy = LAZY.load(Ordering::Relaxed);
    crate::diagnostics::log_once(&std::sync::Once::new(), || {
        format!("background pipelines: {made} made, {failed} untranslatable, {left} waiting, {lazy} created lazily")
    });
}
