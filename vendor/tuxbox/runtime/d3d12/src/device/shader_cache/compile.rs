//! Submit unprofiled console shaders to the running launcher's compiler.
use super::{Stage, read_portable_file, source_digest};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the title's threads may still wait on the translator, in
/// milliseconds (VIctory's per-frame budget). Loading before the first present
/// may wait 60 s in all; each present then grants half a second. A pipeline
/// whose translation misses the budget is created without it and its draws are
/// dropped, while the worker finishes and caches it for the next launch's
/// prewarm. The title's threads never block on the translator past the budget.
static WAIT_BUDGET_MS: AtomicI64 = AtomicI64::new(60_000);
/// test65 fell from 14.6 to 1.6 fps once the game's first scenes bound
/// pipelines the workers were still compiling (the driver, 565 ms a frame):
/// the binds spent up to 300 ms of the old 500 ms per frame waiting. 50 ms
/// keeps frames presenting and drops those draws until the workers finish.
const FRAME_BUDGET_MS: i64 = 50;

/// How long binds may still wait for a pipeline's worker in this frame, apart
/// from the translator polls above (VIctory-fxceless make.rs:97-108, GPL-3.0,
/// credit to its authors: 500 ms per title frame, shared by every thread).
/// Both used to draw on one 50 ms budget, so a frame whose translator polls
/// spent it dropped every draw of a pipeline finished a few ms later; fxceless
/// spends ten times that and draws (lit gameplay at about 2.5 fps where we
/// drew nothing). test65's fall from 14.6 to 1.6 fps was binds spending 300 ms
/// of a shared 500 ms: frames now slow down while pipelines build instead of
/// losing their draws. TUXBOX_PIPELINE_WAIT_MS sets it (50 is the old drop-
/// first trade, 0 never waits after the first present).
static PIPELINE_WAIT_MS: AtomicI64 = AtomicI64::new(60_000);
const PIPELINE_FRAME_MS: i64 = 500;

/// A title frame was presented: grant the next frame its wait budget.
/// TUXBOX_FRAME_WAIT_MS overrides it: lower trades dropped draws (pipelines
/// still building) for frame time; 0 never waits after the first present.
pub(crate) fn frame_presented() {
    let budget = crate::env_toggle!("TUXBOX_FRAME_WAIT_MS").and_then(|v| v.parse().ok()).unwrap_or(FRAME_BUDGET_MS);
    WAIT_BUDGET_MS.store(budget, Ordering::Relaxed);
    let pipelines = crate::env_toggle!("TUXBOX_PIPELINE_WAIT_MS").and_then(|v| v.parse().ok()).unwrap_or(PIPELINE_FRAME_MS);
    PIPELINE_WAIT_MS.store(pipelines, Ordering::Relaxed);
    crate::device::compute::refill_bind_wait();
    crate::device::stream::background::frame_presented();
}

/// Whether this thread's last request ran out of time (left set, unlike take_missed).
pub(crate) fn missed() -> bool {
    MISSED.with(|m| m.get())
}

thread_local! {
    /// Set when a request on this thread gave up because the budget was spent.
    static MISSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Pipeline workers wait for the translator without spending the budget.
    static UNBOUNDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A title thread building a pipeline it waits on (the compute steal)
    /// waits for the translator until this instant instead of the budget.
    static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// A ready pipeline worker waits this long on one translator request (or
    /// a busy bucket) before it hands the pipeline to the compile pool.
    static PATIENCE: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    /// Why this thread's last request returned no translation.
    static REASON: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
    /// The key of this thread's request in progress, and the (key, reason)
    /// of each request that returned no translation since `take_misses`.
    static KEY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static MISSES: std::cell::RefCell<Vec<(String, &'static str)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The (shader key, reason) of each request on this thread that returned no
/// translation since the last call (a background pipeline's build).
pub(crate) fn take_misses() -> Vec<(String, &'static str)> {
    MISSES.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

/// Worker threads a title thread is waiting on (by thread id): their pending
/// translation is marked urgent like a deadline wait's (f1d5b57 left a bind
/// waiting on a pipeline a worker was already building out).
static URGENT_WORKERS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// A title thread waits on the build running on worker `thread` (true), or
/// stopped waiting (false).
pub(crate) fn urge_worker(thread: u32, on: bool) {
    let mut w = URGENT_WORKERS.lock().unwrap_or_else(|e| e.into_inner());
    if on { w.push(thread) } else if let Some(at) = w.iter().position(|&t| t == thread) { w.swap_remove(at); }
}

fn urged() -> bool {
    let me = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    URGENT_WORKERS.lock().unwrap_or_else(|e| e.into_inner()).contains(&me)
}

/// Sets (or clears) this thread's translator deadline; returns the previous one.
pub(crate) fn wait_until(deadline: Option<Instant>) -> Option<Instant> {
    DEADLINE.with(|d| d.replace(deadline))
}

/// Sets (or clears) how long this worker waits on one translator request
/// before giving up as missed; returns the previous value.
pub(crate) fn patience(wait: Option<Duration>) -> Option<Duration> {
    PATIENCE.with(|p| p.replace(wait))
}

fn out_of_patience(since: Instant) -> bool {
    PATIENCE.with(|p| p.get()).is_some_and(|p| since.elapsed() >= p)
}

/// Why the last request on this thread returned no translation ("" if none failed).
pub(crate) fn take_reason() -> &'static str {
    REASON.with(|r| r.replace(""))
}

fn reason(why: &'static str) {
    REASON.with(|r| r.set(why));
    let key = KEY.with(|k| k.borrow().clone());
    MISSES.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() < 8 {
            m.push((key, why));
        }
    });
}

/// Whether this thread's translator waits draw on the frame budget: false on
/// workers and on a title thread with a deadline of its own.
fn budgeted() -> bool {
    !UNBOUNDED.with(|u| u.get()) && DEADLINE.with(|d| d.get()).is_none()
}

/// A title thread whose deadline has passed gives up as on a spent budget.
fn past_deadline() -> bool {
    DEADLINE.with(|d| d.get()).is_some_and(|d| Instant::now() >= d)
}

/// Whether a request on this thread missed the wait budget since the last call.
pub(crate) fn take_missed() -> bool {
    MISSED.with(|m| m.replace(false))
}

/// Draws `step` from the frame's pipeline wait budget (a bind or draw waiting
/// for a lazy pipeline's worker); false once it is spent.
pub(crate) fn spend_wait(step: Duration) -> bool {
    let ms = step.as_millis().max(1) as i64;
    let left = PIPELINE_WAIT_MS.fetch_sub(ms, Ordering::Relaxed) > 0;
    if !left {
        MISSED.with(|m| m.set(true));
    }
    left
}

/// This build's translation cache folder, once the worker has published it.
pub(crate) fn runtime_dir() -> Option<&'static Path> {
    compiler_cache(session()?).map(PathBuf::as_path)
}

/// This thread is a pipeline worker: its requests wait for the translator
/// (up to the request timeout) instead of drawing on the title's budget.
pub(crate) fn wait_unbounded() {
    UNBOUNDED.with(|u| u.set(true));
}

/// The process serving this session's translations (the launcher writes
/// `worker-pid` beside `compiler-id`).
pub(crate) fn worker_pid() -> Option<u32> {
    static PID: OnceLock<u32> = OnceLock::new();
    if let Some(pid) = PID.get() {
        return Some(*pid);
    }
    let pid = fs::read_to_string(session()?.join("worker-pid")).ok()?.trim().parse().ok()?;
    Some(*PID.get_or_init(|| pid))
}

fn session() -> Option<&'static PathBuf> {
    static SESSION: OnceLock<Option<PathBuf>> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let name = std::env::var("TUXBOX_SHADER_SESSION").ok()?;
            if !name.starts_with("run-")
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            {
                return None;
            }
            Some(std::env::temp_dir().join("tuxbox_shader_cache").join(name))
        })
        .as_ref()
}

fn read_digest(path: &Path) -> Option<[u8; 64]> {
    let mut file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() != 64 {
        return None;
    }
    let mut digest = [0; 64];
    file.read_exact(&mut digest).ok()?;
    Some(digest)
}

fn compiler_cache(session: &Path) -> Option<&'static PathBuf> {
    // The worker can publish the fingerprint after the first pipeline arrives.
    // Remember a successful binding for this process and retry an early miss.
    static CACHE: OnceLock<PathBuf> = OnceLock::new();
    if let Some(cache) = CACHE.get() {
        Some(cache)
    } else {
        let id = read_digest(&session.join("compiler-id"))?;
        if !id
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        {
            return None;
        }
        let directory = session
            .parent()?
            .join("runtime")
            .join(str::from_utf8(&id).ok()?);
        Some(CACHE.get_or_init(|| directory))
    }
}

fn cached_translation(cache: &Path, key: &str, stage: Stage) -> Option<Vec<u8>> {
    let file = cache.join(format!("{key}.cso"));
    let expected = read_digest(&file.with_extension("sha256"))?;
    let bytes = read_portable_file(&file, stage)?;
    // Host writers replace both files atomically, but can publish between
    // these two reads. Validate exactly what the pipeline will consume.
    (source_digest(&bytes)?.as_bytes() == expected).then_some(bytes)
}

/// One line per refused shader a title thread waits on, with the
/// translator's reason (the `.unsupported` marker's second line, else the
/// `.failed` reply): test173 hung at 387 s on two urgent compute shaders
/// xshader refused ("dynamic soffset unsupported"), and the log said only
/// "prepared compute shader missing". TUXBOX_REFUSED_WAIT_LOG=0 off.
fn log_waited_refusal(cache: Option<&Path>, failed: &Path, key: &str) {
    if crate::env_toggle!("TUXBOX_REFUSED_WAIT_LOG") == Some("0") {
        return;
    }
    static LOGGED: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);
    {
        let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        let set = logged.get_or_insert_with(Default::default);
        if set.len() >= 256 || !set.insert(key.to_owned()) {
            return;
        }
    }
    let marker = cache.and_then(|c| fs::read_to_string(c.join(format!("{key}.unsupported"))).ok());
    let reason = marker.as_deref().and_then(|m| m.lines().nth(1)).map(str::to_owned)
        .or_else(|| fs::read_to_string(failed).ok().map(|f| f.lines().next().unwrap_or("").to_owned()))
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| "no reason recorded".into());
    crate::diagnostics::log_line(format_args!("title waits on refused shader {key}: {}", reason.trim()));
}

fn cached_refusal(cache: &Path, key: &str) -> bool {
    let Ok(file) = fs::File::open(cache.join(format!("{key}.unsupported"))) else {
        return false;
    };
    // Source, linked pixel order, and stencil variants all use different full keys.
    let mut marker = Vec::with_capacity(key.len());
    if file.take(key.len() as u64 + 1).read_to_end(&mut marker).is_err() || marker != key.as_bytes() {
        return false;
    }
    static HITS: AtomicUsize = AtomicUsize::new(0);
    let hits = HITS.fetch_add(1, Ordering::Relaxed) + 1;
    if hits.is_power_of_two() {
        crate::diagnostics::log_once(&std::sync::Once::new(), || {
            format!("shader cache skipped {hits} requests with known unsupported translations")
        });
    }
    true
}

/// Whether the worker has published a translation for `key` already.
pub(super) fn published(key: &str) -> bool {
    session().is_some_and(|dir| dir.join(format!("{key}.cso")).is_file())
}

pub(super) fn on_demand(shader: &[u8], source: &str, stage: Stage) -> Option<Vec<u8>> {
    request(source, stage, |pending, requests| {
        let published = fs::write(pending, shader).and_then(|()| fs::rename(pending, requests.join(format!("{source}.dxbc"))));
        if published.is_err() {
            // A failed publication leaves no staging file (VIctory 7d73e2b).
            let _ = fs::remove_file(pending);
        }
        published
    })
}

pub(crate) fn quad_mask(shader: &[u8], source: &str) -> Option<Vec<u8>> {
    let key = format!("{source}_qm");
    request(&key, Stage::Pixel, |pending, requests| {
        let published = fs::write(pending, shader).and_then(|()| fs::rename(pending, requests.join(format!("{key}.quadmask"))));
        if published.is_err() {
            let _ = fs::remove_file(pending);
        }
        published
    })
}

pub(crate) fn stencil(shader: &[u8], source: &str, slot: u32) -> Option<Vec<u8>> {
    let key = format!("{source}_rt{slot}");
    request(&key, Stage::Pixel, |pending, requests| {
        let published = fs::write(pending, shader).and_then(|()| fs::rename(pending, requests.join(format!("{key}.stencil"))));
        if published.is_err() {
            // A failed publication leaves no staging file (VIctory 7d73e2b).
            let _ = fs::remove_file(pending);
        }
        published
    })
}

/// A link `linked` would make, if this session or the compiler cache has it
/// already; nothing is requested.
pub(super) fn linked_cached(producer: &str, pixel: &str, pixel_dxil: &[u8], stage: Stage) -> Option<Vec<u8>> {
    let order = super::source_digest(pixel_dxil)?;
    let key = format!("{producer}_{pixel}_{order}");
    let dir = session()?;
    read_portable_file(&dir.join(format!("{key}.cso")), stage)
        .or_else(|| cached_translation(compiler_cache(dir)?, &key, stage))
}

pub(super) fn linked(
    producer: (&str, &[u8]),
    pixel: (&str, &[u8]),
    pixel_dxil: &[u8],
    stage: Stage,
) -> Option<Vec<u8>> {
    let order = super::source_digest(pixel_dxil)?;
    let key = format!("{}_{}_{order}", producer.0, pixel.0);
    request(&key, stage, |pending, requests| {
        fs::create_dir(pending)?;
        let publish = || {
            fs::write(pending.join("producer.dxbc"), producer.1)?;
            fs::write(pending.join("pixel.dxbc"), pixel.1)?;
            fs::write(pending.join("pixel.cso"), pixel_dxil)?;
            fs::rename(pending, requests.join(format!("{key}.link")))
        };
        let result = publish();
        if result.is_err() {
            let _ = fs::remove_dir_all(pending);
        }
        result
    })
}

/// Who holds the right to publish one digest: a bucket lock, or the key itself.
enum Claim {
    #[allow(dead_code)]
    Bucket(std::sync::MutexGuard<'static, ()>),
    #[allow(dead_code)]
    Key(KeyClaim),
}

/// One digest in translation; dropped when its request returns.
struct KeyClaim(String);

static IN_FLIGHT: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl KeyClaim {
    fn try_new(key: &str) -> Option<Self> {
        let mut keys = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if keys.iter().any(|k| k == key) {
            return None;
        }
        keys.push(key.to_owned());
        Some(Self(key.to_owned()))
    }
}

impl Drop for KeyClaim {
    fn drop(&mut self) {
        let mut keys = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = keys.iter().position(|k| *k == self.0) {
            keys.swap_remove(i);
        }
    }
}

fn request(
    key: &str,
    stage: Stage,
    submit: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Option<Vec<u8>> {
    let dir = session()?;
    KEY.with(|k| key.clone_into(&mut k.borrow_mut()));
    let result = dir.join(format!("{key}.cso"));
    if let Some(bytes) = read_portable_file(&result, stage) {
        return Some(bytes);
    }
    // Pipeline workers can request the same program concurrently. Serialize
    // publication for the same digest while allowing unrelated jobs to overlap.
    static LOCKS: [Mutex<()>; 32] = [const { Mutex::new(()) }; 32];
    let bucket = usize::from_str_radix(&key[..2], 16).ok()? % LOCKS.len();
    // test100 hung with the title's inline first-bind compile blocked here
    // behind workers that hold the bucket while they wait up to 90 s on the
    // translator: the frame budget never covered this lock. A budgeted thread
    // now gives up on a busy bucket as on a spent budget, and the pipeline
    // goes to the workers. TUXBOX_SHADER_LOCK_SKIP=0 waits for the lock again.
    // Rather than giving up at once (a dropped draw, rebox c86c310's idea), it
    // first polls the bucket while the frame's wait budget lasts, so the wait
    // stays bounded by the same budget as the translator wait below.
    // A ready pipeline worker (PATIENCE) must not sit behind a compile-pool
    // thread holding the bucket for a long translation: it polls the bucket
    // up to its patience and then hands its pipeline on.
    let patient = PATIENCE.with(|p| p.get()).is_some();
    let polled = Instant::now();
    // test153: binds were handed on as untranslatable with the bucket busy,
    // its holder a compile-pool thread translating a different program that
    // only shared the digest's first byte. The lock exists to serialize one
    // digest, so it is now claimed per key; a busy claim means this very
    // program is being translated. TUXBOX_SHADER_KEY_LOCK=0 uses the 32
    // buckets again.
    let per_key = crate::env_toggle!("TUXBOX_SHADER_KEY_LOCK") != Some("0");
    let take = || -> Option<Claim> {
        if per_key {
            KeyClaim::try_new(key).map(Claim::Key)
        } else {
            match LOCKS[bucket].try_lock() {
                Ok(guard) => Some(Claim::Bucket(guard)),
                Err(std::sync::TryLockError::Poisoned(error)) => Some(Claim::Bucket(error.into_inner())),
                Err(std::sync::TryLockError::WouldBlock) => None,
            }
        }
    };
    let mut busy = take();
    while busy.is_none()
        && (!UNBOUNDED.with(|u| u.get()) || patient)
        && (if patient {
            !out_of_patience(polled)
        } else if budgeted() {
            WAIT_BUDGET_MS.fetch_sub(1, Ordering::Relaxed) > 0
        } else {
            !past_deadline()
        })
    {
        std::thread::sleep(Duration::from_millis(1));
        busy = take();
    }
    let _guard = match busy {
        Some(claim) => claim,
        None => {
            if patient {
                MISSED.with(|m| m.set(true));
                reason("translator bucket busy past the ready worker's wait (handed to the compile pool)");
                return None;
            }
            if !UNBOUNDED.with(|u| u.get()) && crate::env_toggle!("TUXBOX_SHADER_LOCK_SKIP") != Some("0") {
                MISSED.with(|m| m.set(true));
                reason("translator bucket busy past the wait budget");
                static SKIPPED: AtomicUsize = AtomicUsize::new(0);
                let skipped = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
                if skipped <= 4 || skipped.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "shader cache: a title-thread request met a bucket busy past the frame wait budget and went to the workers ({skipped} so far; TUXBOX_SHADER_LOCK_SKIP=0 off)"));
                }
                return None;
            }
            loop {
                if let Some(claim) = take() {
                    break claim;
                }
                if !per_key {
                    break Claim::Bucket(LOCKS[bucket].lock().unwrap_or_else(|error| error.into_inner()));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    if let Some(bytes) = read_portable_file(&result, stage) {
        return Some(bytes);
    }
    let failed = dir.join(format!("{key}.failed"));
    let pending = dir.join(format!("{key}.{}.tmp", std::process::id()));
    if let Some(cache) = compiler_cache(dir) {
        if let Some(bytes) = cached_translation(cache, key, stage) {
            // Later PSOs in this session can use the verified reply directly.
            if fs::write(&pending, &bytes)
                .and_then(|()| fs::rename(&pending, &result))
                .is_err()
            {
                let _ = fs::remove_file(&pending);
            }
            return Some(bytes);
        }
        if cached_refusal(cache, key) {
            if DEADLINE.with(|d| d.get()).is_some() || UNBOUNDED.with(|u| u.get()) {
                log_waited_refusal(Some(cache.as_ref()), &failed, key);
            }
            reason("translator refused it (cached .unsupported)");
            return None;
        }
    }
    // An earlier temporary tool failure completed its request. A new pipeline may retry it.
    let _ = fs::remove_file(&failed);
    if let Err(err) = submit(&pending, &dir.join("requests"))
        && err.kind() != io::ErrorKind::AlreadyExists
    {
        return None;
    }
    let start = Instant::now();
    // A title thread waiting with a deadline (a bind or a stolen build) puts
    // its request ahead of background ones (the launcher reads urgent/).
    let mut marked = false;
    let mut mark = || {
        if !marked {
            marked = true;
            let urgent = dir.join("urgent");
            let _ = fs::create_dir_all(&urgent);
            let _ = fs::write(urgent.join(key), b"");
            static MARKED: AtomicUsize = AtomicUsize::new(0);
            let n = MARKED.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 8 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "shader cache: request {} marked urgent, a title thread waits on it ({n} so far; the launcher logs each one it serves first)",
                    &key[..key.len().min(16)]));
            }
        }
    };
    // A ready pipeline worker (PATIENCE) builds a pipeline the title bound,
    // but its request was not marked: test178 launched on a new xshader (new
    // compiler id, empty key dir), the launcher's background prewarm of 2173
    // programs went first, and 52 bound programs gave up 24829 times "past the
    // ready worker's wait" while their .cso was still minutes away. It is
    // marked urgent too now, so the launcher serves it ahead of the prewarm.
    // TUXBOX_PATIENT_URGENT=0 leaves a worker's request unmarked.
    if DEADLINE.with(|d| d.get()).is_some()
        || (PATIENCE.with(|p| p.get()).is_some() && crate::env_toggle!("TUXBOX_PATIENT_URGENT") != Some("0"))
    {
        mark();
    }
    loop {
        if let Some(bytes) = read_portable_file(&result, stage) {
            return Some(bytes);
        }
        if UNBOUNDED.with(|u| u.get()) && urged() {
            mark();
        }
        if failed.exists() {
            if marked {
                log_waited_refusal(compiler_cache(dir).map(|c| c.as_path()), &failed, key);
            }
            reason("translator refused or failed it (.failed; see shader-worker.log)");
            return None;
        }
        if start.elapsed() >= Duration::from_secs(90) {
            reason("translator gave no reply in 90 s");
            return None;
        }
        if past_deadline() {
            MISSED.with(|m| m.set(true));
            reason("translator still busy at the title wait deadline");
            return None;
        }
        // A title thread waiting on this build keeps it here (hurry/urge target this thread).
        if out_of_patience(start) && !urged() {
            MISSED.with(|m| m.set(true));
            reason("translator still busy past the ready worker's wait (handed to the compile pool)");
            return None;
        }
        if budgeted() && WAIT_BUDGET_MS.fetch_sub(25, Ordering::Relaxed) <= 0 {
            MISSED.with(|m| m.set(true));
            reason("frame wait budget spent");
            static DROPPED: AtomicUsize = AtomicUsize::new(0);
            let dropped = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped.is_power_of_two() {
                crate::diagnostics::log_once(&std::sync::Once::new(), || {
                    format!("shader wait budget spent: {dropped} pipelines handed to the background builders")
                });
            }
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Where a heap index a program uses comes from (xshader TB_HEAP_REFLECT,
/// `<key>.heap`): a literal, a root constant dword plus `add`, a root CBV
/// word plus `add`, a table entry (param, byte offset), or something per lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HeapSource { Const(u32), Root(u32, u32, i64), Cbv(u32, u32, i64), Table(u32, u32), Dynamic }

/// The heap descriptors a program reads and writes (TUXBOX_SHADOW_UAV_REMARK).
#[derive(Debug, Default)]
pub(crate) struct HeapAccess { pub(crate) writes: Vec<HeapSource>, pub(crate) reads: Vec<HeapSource> }

/// The UAV-from-SRV slots text the launcher published for `source`
/// (its `uav_from_srv` lines in `<source>.heap`, see shader_cache::slots), from the session or the
/// compiler cache.
pub(crate) fn slots_text(source: &str) -> Option<String> {
    let dir = session()?;
    // Coder4 ea97bf2: the uav_from_srv lines ride in the .heap sidecar.
    let name = format!("{source}.heap");
    fs::read_to_string(dir.join(&name)).ok()
        .or_else(|| fs::read_to_string(compiler_cache(dir)?.join(&name)).ok())
}

/// The heap access reflection the launcher published for `source`, from the
/// session or the compiler cache. None: unknown (no file, older xshader).
pub(crate) fn heap_access(source: &str) -> Option<std::sync::Arc<HeapAccess>> {
    // Census (test177: the re-mark never saw a heap write although the
    // compiler cache held 1939 .heap files): lookups found / missed, the
    // first misses with where they looked.
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static FOUND: AtomicU64 = AtomicU64::new(0);
    static MISSED: AtomicU64 = AtomicU64::new(0);
    let name = format!("{source}.heap");
    let dir = session();
    let cache = dir.and_then(|d| compiler_cache(d));
    let text = dir.and_then(|d| fs::read_to_string(d.join(&name)).ok())
        .or_else(|| fs::read_to_string(cache?.join(&name)).ok());
    let (n, total) = if text.is_some() { (FOUND.fetch_add(1, Relaxed) + 1, MISSED.load(Relaxed)) } else { (MISSED.fetch_add(1, Relaxed) + 1, FOUND.load(Relaxed)) };
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "shadow uav re-mark: heap reflection {} for {source} (#{n}, {total} the other way): session {:?}, compiler cache {:?}",
            if text.is_some() { "found" } else { "missing" }, dir, cache));
    }
    let text = text?;
    let mut access = HeapAccess::default();
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let [op, kind, rest @ ..] = words.as_slice() else { continue };
        // SRV-as-UAV lines share the file (shader_cache::slots), not accesses.
        if *op == "uav_from_srv" { continue; }
        let n = |i: usize| rest.get(i).and_then(|w| w.parse::<i64>().ok());
        let source = match *kind {
            "const" => n(0).and_then(|v| u32::try_from(v).ok()).map_or(HeapSource::Dynamic, HeapSource::Const),
            "root" => HeapSource::Root(n(0).unwrap_or(0) as u32, n(1).unwrap_or(0) as u32, n(2).unwrap_or(0)),
            "cbv" => HeapSource::Cbv(n(0).unwrap_or(0) as u32, n(1).unwrap_or(0) as u32, n(2).unwrap_or(0)),
            "table" => HeapSource::Table(n(0).unwrap_or(0) as u32, n(1).unwrap_or(0) as u32),
            _ => HeapSource::Dynamic,
        };
        if op.contains('w') || *op == "atomic" { access.writes.push(source); }
        if op.contains('r') { access.reads.push(source); }
    }
    Some(std::sync::Arc::new(access))
}
