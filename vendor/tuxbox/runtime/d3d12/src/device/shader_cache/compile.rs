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
const FRAME_BUDGET_MS: i64 = 500;

/// A title frame was presented: grant the next frame its wait budget.
pub(crate) fn frame_presented() {
    WAIT_BUDGET_MS.store(FRAME_BUDGET_MS, Ordering::Relaxed);
}

thread_local! {
    /// Set when a request on this thread gave up because the budget was spent.
    static MISSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Pipeline workers wait for the translator without spending the budget.
    static UNBOUNDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
        fs::write(pending, shader)?;
        fs::rename(pending, requests.join(format!("{source}.dxbc")))
    })
}

pub(crate) fn stencil(shader: &[u8], source: &str, slot: u32) -> Option<Vec<u8>> {
    let key = format!("{source}_rt{slot}");
    request(&key, Stage::Pixel, |pending, requests| {
        fs::write(pending, shader)?;
        fs::rename(pending, requests.join(format!("{key}.stencil")))
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
    let _guard = LOCKS[bucket]
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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
        if failed.exists() || start.elapsed() >= Duration::from_secs(90) {
            return None;
        }
        if !UNBOUNDED.with(|u| u.get()) && WAIT_BUDGET_MS.fetch_sub(25, Ordering::Relaxed) <= 0 {
            MISSED.with(|m| m.set(true));
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
