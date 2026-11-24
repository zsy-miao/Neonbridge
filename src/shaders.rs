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
    /// dxc `-D` defines; bind_cache fills them from the host wave range.
    pub defines: Vec<String>,
}

/// `TB_NO_WAVESIZE_<n>` for each wave width outside the host's lane range, which
/// d3d12_xs leaves as "min max" in `<cache_root>/wave_lanes.txt` at device
/// creation. xshader's HLSL wraps `[WaveSize(n)]` in `#ifndef` on them, so a
/// wave32-only GPU doesn't reject the pipeline. No file yet (first run on this
/// machine) means no defines.
fn wave_size_defines(cache_root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(cache_root.join("wave_lanes.txt")) else { return Vec::new() };
    let mut it = text.split_whitespace().map(str::parse::<u32>);
    let (Some(Ok(min)), Some(Ok(max))) = (it.next(), it.next()) else { return Vec::new() };
    [32, 64].into_iter().filter(|&n| n < min || n > max).map(|n| format!("TB_NO_WAVESIZE_{n}")).collect()
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
    /// Prewarm refusals and failures by reason (registers, program counters
    /// and numbers masked, as TuxBox's triage_scarlett_shaders.py does).
    pub reasons: std::sync::Mutex<std::collections::BTreeMap<String, usize>>,
    /// The same, grouped by the construct named before " at N" (or the first ':').
    pub constructs: std::sync::Mutex<std::collections::BTreeMap<String, usize>>,
}

/// One line per refusal: "[op @ 0x1a0]" variant tags dropped, registers to
/// `sN`/`vN`/`tN`, other numbers to `N`, so the same gap in different shaders
/// shares one row.
fn mask_reason(msg: &str) -> String {
    let line = msg.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("(no message)");
    // Drop " [mnemonic @ 0xADDR]".
    let mut untagged = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('[') {
        let tail = &rest[at + 1..];
        let tag = tail.find(']').map(|end| &tail[..end]).filter(|t| {
            t.split_once(" @ 0x").is_some_and(|(op, addr)| !op.is_empty()
                && op.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                && !addr.is_empty() && addr.bytes().all(|b| b.is_ascii_hexdigit()))
        });
        match tag {
            Some(t) => { untagged.push_str(rest[..at].trim_end()); rest = &tail[t.len() + 1..]; }
            None => { untagged.push_str(&rest[..=at]); rest = tail; }
        }
    }
    untagged.push_str(rest);
    let b = untagged.as_bytes();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        let next = b.get(i + 1).copied();
        if boundary && matches!(c, b's' | b'v' | b't')
            && (next.is_some_and(|n| n.is_ascii_digit())
                || (next == Some(b'[') && b.get(i + 2).is_some_and(u8::is_ascii_digit)))
        {
            let mut j = i + 1;
            if b[j] == b'[' {
                while j < b.len() && b[j] != b']' { j += 1; }
                j = (j + 1).min(b.len());
            } else {
                while j < b.len() && (b[j].is_ascii_digit() || b[j] == b':') { j += 1; }
            }
            if !b.get(j).is_some_and(|n| n.is_ascii_alphanumeric() || *n == b'_') {
                out.push(c as char);
                out.push('N');
                i = j;
                continue;
            }
        }
        if c.is_ascii_digit() {
            i += 1;
            while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'x') { i += 1; }
            out.push('N');
            continue;
        }
        let ch = untagged[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    if out.is_empty() { "(no message)".to_owned() } else { out }
}

/// Coarse group: the construct named before " at N", else before the first ':'.
fn construct(reason: &str) -> &str {
    reason.split_once(" at N").map_or_else(|| reason.split(':').next().unwrap_or(reason), |(head, _)| head)
}

fn ranked(map: &std::sync::Mutex<std::collections::BTreeMap<String, usize>>, n: usize) -> Vec<(String, usize)> {
    let mut v: Vec<_> = map.lock().unwrap().iter().map(|(r, c)| (r.clone(), *c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

impl Stats {
    fn note(&self, kind: &str, msg: &str) {
        let masked = mask_reason(msg);
        let group: String = format!("{kind}: {}", construct(&masked)).chars().take(120).collect();
        let reason: String = format!("{kind}: {masked}").chars().take(160).collect();
        *self.reasons.lock().unwrap().entry(reason).or_default() += 1;
        *self.constructs.lock().unwrap().entry(group).or_default() += 1;
    }

    /// The most common reasons, most frequent first.
    pub fn top_reasons(&self, n: usize) -> Vec<(String, usize)> {
        ranked(&self.reasons, n)
    }

    /// The most common constructs, most frequent first.
    pub fn top_constructs(&self, n: usize) -> Vec<(String, usize)> {
        ranked(&self.constructs, n)
    }
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
    let defines = wave_size_defines(cache_root);
    let mut fp = Sha256::new();
    fp.update(&version);
    // The defines change the DXIL, so they're part of the cache key.
    for d in &defines {
        fp.update(b"-D");
        fp.update(d.as_bytes());
    }
    for t in [&xshader, &tools.dxc] {
        fp.update(sha256(&fs::read(t).map_err(|e| e.to_string())?).as_bytes());
    }
    let cache = cache_root.join("runtime").join(hex(&fp.finalize()));
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    Ok((Tools { xshader, dxc: tools.dxc.clone(), defines }, cache))
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
        // `<key>` on the first line, then xshader's reason (older entries hold the key alone).
        if let Ok(text) = fs::read_to_string(&refusal) {
            let (first, reason) = text.split_once('\n').unwrap_or((&text, ""));
            if first == key {
                let reason = if reason.trim().is_empty() { "cached unsupported HLSL translation" } else { reason.trim() };
                return Ok(Outcome::Unsupported(reason.to_owned()));
            }
        }
        let (hlsl, output, profile) = (work.join("shader.hlsl"), work.join("out.cso"), work.join("profile"));
        match run(Command::new(&tools.xshader).arg(&cooked).args(&flags).arg("--hlsl-out").arg(&hlsl)
            .arg("--profile-out").arg(&profile), Duration::from_secs(120)) {
            Ok(()) => {}
            Err(ToolError::Exit(XSHADER_UNSUPPORTED_HLSL, msg)) => {
                let _ = fs::write(&refusal, format!("{key}\n{msg}"));
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
        let dxc = |opt: &str, secs: u64| {
            let mut dxc = Command::new(&tools.dxc);
            dxc.arg("-T").arg(&target).arg(opt);
            for d in &tools.defines {
                dxc.arg("-D").arg(d);
            }
            if target != "lib_6_3" {
                dxc.args(["-E", "main"]);
            }
            run(dxc.arg("-Fo").arg(&output).arg(&hlsl), Duration::from_secs(secs))
        };
        // Large skinning and edge-emit programs can spend minutes in DXC's optimiser;
        // unoptimised DXIL compiles in seconds and the driver optimises it again.
        let compiled = match dxc("-O1", 120) {
            Err(ToolError::Other(msg)) if msg == "timed out" => {
                let _ = fs::remove_file(&output);
                dxc("-O0", 120)
            }
            other => other,
        };
        match compiled {
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

/// DXBC parts as (fourcc, payload offset, payload length), or None if the
/// container is malformed.
fn dxbc_parts(data: &[u8]) -> Option<Vec<([u8; 4], usize, usize)>> {
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    if data.len() < 32 || &data[..4] != b"DXBC" || u32_at(24)? != data.len() {
        return None;
    }
    let count = u32_at(28)?;
    (0..count).map(|i| {
        let o = u32_at(32 + i * 4)?;
        let len = u32_at(o + 4)?;
        (o + 8 + len <= data.len()).then(|| (data[o..o + 4].try_into().unwrap(), o + 8, len))
    }).collect()
}

/// D3D12 program stage: high 16 bits of the DXIL part's program version.
fn dxil_stage(data: &[u8], parts: &[([u8; 4], usize, usize)]) -> Option<u16> {
    let &(_, o, len) = parts.iter().find(|p| &p.0 == b"DXIL")?;
    (len >= 4).then(|| u16::from_le_bytes(data[o + 2..o + 4].try_into().unwrap()))
}

/// The program as d3d12_xs sends it when the pipeline supplies its root
/// signature (`with_root` in runtime/d3d12/src/device/shader_cache.rs):
/// hash cleared, one more part offset, RTS0 appended at a 4-byte boundary.
fn with_root(shader: &[u8], count: usize, root: &[u8]) -> Vec<u8> {
    let end = 32 + count * 4;
    let root_offset = (shader.len() + 4).next_multiple_of(4);
    let size = root_offset + 8 + root.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&shader[..24]);
    out[4..20].fill(0);
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&((count + 1) as u32).to_le_bytes());
    for i in 0..count {
        let o = u32::from_le_bytes(shader[32 + i * 4..36 + i * 4].try_into().unwrap());
        out.extend_from_slice(&(o + 4).to_le_bytes());
    }
    out.extend_from_slice(&(root_offset as u32).to_le_bytes());
    out.extend_from_slice(&shader[end..]);
    out.resize(root_offset, 0);
    out.extend_from_slice(b"RTS0");
    out.extend_from_slice(&(root.len() as u32).to_le_bytes());
    out.extend_from_slice(root);
    out
}

/// Graphics stages whose pipelines share a few root signatures (pixel, vertex,
/// geometry, hull, domain, mesh, amplification). Compute roots are per program.
const SHARED_ROOT_STAGES: [u16; 7] = [0, 1, 2, 3, 4, 13, 14];
const MAX_SHARED_ROOTS: usize = 4;

fn write_request(archive: &Path, data: &[u8]) -> bool {
    let name = format!("{}.dxbc", sha256(data));
    let dest = archive.join(&name);
    if fs::symlink_metadata(&dest).is_ok() {
        return false;
    }
    let tmp = archive.join(format!("{name}.{}.tmp", std::process::id()));
    if fs::write(&tmp, data).and_then(|_| fs::rename(&tmp, &dest)).is_err() {
        let _ = fs::remove_file(&tmp);
        return false;
    }
    true
}

pub struct Built { pub programs: usize, pub written: usize }

/// Build every shader program in the title's `shaders.rpf` into the prewarm
/// archive, as PC games do before the first frame.
///
/// The data area of shaders.rpf is not encrypted: after the `SGD5` index each
/// program is a record `<name>_Wrapped\0`, a tag byte, u32 stored size, u32,
/// u32 stored size again, u32 expanded size, then a raw deflate stream that
/// inflates to the exact DXBC container the title hands d3d12_xs (its SHA-256
/// is the request key). Records are found by scanning, so the encrypted TOC is
/// not needed. Programs whose pipelines bring a root signature are requested
/// with it appended; for graphics stages that use few roots, each program is
/// also written with every root of its stage seen in archived requests.
/// Skipped when the archive already holds this build of this shaders.rpf.
pub fn build_requests(rpf: &Path, archive: &Path) -> Result<Option<Built>, String> {
    let meta = fs::metadata(rpf).map_err(|e| format!("{}: {e}", rpf.display()))?;
    fs::create_dir_all(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut roots: std::collections::BTreeMap<u16, std::collections::BTreeMap<String, Vec<u8>>> = Default::default();
    for entry in fs::read_dir(archive).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !matches!(valid_request(&name), Some((_, "dxbc"))) {
            continue;
        }
        let Ok(data) = fs::read(entry.path()) else { continue };
        let Some(parts) = dxbc_parts(&data) else { continue };
        let (Some(stage), Some(&(_, o, len))) = (dxil_stage(&data, &parts), parts.iter().find(|p| &p.0 == b"RTS0")) else { continue };
        if SHARED_ROOT_STAGES.contains(&stage) {
            let root = data[o..o + len].to_vec();
            roots.entry(stage).or_default().insert(sha256(&root), root);
        }
    }
    roots.retain(|_, r| r.len() <= MAX_SHARED_ROOTS);
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let mut stamp = format!("shaders.rpf build v1\n{} {} {mtime}\n", rpf.display(), meta.len());
    for (stage, r) in &roots {
        for key in r.keys() {
            stamp.push_str(&format!("root {stage} {key}\n"));
        }
    }
    let marker = archive.join(".built");
    if fs::read_to_string(&marker).is_ok_and(|s| s == stamp) {
        return Ok(None);
    }

    let data = fs::read(rpf).map_err(|e| format!("{}: {e}", rpf.display()))?;
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    const TAG: &[u8] = b"_Wrapped\0";
    let mut seen = std::collections::HashSet::new();
    let mut built = Built { programs: 0, written: 0 };
    let mut at = 0;
    while let Some(i) = data[at..].windows(TAG.len()).position(|w| w == TAG) {
        let p = at + i + TAG.len();
        at = p;
        let (Some(stored), Some(again), Some(size)) = (u32_at(p + 1), u32_at(p + 9), u32_at(p + 13)) else { break };
        let start = p + 17;
        if stored != again || size as u64 > MAX_SHADER_BYTES || start + stored > data.len() {
            continue;
        }
        let mut program = Vec::with_capacity(size);
        let mut inflate = flate2::Decompress::new(false);
        if inflate.decompress_vec(&data[start..start + stored], &mut program, flate2::FlushDecompress::Finish).is_err()
            || program.len() != size {
            continue;
        }
        at = start + stored;
        let Some(parts) = dxbc_parts(&program) else { continue };
        if !seen.insert(sha256(&program)) {
            continue;
        }
        built.programs += 1;
        built.written += write_request(archive, &program) as usize;
        if let Some(stage_roots) = dxil_stage(&program, &parts).and_then(|s| roots.get(&s)) {
            for root in stage_roots.values() {
                built.written += write_request(archive, &with_root(&program, parts.len(), root)) as usize;
            }
        }
    }
    if built.programs == 0 {
        return Err(format!("no shader programs found in {}", rpf.display()));
    }
    fs::write(&marker, stamp).map_err(|e| e.to_string())?;
    Ok(Some(built))
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
                        Outcome::Unsupported(m) => { stats.note("unsupported", &m); &stats.unsupported }
                        Outcome::Failed(m) => { stats.note("failed", &m); &stats.failed }
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
        // TUXBOX_PREWARM=0 serves the title's requests at once, without building the archive first.
        if std::env::var("TUXBOX_PREWARM").as_deref() != Ok("0") {
            prewarm(&tools, &cache, &archive, &private.join("prewarm"), workers, &stats);
        }
        let (s, st) = (stop.clone(), stats.clone());
        let log = Arc::new(log);
        let handle = thread::Builder::new().name("shader-worker".into()).spawn(move || {
            let tools = Arc::new(tools);
            let busy: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Default::default();
            let active = Arc::new(AtomicUsize::new(0));
            // test110: a title thread waiting on one translation sat behind
            // hundreds of background requests in directory order. A key the
            // title waits on has a marker in urgent/ (d3d12_xs writes it);
            // those requests go first, and a scan stops to start over as soon
            // as one appears. NEONBRIDGE_URGENT_SHADERS=0 keeps directory order.
            let urgent_dir = session_dir.join("urgent");
            let urgent_on = std::env::var("NEONBRIDGE_URGENT_SHADERS").as_deref() != Ok("0");
            let urgent_keys = |busy: &std::collections::HashSet<String>| -> Vec<String> {
                if !urgent_on { return Vec::new(); }
                // A marker whose request is gone (served before the marker was
                // written) is removed, or it would stop every scan for good.
                fs::read_dir(&urgent_dir).map(|d| d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|k| {
                        let pending = ["dxbc", "stencil", "link"].iter().any(|x| requests.join(format!("{k}.{x}")).exists());
                        if !pending { let _ = fs::remove_file(urgent_dir.join(k)); }
                        pending
                    })
                    .filter(|k| !busy.iter().any(|n| n.rsplit_once('.').is_some_and(|(b, _)| b == k)))
                    .collect()).unwrap_or_default()
            };
            while !s.load(Ordering::Acquire) {
                let mut idle = true;
                if let Ok(entries) = fs::read_dir(&requests) {
                    let mut entries: Vec<_> = entries.flatten().collect();
                    let first = urgent_keys(&busy.lock().unwrap());
                    if !first.is_empty() {
                        entries.sort_by_key(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            !first.iter().any(|k| name.rsplit_once('.').is_some_and(|(b, _)| b == k))
                        });
                    }
                    for entry in entries {
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
                        let mine = name.rsplit_once('.').map_or("", |(k, _)| k).to_owned();
                        let mut rescan = false;
                        while active.load(Ordering::Acquire) >= workers {
                            thread::sleep(Duration::from_millis(5));
                            if urgent_keys(&busy.lock().unwrap()).iter().any(|k| *k != mine) {
                                rescan = true;
                                break;
                            }
                        }
                        if rescan {
                            busy.lock().unwrap().remove(&name);
                            break;
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
                            let _ = fs::remove_file(session.join("urgent").join(&key));
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

