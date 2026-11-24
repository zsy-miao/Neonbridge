//! Runtime shader worker: compiles the Xbox shader programs the running title
//! asks for, the Windows counterpart of TuxBox's `project_tuxbox/runtime_shaders.py`.
//!
//! d3d12_xs (env `TUXBOX_SHADER_SESSION=run-…`) publishes requests by renaming
//! them into `%TEMP%\tuxbox_shader_cache\<session>\requests`:
//!   `<sha256>.dxbc`, `<sha256>_rtN.stencil` (files) and
//!   `<producer>_<pixel>_<order>.link` (dir: producer.dxbc, pixel.dxbc, pixel.cso).
//! The worker answers with `<key>.cso` (DXIL) or `<key>.failed` in the session dir.
//! Translations: vendored `tools/shader-compiler` (xshader: Xbox ISA -> HLSL),
//! then the Windows SDK `dxc` (signed by its `dxil.dll`). Results persist in
//! `tuxbox_shader_cache\runtime\<toolchain fingerprint>`, which d3d12_xs also
//! reads directly via the session's `compiler-id`.
//! Under Sandboxie the session lives in the box's copy of %TEMP% (where the
//! boxed title writes); the persistent cache stays in the real %TEMP%, which
//! the box reads through.
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use std::{fs, thread};

const MAX_SHADER_BYTES: u64 = 64 << 20;
const XSHADER_UNSUPPORTED_HLSL: i32 = 65;

pub struct Tools {
    pub xshader: PathBuf,
    pub dxc: PathBuf,
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    pub stats: Arc<Stats>,
    private: PathBuf,
}

#[derive(Default)]
pub struct Stats {
    pub compiled: AtomicUsize,
    pub cached: AtomicUsize,
    pub unsupported: AtomicUsize,
    pub failed: AtomicUsize,
    /// Archived requests translated before the title started.
    pub prewarmed: AtomicUsize,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Windows SDK dxc.exe beside its dxil.dll (needed to sign DXIL for native D3D12).
pub fn find_dxc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DXC").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let kits = Path::new(&std::env::var("ProgramFiles(x86)").ok()?).join(r"Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = fs::read_dir(kits).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(r"x64\dxc.exe"))
        .find(|p| p.is_file() && p.with_file_name("dxil.dll").is_file())
}

pub fn has_dxil_container(data: &[u8]) -> bool {
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    if data.len() < 32 || &data[..4] != b"DXBC" {
        return false;
    }
    let (Some(size), Some(count)) = (u32_at(24), u32_at(28)) else { return false };
    let toc_end = 32 + count * 4;
    if count == 0 || !(toc_end <= size && size <= data.len()) {
        return false;
    }
    let mut found = false;
    for i in 0..count {
        let Some(offset) = u32_at(32 + i * 4) else { return false };
        if offset < toc_end || offset + 8 > size {
            return false;
        }
        let Some(length) = u32_at(offset + 4) else { return false };
        if offset + 8 + length > size {
            return false;
        }
        found |= &data[offset..offset + 4] == b"DXIL";
    }
    found
}

fn portable_pixel(data: &[u8]) -> bool {
    if !has_dxil_container(data) {
        return false;
    }
    let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize;
    let stages: Vec<u16> = (0..u32_at(28)).map(|i| u32_at(32 + i * 4))
        .filter(|&o| &data[o..o + 4] == b"DXIL")
        .map(|o| u16::from_le_bytes(data[o + 10..o + 12].try_into().unwrap())).collect();
    stages == [0] // D3D12_SHVER_PIXEL_SHADER
}

fn is_key(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn valid_request(name: &str) -> Option<(&str, &str)> {
    let (key, ext) = name.rsplit_once('.')?;
    let ok = match ext {
        "dxbc" => is_key(key),
        "stencil" => key.split_once("_rt").is_some_and(|(k, n)| is_key(k) && n.len() == 1 && (b'0'..=b'7').contains(&n.as_bytes()[0])),
        "link" => { let p: Vec<_> = key.split('_').collect(); p.len() == 3 && p.iter().all(|k| is_key(k)) }
        _ => false,
    };
    ok.then_some((key, ext))
}

fn verified(path: &Path, expected: &str) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_SHADER_BYTES {
        return Err(format!("{} is not a shader file within 64 MiB", path.display()));
    }
    let data = fs::read(path).map_err(|e| e.to_string())?;
    if sha256(&data) != expected {
        return Err(format!("{} contents differ from the requested digest", path.display()));
    }
    Ok(data)
}

enum ToolError { Exit(i32, String), Other(String) }

fn run(cmd: &mut Command, timeout: Duration) -> Result<(), ToolError> {
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
        .map_err(|e| ToolError::Other(format!("{e}")))?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut s, &mut err);
                }
                let tail: String = err.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
                return Err(ToolError::Exit(status.code().unwrap_or(-1), tail.trim().to_owned()));
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                return Err(ToolError::Other("timed out".into()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(ToolError::Other(e.to_string())),
        }
    }
}

/// Pin the compiler (a private copy of xshader) and return the persistent cache dir.
fn bind_cache(tools: &Tools, cache_root: &Path, private: &Path) -> Result<(Tools, PathBuf), String> {
    fs::create_dir_all(private).map_err(|e| e.to_string())?;
    let xshader = private.join("xshader.exe");
    fs::copy(&tools.xshader, &xshader).map_err(|e| format!("copy xshader: {e}"))?;
    let version = Command::new(&tools.dxc).arg("--version").output().map_err(|e| format!("dxc: {e}"))?.stdout;
    let mut fp = Sha256::new();
    fp.update(&version);
    for t in [&xshader, &tools.dxc] {
        fp.update(sha256(&fs::read(t).map_err(|e| e.to_string())?).as_bytes());
    }
    let cache = cache_root.join("runtime").join(hex(&fp.finalize()));
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    Ok((Tools { xshader, dxc: tools.dxc.clone() }, cache))
}

enum Outcome { Compiled, Cached, Unsupported(String), Failed(String) }

fn compile(source: &Path, key: &str, ext: &str, tools: &Tools, cache: &Path, session: &Path) -> Outcome {
    let work = cache.join(format!("tmp-{}-{}", std::process::id(), sha256(key.as_bytes()).get(..12).unwrap_or("x")));
    let _ = fs::remove_dir_all(&work);
    if let Err(e) = fs::create_dir_all(&work) {
        return Outcome::Failed(e.to_string());
    }
    let result = (|| -> Result<Outcome, String> {
        let mut flags: Vec<std::ffi::OsString> = Vec::new();
        let cooked;
        if ext == "link" {
            if !fs::symlink_metadata(source).map(|m| m.is_dir()).unwrap_or(false) {
                return Err("linked shader request must be a directory".into());
            }
            let mut k = key.split('_');
            let (ph, xh, dh) = (k.next().unwrap(), k.next().unwrap(), k.next().unwrap());
            cooked = work.join("producer.dxbc");
            fs::write(&cooked, verified(&source.join("producer.dxbc"), ph)?).map_err(|e| format!("write {}: {e}", cooked.display()))?;
            let pixel = work.join("pixel.dxbc");
            fs::write(&pixel, verified(&source.join("pixel.dxbc"), xh)?).map_err(|e| e.to_string())?;
            let code = verified(&source.join("pixel.cso"), dh)?;
            if !portable_pixel(&code) {
                return Err("pixel order requires a portable pixel DXIL container".into());
            }
            let order = work.join("pixel.cso");
            fs::write(&order, code).map_err(|e| e.to_string())?;
            flags.extend(["--link-pixel".into(), pixel.into_os_string(), "--pixel-order".into(), order.into_os_string()]);
        } else {
            cooked = work.join("input.dxbc");
            let source_hash = if ext == "stencil" {
                let (h, slot) = key.split_once("_rt").unwrap();
                flags.extend(["--stencil-target".into(), slot.into()]);
                h
            } else { key };
            fs::write(&cooked, verified(source, source_hash)?).map_err(|e| format!("write {}: {e}", cooked.display()))?;
        }
        let dest = cache.join(format!("{key}.cso"));
        let checksum = cache.join(format!("{key}.sha256"));
        let refusal = cache.join(format!("{key}.unsupported"));
        let publish = |bytes: &[u8]| -> Result<(), String> {
            let tmp = session.join(format!("{key}.{}.reply", std::process::id()));
            fs::write(&tmp, bytes).and_then(|_| fs::rename(&tmp, session.join(format!("{key}.cso")))).map_err(|e| format!("publish to {}: {e}", session.display()))
        };
        if let (Ok(bytes), Ok(sum)) = (fs::read(&dest), fs::read(&checksum))
            && bytes.len() as u64 <= MAX_SHADER_BYTES && sum == sha256(&bytes).as_bytes() {
            publish(&bytes)?;
            return Ok(Outcome::Cached);
        }
        if fs::read(&refusal).is_ok_and(|m| m == key.as_bytes()) {
            return Ok(Outcome::Unsupported("cached unsupported HLSL translation".into()));
        }
        let (hlsl, output, profile) = (work.join("shader.hlsl"), work.join("out.cso"), work.join("profile"));
        match run(Command::new(&tools.xshader).arg(&cooked).args(&flags).arg("--hlsl-out").arg(&hlsl)
            .arg("--profile-out").arg(&profile), Duration::from_secs(30)) {
            Ok(()) => {}
            Err(ToolError::Exit(XSHADER_UNSUPPORTED_HLSL, msg)) => {
                let _ = fs::write(&refusal, key);
                return Ok(Outcome::Unsupported(msg));
            }
            Err(ToolError::Exit(c, msg)) => return Err(format!("xshader failed ({c}): {msg}")),
            Err(ToolError::Other(msg)) => return Err(format!("xshader: {msg}")),
        }
        let target = fs::read_to_string(&profile).map_err(|e| format!("xshader wrote no profile: {e}"))?.trim().to_owned();
        let allowed: &[&str] = if ext == "link" {
            &["vs_6_0", "vs_6_1", "vs_6_6", "gs_6_0", "gs_6_1", "gs_6_6"]
        } else {
            &["cs_6_0", "cs_6_1", "cs_6_6", "ps_6_0", "ps_6_1", "ps_6_6", "vs_6_0", "vs_6_1", "vs_6_6",
              "gs_6_0", "gs_6_1", "gs_6_6", "cs_6_4", "cs_6_5", "ms_6_5", "ms_6_6", "as_6_5", "as_6_6", "lib_6_3"]
        };
        if !allowed.contains(&target.as_str()) {
            return Err(format!("unsupported compiler profile {target:?}"));
        }
        let mut dxc = Command::new(&tools.dxc);
        dxc.arg("-T").arg(&target);
        if target != "lib_6_3" {
            dxc.args(["-E", "main"]);
        }
        match run(dxc.arg("-Fo").arg(&output).arg(&hlsl), Duration::from_secs(50)) {
            Ok(()) => {}
            Err(ToolError::Exit(c, msg)) => return Err(format!("dxc failed ({c}): {msg}")),
            Err(ToolError::Other(msg)) => return Err(format!("dxc: {msg}")),
        }
        let bytes = fs::read(&output).map_err(|e| format!("dxc wrote no output: {e}"))?;
        if !has_dxil_container(&bytes) {
            return Err("compiler returned an invalid DXIL container".into());
        }
        let stamp = work.join("sha256");
        fs::write(&stamp, sha256(&bytes)).map_err(|e| e.to_string())?;
        fs::rename(&output, &dest).map_err(|e| format!("store {}: {e}", dest.display()))?;
        fs::rename(&stamp, &checksum).map_err(|e| e.to_string())?;
        publish(&bytes)?;
        Ok(Outcome::Compiled)
    })();
    let _ = fs::remove_dir_all(&work);
    result.unwrap_or_else(Outcome::Failed)
}

fn remove_request(path: &Path) {
    if fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

/// Keep a copy of a request the title made, so the next launch can translate it
/// before the title asks (TuxBox's ProjectScarlett prewarm).
fn archive_request(path: &Path, name: &str, archive: &Path) {
    let dest = archive.join(name);
    if fs::symlink_metadata(&dest).is_ok() {
        return;
    }
    let tmp = archive.join(format!("{name}.{}.tmp", std::process::id()));
    let copied = if fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
        fs::create_dir_all(&tmp).and_then(|_| {
            for f in ["producer.dxbc", "pixel.dxbc", "pixel.cso"] {
                fs::copy(path.join(f), tmp.join(f))?;
            }
            Ok(())
        })
    } else {
        fs::copy(path, &tmp).map(|_| ())
    };
    if copied.and_then(|_| fs::rename(&tmp, &dest)).is_err() {
        remove_request(&tmp);
    }
}

/// Translate every archived request into the cache before the title starts,
/// so its pipelines load translations instead of waiting on the worker.
fn prewarm(tools: &Tools, cache: &Path, archive: &Path, scratch: &Path, workers: usize, stats: &Stats) {
    let Ok(entries) = fs::read_dir(archive) else { return };
    let jobs: Vec<(PathBuf, String, String)> = entries.flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        let (key, ext) = valid_request(&name)?;
        Some((e.path(), key.to_owned(), ext.to_owned()))
    }).collect();
    if jobs.is_empty() || fs::create_dir_all(scratch).is_err() {
        return;
    }
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some((path, key, ext)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let counter = match compile(path, key, ext, tools, cache, scratch) {
                        Outcome::Compiled => &stats.prewarmed,
                        Outcome::Cached => &stats.cached,
                        Outcome::Unsupported(_) => &stats.unsupported,
                        Outcome::Failed(_) => &stats.failed,
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                    let _ = fs::remove_file(scratch.join(format!("{key}.cso")));
                }
            });
        }
    });
    let _ = fs::remove_dir_all(scratch);
}

impl Worker {
    /// Create the session (`session_dir`, where the title publishes requests) and serve it.
    pub fn start(tools: Tools, cache_root: PathBuf, session_dir: PathBuf, log: impl Fn(String) + Send + Sync + 'static)
        -> Result<Self, String> {
        let requests = session_dir.join("requests");
        fs::create_dir_all(&requests).map_err(|e| format!("{}: {e}", requests.display()))?;
        // Outside the session: Sandboxie forces programs started from a box folder into the box.
        let private = cache_root.join(format!("xshader-{}", std::process::id()));
        let (tools, cache) = bind_cache(&tools, &cache_root, &private)?;
        let id = cache.file_name().unwrap().to_string_lossy().into_owned();
        fs::write(session_dir.join("compiler-id.tmp"), &id)
            .and_then(|_| fs::rename(session_dir.join("compiler-id.tmp"), session_dir.join("compiler-id")))
            .map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let workers = thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8));
        let archive = cache_root.join("requests");
        let _ = fs::create_dir_all(&archive);
        prewarm(&tools, &cache, &archive, &private.join("prewarm"), workers, &stats);
        let (s, st) = (stop.clone(), stats.clone());
        let log = Arc::new(log);
        let handle = thread::Builder::new().name("shader-worker".into()).spawn(move || {
            let tools = Arc::new(tools);
            let busy: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Default::default();
            let active = Arc::new(AtomicUsize::new(0));
            while !s.load(Ordering::Acquire) {
                let mut idle = true;
                if let Ok(entries) = fs::read_dir(&requests) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let path = entry.path();
                        let Some((key, ext)) = valid_request(&name).map(|(k, e)| (k.to_owned(), e.to_owned())) else {
                            if !name.ends_with(".tmp") { remove_request(&path); }
                            continue;
                        };
                        if !busy.lock().unwrap().insert(name.clone()) {
                            continue;
                        }
                        idle = false;
                        while active.load(Ordering::Acquire) >= workers {
                            thread::sleep(Duration::from_millis(5));
                        }
                        active.fetch_add(1, Ordering::AcqRel);
                        archive_request(&path, &name, &archive);
                        let (tools, cache, session, st, log, busy, active) =
                            (tools.clone(), cache.clone(), session_dir.clone(), st.clone(), log.clone(), busy.clone(), active.clone());
                        thread::spawn(move || {
                            let outcome = compile(&path, &key, &ext, &tools, &cache, &session);
                            match &outcome {
                                Outcome::Compiled => { st.compiled.fetch_add(1, Ordering::Relaxed); log(format!("compiled Xbox shader {key}")); }
                                Outcome::Cached => { st.cached.fetch_add(1, Ordering::Relaxed); }
                                Outcome::Unsupported(m) => { st.unsupported.fetch_add(1, Ordering::Relaxed); log(format!("Xbox shader {key}: unsupported: {m}")); }
                                Outcome::Failed(m) => { st.failed.fetch_add(1, Ordering::Relaxed); log(format!("Xbox shader {key}: {m}")); }
                            }
                            if matches!(outcome, Outcome::Unsupported(_) | Outcome::Failed(_)) {
                                let _ = fs::write(session.join(format!("{key}.failed")), b"");
                                // Keep refused programs as a corpus for translator work offline.
                                if ext == "dxbc" && let Some(root) = cache.parent() {
                                    let refused = root.join("refused");
                                    let _ = fs::create_dir_all(&refused);
                                    let _ = fs::copy(&path, refused.join(format!("{key}.dxbc")));
                                }
                            }
                            remove_request(&path);
                            busy.lock().unwrap().remove(&name);
                            active.fetch_sub(1, Ordering::AcqRel);
                        });
                    }
                }
                if idle {
                    thread::sleep(Duration::from_millis(25));
                }
            }
            while active.load(Ordering::Acquire) > 0 {
                thread::sleep(Duration::from_millis(10));
            }
        }).map_err(|e| e.to_string())?;
        Ok(Self { stop, handle: Some(handle), stats, private })
    }

    pub fn stop(mut self) -> Arc<Stats> {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = fs::remove_dir_all(&self.private);
        self.stats.clone()
    }
}
