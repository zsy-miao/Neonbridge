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

/// A title frame was presented: grant the next frame its wait budget.
/// TUXBOX_FRAME_WAIT_MS overrides it: lower trades dropped draws (pipelines
/// still building) for frame time; 0 never waits after the first present.
pub(crate) fn frame_presented() {
    let budget = crate::env_toggle!("TUXBOX_FRAME_WAIT_MS").and_then(|v| v.parse().ok()).unwrap_or(FRAME_BUDGET_MS);
    WAIT_BUDGET_MS.store(budget, Ordering::Relaxed);
}

thread_local! {
    /// Set when a request on this thread gave up because the budget was spent.
    static MISSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Pipeline workers wait for the translator without spending the budget.
    static UNBOUNDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A title thread building a pipeline it waits on (the compute steal)
    /// waits for the translator until this instant instead of the budget.
    static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// Why this thread's last request returned no translation.
    static REASON: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
}

/// Sets (or clears) this thread's translator deadline; returns the previous one.
pub(crate) fn wait_until(deadline: Option<Instant>) -> Option<Instant> {
    DEADLINE.with(|d| d.replace(deadline))
}

/// Why the last request on this thread returned no translation ("" if none failed).
pub(crate) fn take_reason() -> &'static str {
    REASON.with(|r| r.replace(""))
}

fn reason(why: &'static str) {
    REASON.with(|r| r.set(why));
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

/// Draws `step` from the frame's wait budget (a bind waiting for a lazy
/// pipeline's worker); false once it is spent.
pub(crate) fn spend_wait(step: Duration) -> bool {
    let ms = step.as_millis().max(1) as i64;
    let left = WAIT_BUDGET_MS.fetch_sub(ms, Ordering::Relaxed) > 0;
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

fn request(
    key: &str,
    stage: Stage,
    submit: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Option<Vec<u8>> {
    let dir = session()?;
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
    let mut busy = LOCKS[bucket].try_lock();
    while matches!(busy, Err(std::sync::TryLockError::WouldBlock))
        && !UNBOUNDED.with(|u| u.get())
        && (if budgeted() { WAIT_BUDGET_MS.fetch_sub(1, Ordering::Relaxed) > 0 } else { !past_deadline() })
    {
        std::thread::sleep(Duration::from_millis(1));
        busy = LOCKS[bucket].try_lock();
    }
    let _guard = match busy {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
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
            LOCKS[bucket].lock().unwrap_or_else(|error| error.into_inner())
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
    loop {
        if let Some(bytes) = read_portable_file(&result, stage) {
            return Some(bytes);
        }
        if failed.exists() {
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
