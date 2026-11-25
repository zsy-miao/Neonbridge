//! Submit unprofiled console shaders to the running launcher's compiler.
use super::{Stage, read_portable_file, source_digest};
use crate::progress::{self, Shader};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};

mod request_locks;
use request_locks::RequestLocks;
use std::time::{Duration, Instant};

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

/// The names in a directory, listed once and then extended by this process.
///
/// Wine matches names without regard to case. Opening an absent one therefore
/// reads the whole directory to look for another spelling of it, and the cache
/// holds tens of thousands of files. A name the host added after the listing
/// only costs a request, which the host answers from its cache.
struct Names(OnceLock<Mutex<HashSet<String>>>);

impl Names {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    fn of(&self, directory: &Path) -> MutexGuard<'_, HashSet<String>> {
        self.0
            .get_or_init(|| {
                let entries = fs::read_dir(directory).into_iter().flatten().flatten();
                let names = entries.filter_map(|entry| entry.file_name().into_string().ok());
                Mutex::new(names.collect())
            })
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// Files of the compiler's cache.
static CACHED: Names = Names::new();
/// Replies published in this session.
static REPLIES: Names = Names::new();

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
    if !CACHED.of(cache).contains(&format!("{key}.sha256")) {
        return None;
    }
    let file = cache.join(format!("{key}.cso"));
    let expected = read_digest(&file.with_extension("sha256"))?;
    let bytes = read_portable_file(&file, stage)?;
    // Host writers replace both files atomically, but can publish between
    // these two reads. Validate exactly what the pipeline will consume.
    (source_digest(&bytes)?.as_bytes() == expected).then_some(bytes)
}

fn cached_refusal(cache: &Path, key: &str) -> bool {
    if !CACHED.of(cache).contains(&format!("{key}.unsupported")) {
        return false;
    }
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
    request(source, stage, "dxbc", |pending, requests| {
        fs::write(pending, shader)?;
        fs::rename(pending, requests.join(format!("{source}.dxbc")))
    })
}

pub(crate) fn stencil(shader: &[u8], source: &str, slot: u32) -> Option<Vec<u8>> {
    let key = format!("{source}_rt{slot}");
    request(&key, Stage::Pixel, "stencil", |pending, requests| {
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
    request(&key, stage, "link", |pending, requests| {
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
    extension: &str,
    submit: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Option<Vec<u8>> {
    let dir = session()?;
    let name = format!("{key}.cso");
    let result = dir.join(&name);
    let reply = || {
        let known = REPLIES.of(dir).contains(&name);
        if known { read_portable_file(&result, stage) } else { None }
    };
    if let Some(bytes) = reply() {
        progress::record(key, Shader::Cached);
        return Some(bytes);
    }
    // Serialize the exact digest, not a hash bucket. Unrelated programs
    // must not wait behind a slow translation (or its 90-second timeout).
    static LOCKS: LazyLock<RequestLocks> = LazyLock::new(RequestLocks::default);
    let lock = LOCKS.for_key(key);
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(bytes) = reply() {
        progress::record(key, Shader::Cached);
        return Some(bytes);
    }
    let failed = dir.join(format!("{key}.failed"));
    let pending = dir.join(format!("{key}.{}.tmp", std::process::id()));
    if let Some(cache) = compiler_cache(dir) {
        if let Some(bytes) = cached_translation(cache, key, stage) {
            progress::record(key, Shader::Cached);
            return Some(bytes);
        }
        if cached_refusal(cache, key) {
            progress::record(key, Shader::Unsupported);
            return None;
        }
    }
    let requests = dir.join("requests");
    let published = requests.join(format!("{key}.{extension}"));
    // A timed-out caller does not own the source anymore. Rejoin the
    // outstanding request rather than replace it while the host is reading
    // or about to remove it. The host publishes failure before removing the
    // source; only a new publication may clear that old failure.
    if !published.exists() {
        // A successful worker may have completed between the first read
        // and the existence check. Avoid compiling that request twice.
        if let Some(bytes) = read_portable_file(&result, stage) {
            REPLIES.of(dir).insert(name);
            progress::record(key, Shader::Translated);
            return Some(bytes);
        }
        let _ = fs::remove_file(&failed);
        if let Err(err) = submit(&pending, &requests)
            && err.kind() != io::ErrorKind::AlreadyExists
        {
            return None;
        }
    }
    // The title is now waiting on the host worker; the progress panel counts it.
    let _waiting = progress::waiting();
    let start = Instant::now();
    loop {
        if let Some(bytes) = read_portable_file(&result, stage) {
            REPLIES.of(dir).insert(name);
            progress::record(key, Shader::Translated);
            return Some(bytes);
        }
        if failed.exists() {
            // The host records a refusal in its cache before it reports the failure.
            if let Some(cache) = compiler_cache(dir) {
                CACHED.of(cache).insert(format!("{key}.unsupported"));
            }
            progress::record(key, Shader::Unsupported);
            return None;
        }
        if start.elapsed() >= Duration::from_secs(90) {
            progress::record(key, Shader::TimedOut);
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
#[path = "../../tests/device/shader_cache/compile.rs"]
mod tests;
