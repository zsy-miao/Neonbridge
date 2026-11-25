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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;
use std::{fs, thread};

const MAX_SHADER_BYTES: u64 = 64 << 20;

/// The running title's process id (main.rs sets it at each start): a child
/// of this process that the worker's own size below leaves out.
pub static GAME_PID: AtomicU32 = AtomicU32::new(0);

/// An explicit `neonbridge --prewarm` (main.rs prewarm_only): nothing waits on
/// its compiles, so -O1 gets 120 s (NEONBRIDGE_DXC_O1_SECS still sets it) and
/// -O0 DXIL already cached (`.o0`) is compiled again at -O1.
pub static PREWARM_ONLY: AtomicBool = AtomicBool::new(false);

fn upgrading() -> bool {
    PREWARM_ONLY.load(Ordering::Relaxed) && std::env::var("NEONBRIDGE_PREWARM_UPGRADE_O0").as_deref() != Ok("0")
}

/// The private commit of `pid` and, with `children`, of the processes it
/// started but the title (this worker's xshader and dxc).
fn private_bytes(pid: u32, children: bool) -> u64 {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let mut pids = vec![pid];
    if children {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot != INVALID_HANDLE_VALUE {
                let mut entry: PROCESSENTRY32W = std::mem::zeroed();
                entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
                let mut more = Process32FirstW(snapshot, &mut entry) != 0;
                while more {
                    // The title is a child too; the caller counts it once.
                    if entry.th32ParentProcessID == pid && entry.th32ProcessID != GAME_PID.load(Ordering::Relaxed) {
                        pids.push(entry.th32ProcessID);
                    }
                    more = Process32NextW(snapshot, &mut entry) != 0;
                }
                CloseHandle(snapshot);
            }
        }
    }
    pids.into_iter().map(|p| unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, p);
        if process.is_null() {
            return 0;
        }
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessMemoryInfo(process, (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(), counters.cb) != 0;
        CloseHandle(process);
        if ok { counters.PrivateUsage as u64 } else { 0 }
    }).sum()
}

/// Whether a translation job may start beside `running` others: this
/// worker's private bytes, with its xshader and dxc, where the translations'
/// memory goes, are under NEONBRIDGE_WORKER_CEILING_GB (8; 0 off), or none
/// runs. Ported from five-star-recomp fec516c and 36c506c (GPL-3.0,
/// CompileBudget.RoomForPrograms), with credit. The title's private bytes are
/// left out: they are its own and the driver's (test140 had 19.8 GB), and
/// past the 28 GB compile ceiling counting them ran translations one at a
/// time while the title still waited on them.
fn room_for_programs(running: usize) -> bool {
    static CEILING: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ceiling = *CEILING.get_or_init(|| {
        std::env::var("NEONBRIDGE_WORKER_CEILING_GB").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(8) << 30
    });
    if running == 0 || ceiling == 0 {
        return true;
    }
    // Sampled at most every 250 ms: the dispatch loop asks every 5 ms.
    static LAST: std::sync::Mutex<Option<(std::time::Instant, u64)>> = std::sync::Mutex::new(None);
    let total = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        match *last {
            Some((at, total)) if at.elapsed() < Duration::from_millis(250) => total,
            _ => {
                let total = private_bytes(std::process::id(), true);
                *last = Some((std::time::Instant::now(), total));
                total
            }
        }
    };
    if total <= ceiling {
        return true;
    }
    static HELD: AtomicUsize = AtomicUsize::new(0);
    let n = HELD.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n.is_power_of_two() {
        println!("[launch] shaders: a translation waits: the shader worker with its xshader and dxc holds {} MiB private, over NEONBRIDGE_WORKER_CEILING_GB={} ({running} running, {n} waits so far)",
            total >> 20, ceiling >> 30);
    }
    false
}
const XSHADER_UNSUPPORTED_HLSL: i32 = 65;

pub struct Tools {
    pub xshader: PathBuf,
    /// Hash of the shader-compiler sources xshader was built from; keys the
    /// translation cache in place of the exe's bytes (None: the exe's bytes).
    pub source_key: Option<String>,
    pub dxc: PathBuf,
    /// dxc `-D` defines; bind_cache fills them from the host wave range.
    pub defines: Vec<String>,
    /// Where dxc's output is kept by what it was given (`<cache_root>/dxil/<dxc
    /// toolchain fingerprint>`); bind_cache fills it.
    pub dxil: Option<PathBuf>,
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
    /// Retranslates stale programs while the game runs (stale_on); not
    /// joined at stop, it stops taking programs.
    _background: Option<thread::JoinHandle<()>>,
    handle: Option<thread::JoinHandle<()>>,
    pub stats: Arc<Stats>,
    private: PathBuf,
}

/// Stale translations: a program's last good translation from any xshader
/// build is kept in `<cache_root>/last/<key>.cso` (with `.sha256`, and
/// `.from` naming the build that made it). After an xshader change every
/// build-keyed entry was stale and the launch translated the whole seen set
/// again (test143: ten minutes) before the game started. Now only programs
/// never translated hold the launch; the rest start on their last
/// translation, are translated again in the background, and the title's
/// requests are answered from the last translation at once while the fresh
/// one is made (it is served from the next request on). VIctory c47b529 keys
/// its cache by the translator and five-star 2d73fe7 keeps it across
/// rebuilds; this keeps the last one across translator builds as a fallback.
/// NEONBRIDGE_STALE_SHADERS=0 blocks on every stale program as before.
pub fn stale_on() -> bool {
    std::env::var("NEONBRIDGE_STALE_SHADERS").as_deref() != Ok("0")
}

pub static STALE_SERVED: AtomicUsize = AtomicUsize::new(0);
pub static STALE_REPLACED: AtomicUsize = AtomicUsize::new(0);
pub static STALE_UNCHANGED: AtomicUsize = AtomicUsize::new(0);

/// `<cache_root>/last` for a build cache `<cache_root>/runtime/<build>`.
fn last_dir(cache: &Path) -> Option<PathBuf> {
    Some(cache.parent()?.parent()?.join("last"))
}

/// The last good translation of `key` from any build, verified.
fn last_good(cache: &Path, key: &str) -> Option<Vec<u8>> {
    let dir = last_dir(cache)?;
    let (bytes, sum) = (fs::read(dir.join(format!("{key}.cso"))).ok()?, fs::read(dir.join(format!("{key}.sha256"))).ok()?);
    (bytes.len() as u64 <= MAX_SHADER_BYTES && sum == sha256(&bytes).as_bytes() && has_dxil_container(&bytes)).then_some(bytes)
}

/// Keeps `bytes` as `key`'s last good translation (made by build `cache`),
/// counting whether it replaced a different one.
fn remember(cache: &Path, key: &str, bytes: &[u8]) {
    let Some(dir) = last_dir(cache) else { return };
    let build = cache.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let from = dir.join(format!("{key}.from"));
    if fs::read_to_string(&from).is_ok_and(|b| b == build) {
        return;
    }
    match last_good(cache, key) {
        Some(old) if old == bytes => { STALE_UNCHANGED.fetch_add(1, Ordering::Relaxed); }
        Some(_) => { STALE_REPLACED.fetch_add(1, Ordering::Relaxed); }
        None => {}
    }
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    // Under temporary names, then renamed: a reader sees a whole pair or the old one.
    let tag = format!("{}.{}", std::process::id(), sha256(key.as_bytes()).get(..8).unwrap_or("x"));
    let (cso, sum) = (dir.join(format!("{key}.cso.{tag}.tmp")), dir.join(format!("{key}.sha256.{tag}.tmp")));
    let ok = fs::write(&cso, bytes).is_ok() && fs::write(&sum, sha256(bytes)).is_ok()
        && fs::rename(&cso, dir.join(format!("{key}.cso"))).is_ok() && fs::rename(&sum, dir.join(format!("{key}.sha256"))).is_ok();
    if ok {
        let _ = fs::write(&from, build);
    } else {
        let _ = fs::remove_file(&cso);
        let _ = fs::remove_file(&sum);
    }
}

/// A program this build refuses loses its last translation: the refusal may
/// be what an older build got wrong.
fn forget(cache: &Path, key: &str) {
    if let Some(dir) = last_dir(cache) {
        for ext in ["cso", "sha256", "from"] {
            let _ = fs::remove_file(dir.join(format!("{key}.{ext}")));
        }
    }
}

/// Fills `last` once from the build caches already on disk, newest first, so
/// the first xshader change after this landed already starts on them.
fn seed_last(cache_root: &Path, cache: &Path) -> usize {
    let Some(dir) = last_dir(cache) else { return 0 };
    if dir.exists() || fs::create_dir_all(&dir).is_err() {
        return 0;
    }
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(cache_root.join("runtime")).into_iter().flatten().flatten()
        .filter(|e| is_key(&e.file_name().to_string_lossy()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seeded = 0;
    for (_, build) in dirs.iter().take(8) {
        let Ok(entries) = fs::read_dir(build) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(key) = name.strip_suffix(".cso") else { continue };
            if dir.join(&name).exists() {
                continue;
            }
            let (Ok(bytes), Ok(sum)) = (fs::read(entry.path()), fs::read(build.join(format!("{key}.sha256")))) else { continue };
            if bytes.len() as u64 <= MAX_SHADER_BYTES && sum == sha256(&bytes).as_bytes() && has_dxil_container(&bytes) {
                remember(build, key, &bytes);
                seeded += 1;
            }
        }
    }
    seeded
}

/// One line on how many of this run's programs were served stale.
pub fn stale_summary() -> String {
    format!("stale shaders: {} served from an older xshader's translation while the new one was made, {} retranslated to different code, {} retranslated unchanged (NEONBRIDGE_STALE_SHADERS=0 waits for every one)",
        STALE_SERVED.load(Ordering::Relaxed), STALE_REPLACED.load(Ordering::Relaxed), STALE_UNCHANGED.load(Ordering::Relaxed))
}

/// dxc outputs reused by the normalised HLSL key alone (not the exact text).
pub static NORMALISED_REUSES: AtomicUsize = AtomicUsize::new(0);

/// HLSL with `//` and `/* */` comments removed, each line trimmed with its
/// runs of spaces and tabs made one space, and blank lines dropped. Line
/// breaks stay (preprocessor directives end at them), string literals are
/// kept as written. dxc's DXIL does not depend on any of it without debug
/// info, which the worker never asks for.
fn normalised_hlsl(source: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(source.len());
    let mut line: Vec<u8> = Vec::new();
    let (mut i, mut block, mut string) = (0, false, false);
    let flush = |line: &mut Vec<u8>, out: &mut Vec<u8>| {
        let mut prev_space = true;
        let mut compact: Vec<u8> = Vec::with_capacity(line.len());
        for &c in line.iter() {
            let space = matches!(c, b' ' | b'\t' | b'\r');
            if !space {
                compact.push(c);
            } else if !prev_space {
                compact.push(b' ');
            }
            prev_space = space;
        }
        while compact.last() == Some(&b' ') {
            compact.pop();
        }
        if !compact.is_empty() {
            out.extend_from_slice(&compact);
            out.push(b'\n');
        }
        line.clear();
    };
    while i < source.len() {
        let c = source[i];
        let next = source.get(i + 1).copied();
        if block {
            if c == b'*' && next == Some(b'/') {
                block = false;
                line.push(b' ');
                i += 2;
                continue;
            }
            if c == b'\n' {
                flush(&mut line, &mut out);
            }
            i += 1;
            continue;
        }
        if string {
            line.push(c);
            if c == b'\\' && let Some(n) = next {
                line.push(n);
                i += 2;
                continue;
            }
            if c == b'"' || c == b'\n' {
                string = false;
            }
            i += 1;
            continue;
        }
        match (c, next) {
            (b'/', Some(b'/')) => {
                while i < source.len() && source[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            (b'/', Some(b'*')) => {
                block = true;
                i += 2;
                continue;
            }
            (b'"', _) => {
                string = true;
                line.push(c);
            }
            (b'\n', _) => flush(&mut line, &mut out),
            _ => line.push(c),
        }
        i += 1;
    }
    flush(&mut line, &mut out);
    out
}

#[derive(Default)]
pub struct Stats {
    pub compiled: AtomicUsize,
    pub cached: AtomicUsize,
    pub unsupported: AtomicUsize,
    pub failed: AtomicUsize,
    /// Archived requests translated before the title started.
    pub prewarmed: AtomicUsize,
    /// Translations whose HLSL dxc had compiled before (another xshader build).
    pub reused: AtomicUsize,
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

pub fn sha256_hex(data: &[u8]) -> String {
    sha256(data)
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

/// Peak private bytes of the xshader and dxc processes this launcher ran: the
/// per-job memory the worker ceiling has to fit (reported after each prewarm).
static JOB_PEAK_MAX: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static JOB_PEAK_SUM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static JOB_PEAK_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_peak(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    c.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if unsafe { GetProcessMemoryInfo(child.as_raw_handle() as _, &mut c, c.cb) } != 0 {
        let peak = c.PeakPagefileUsage as u64;
        JOB_PEAK_MAX.fetch_max(peak, Ordering::Relaxed);
        JOB_PEAK_SUM.fetch_add(peak, Ordering::Relaxed);
        JOB_PEAK_N.fetch_add(1, Ordering::Relaxed);
    }
}

/// Where the prewarm's time goes: xshader (translation) and dxc milliseconds, summed over jobs.
static TRANSLATE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TRANSLATE_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DXC_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DXC_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_times(outcome: &Outcome) {
    let (xs, dxc) = match outcome {
        Outcome::Compiled(xs, dxc) => (Some(*xs), Some(*dxc)),
        Outcome::Reused(xs) => (Some(*xs), None),
        _ => (None, None),
    };
    if let Some(ms) = xs {
        TRANSLATE_MS.fetch_add(ms, Ordering::Relaxed);
        TRANSLATE_N.fetch_add(1, Ordering::Relaxed);
    }
    if let Some(ms) = dxc {
        DXC_MS.fetch_add(ms, Ordering::Relaxed);
        DXC_N.fetch_add(1, Ordering::Relaxed);
    }
}

fn job_times(workers: usize) -> String {
    let (xs, xn, dx, dn) = (TRANSLATE_MS.load(Ordering::Relaxed), TRANSLATE_N.load(Ordering::Relaxed), DXC_MS.load(Ordering::Relaxed), DXC_N.load(Ordering::Relaxed));
    format!("time: xshader {:.1} s over {xn} translations ({} ms each), dxc {:.1} s over {dn} compiles ({} ms each); over {workers} workers that is about {:.0} s of xshader and {:.0} s of dxc",
        xs as f64 / 1000.0, xs / xn.max(1), dx as f64 / 1000.0, dx / dn.max(1), xs as f64 / 1000.0 / workers as f64, dx as f64 / 1000.0 / workers as f64)
}

fn job_peaks() -> String {
    let n = JOB_PEAK_N.load(Ordering::Relaxed).max(1);
    format!("xshader/dxc process peaks: mean {} MiB, max {} MiB over {} processes; in-process compiles still running past their time: {}",
        JOB_PEAK_SUM.load(Ordering::Relaxed) / n >> 20, JOB_PEAK_MAX.load(Ordering::Relaxed) >> 20, JOB_PEAK_N.load(Ordering::Relaxed), crate::dxc::abandoned())
}

/// `run`, also ending the tool early (as a timeout) once `give_up` says so.
fn run_until(cmd: &mut Command, timeout: Duration, give_up: impl Fn() -> bool) -> Result<(), ToolError> {
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
        .map_err(|e| ToolError::Other(format!("{e}")))?;
    let start = std::time::Instant::now();
    let mut polls = 0u32;
    loop {
        polls += 1;
        // Every 100 ms: the predicate reads the file system.
        if polls % 10 == 0 && give_up() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ToolError::Other("timed out".into()));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                note_peak(&child);
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
/// The TB_* switches xshader inherits, as sorted NAME=value.
fn xshader_switches() -> Vec<String> {
    let mut switches: Vec<String> = std::env::vars().filter(|(k, _)| k.starts_with("TB_")).map(|(k, v)| format!("{k}={v}")).collect();
    switches.sort();
    switches
}

fn bind_cache(tools: &Tools, cache_root: &Path, private: &Path) -> Result<(Tools, PathBuf), String> {
    fs::create_dir_all(private).map_err(|e| e.to_string())?;
    let xshader = private.join("xshader.exe");
    fs::copy(&tools.xshader, &xshader).map_err(|e| format!("copy xshader: {e}"))?;
    // Its cdylib from the same build goes beside the copy (xshader_lib.rs); a
    // rebuild then replaces the original, not a DLL this process holds.
    let _ = fs::remove_file(xshader.with_file_name(crate::xshader_lib::DLL));
    if let Some(dll) = crate::xshader_lib::beside(&tools.xshader) {
        let _ = fs::copy(&dll, xshader.with_file_name(crate::xshader_lib::DLL));
    }
    let version = Command::new(&tools.dxc).arg("--version").output().map_err(|e| format!("dxc: {e}"))?.stdout;
    let defines = wave_size_defines(cache_root);
    let mut fp = Sha256::new();
    fp.update(&version);
    // The defines change the DXIL, so they're part of the cache key.
    for d in &defines {
        fp.update(b"-D");
        fp.update(d.as_bytes());
    }
    // The translator is keyed by its source, not its bytes: a no-op rebuild
    // relinked xshader.exe and invalidated every cached shader (1,308).
    // VIctory c47b529 keys its cache by the translator.
    match &tools.source_key {
        Some(k) => { fp.update(b"-S"); fp.update(k.as_bytes()); }
        None => fp.update(sha256(&fs::read(&xshader).map_err(|e| e.to_string())?).as_bytes()),
    }
    fp.update(sha256(&fs::read(&tools.dxc).map_err(|e| e.to_string())?).as_bytes());
    // xshader inherits this process's environment, and its TB_* switches change
    // the code it emits: a cache made under other switches answered for them
    // (a TB_ switch reached only shaders not yet cached). With none set the key
    // is unchanged, so the existing cache stays valid.
    for s in xshader_switches() {
        fp.update(b"-E");
        fp.update(s.as_bytes());
    }
    let cache = cache_root.join("runtime").join(hex(&fp.finalize()));
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    // dxc's output depends only on its input, its own binaries and the
    // defines, not on xshader: keyed apart from the cache above, DXIL for HLSL
    // a new xshader still emits unchanged is reused (VIctory c47b529 keys its
    // cache by the translator, five-star 2d73fe7 keeps it across rebuilds).
    let mut dxc_fp = Sha256::new();
    dxc_fp.update(&version);
    for d in &defines {
        dxc_fp.update(b"-D");
        dxc_fp.update(d.as_bytes());
    }
    dxc_fp.update(sha256(&fs::read(&tools.dxc).map_err(|e| e.to_string())?).as_bytes());
    if let Some(dll) = tools.dxc.parent().map(|d| d.join("dxil.dll")) && let Ok(bytes) = fs::read(dll) {
        dxc_fp.update(sha256(&bytes).as_bytes());
    }
    let dxil = (std::env::var("NEONBRIDGE_DXIL_REUSE").as_deref() != Ok("0"))
        .then(|| cache_root.join("dxil").join(hex(&dxc_fp.finalize())))
        .filter(|d| fs::create_dir_all(d).is_ok());
    Ok((Tools { xshader, source_key: tools.source_key.clone(), dxc: tools.dxc.clone(), defines, dxil }, cache))
}

/// Compiled carries the milliseconds spent in xshader and in dxc.
/// Reused carries the xshader milliseconds of a translation whose HLSL dxc had compiled before.
enum Outcome { Compiled(u64, u64), Reused(u64), Cached, Unsupported(String), Failed(String) }

/// `cancel`: the prewarm's tail ran out (prewarm_tail): a dxc still running
/// is ended and the program left to the title's request, with no failure kept.
fn compile(source: &Path, key: &str, ext: &str, tools: &Tools, cache: &Path, session: &Path, o0_first: bool, cancel: Option<&AtomicBool>) -> Outcome {
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
            && !(upgrading() && crate::dxil_upgrade::marker(&dest).exists())
            && bytes.len() as u64 <= MAX_SHADER_BYTES && sum == sha256(&bytes).as_bytes() {
            remember(cache, key, &bytes);
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
        let translate_started = std::time::Instant::now();
        // In-process first (xshader_lib.rs): the same inputs as the command line below.
        let dll = tools.xshader.with_file_name(crate::xshader_lib::DLL);
        let inproc = if crate::xshader_lib::enabled() && dll.is_file() {
            let read = |p: &Path| fs::read(p).map_err(|e| format!("read {}: {e}", p.display()));
            let (consumer, order) = if ext == "link" {
                (Some(read(&work.join("pixel.dxbc"))?), Some(read(&work.join("pixel.cso"))?))
            } else { (None, None) };
            let stencil = (ext == "stencil").then(|| key.split_once("_rt").and_then(|(_, n)| n.parse().ok())).flatten();
            match crate::xshader_lib::translate(&dll, read(&cooked)?, consumer, order, stencil, Duration::from_secs(120), cancel) {
                crate::xshader_lib::Outcome::Done(text, prof) => {
                    fs::write(&hlsl, text).and_then(|_| fs::write(&profile, prof)).map_err(|e| format!("write {}: {e}", hlsl.display()))?;
                    true
                }
                crate::xshader_lib::Outcome::Unsupported(msg) => {
                    let _ = fs::write(&refusal, format!("{key}
{msg}"));
                    forget(cache, key);
                    return Ok(Outcome::Unsupported(msg));
                }
                crate::xshader_lib::Outcome::Failed(msg) => return Err(format!("xshader failed (1): {msg}")),
                crate::xshader_lib::Outcome::Deferred =>
                    return Err("xshader: left to the title's request at the prewarm's end (NEONBRIDGE_PREWARM_TAIL_S)".into()),
                crate::xshader_lib::Outcome::Fallback(why) => {
                    static SAID: std::sync::Once = std::sync::Once::new();
                    SAID.call_once(|| println!("[launch] shaders: in-process xshader fell back to the xshader process: {why} (more may follow unlogged)"));
                    false
                }
            }
        } else { false };
        if !inproc { match run_until(Command::new(&tools.xshader).arg(&cooked).args(&flags).arg("--hlsl-out").arg(&hlsl)
            .arg("--profile-out").arg(&profile), Duration::from_secs(120), || cancel.is_some_and(|c| c.load(Ordering::Acquire))) {
            Err(ToolError::Other(msg)) if msg == "timed out" && cancel.is_some_and(|c| c.load(Ordering::Acquire)) =>
                return Err("xshader: left to the title's request at the prewarm's end (NEONBRIDGE_PREWARM_TAIL_S)".into()),
            Ok(()) => {}
            Err(ToolError::Exit(XSHADER_UNSUPPORTED_HLSL, msg)) => {
                let _ = fs::write(&refusal, format!("{key}\n{msg}"));
                forget(cache, key);
                return Ok(Outcome::Unsupported(msg));
            }
            Err(ToolError::Exit(c, msg)) => return Err(format!("xshader failed ({c}): {msg}")),
            Err(ToolError::Other(msg)) if msg == "timed out" => return Err("xshader: timed out after 120 s".into()),
            Err(ToolError::Other(msg)) => return Err(format!("xshader: {msg}")),
        } }
        let translate_ms = translate_started.elapsed().as_millis() as u64;
        let target = fs::read_to_string(&profile).map_err(|e| format!("xshader wrote no profile: {e}"))?.trim().to_owned();
        let allowed: &[&str] = if ext == "link" {
            // A mesh producer links too (d3d12 stream.rs mesh_link, five-star-recomp
            // ddcaa54): 94ba67a's prewarm failed c20f5160 on ms_6_6.
            &["vs_6_0", "vs_6_1", "vs_6_6", "gs_6_0", "gs_6_1", "gs_6_6", "ms_6_5", "ms_6_6"]
        } else {
            &["cs_6_0", "cs_6_1", "cs_6_6", "ps_6_0", "ps_6_1", "ps_6_6", "vs_6_0", "vs_6_1", "vs_6_6",
              "gs_6_0", "gs_6_1", "gs_6_6", "cs_6_4", "cs_6_5", "ms_6_5", "ms_6_6", "as_6_5", "as_6_6", "lib_6_3"]
        };
        if !allowed.contains(&target.as_str()) {
            return Err(format!("unsupported compiler profile {target:?}"));
        }
        // The same HLSL for the same profile compiled before (under another
        // xshader build: test-to-test, most programs' HLSL is unchanged; the
        // helper's exit-stub commit changed 12 of 885) is not compiled again.
        // A second key hashes the HLSL with comments and spacing removed
        // (normalised_hlsl): an xshader change to its comments alone (each
        // instruction is annotated with its address and disassembly) or to
        // its layout no longer sends every program back through dxc.
        // NEONBRIDGE_DXIL_REUSE_NORMALISED=0 keys by the exact text only.
        let source = tools.dxil.as_ref().and_then(|_| fs::read(&hlsl).ok());
        let key_path = |text: &[u8], prefix: &str| tools.dxil.as_ref().map(|dir| {
            let mut key = Sha256::new();
            key.update(target.as_bytes());
            key.update(&[0]);
            key.update(text);
            dir.join(format!("{prefix}{}.cso", hex(&key.finalize())))
        });
        let reused = source.as_deref().and_then(|s| key_path(s, ""));
        let normalised = source.as_deref()
            .filter(|_| std::env::var("NEONBRIDGE_DXIL_REUSE_NORMALISED").as_deref() != Ok("0"))
            .and_then(|s| key_path(&normalised_hlsl(s), "n-"));
        let stored = |path: &PathBuf| -> Option<Vec<u8>> {
            let (bytes, sum) = (fs::read(path).ok()?, fs::read(path.with_extension("sha256")).ok()?);
            (bytes.len() as u64 <= MAX_SHADER_BYTES && sum == sha256(&bytes).as_bytes() && has_dxil_container(&bytes)).then_some(bytes)
        };
        let stored = |path: &PathBuf| (!(upgrading() && crate::dxil_upgrade::marker(path).exists())).then(|| stored(path)).flatten();
        let found = reused.as_ref().and_then(|p| Some((stored(p)?, p))).or_else(|| {
            let found = normalised.as_ref().and_then(|p| Some((stored(p)?, p)))?;
            NORMALISED_REUSES.fetch_add(1, Ordering::Relaxed);
            Some(found)
        });
        // -O0 DXIL (dxil_upgrade.rs) goes on to this build's cache marked too, and is
        // queued for its -O1 compile, if it is not queued already.
        let upgrade = |dxil_o0: bool| {
            let up = crate::dxil_upgrade::marker(&dest);
            if !dxil_o0 {
                let _ = fs::remove_file(&up);
                return;
            }
            let _ = fs::write(&up, b"");
            if let (Some(dir), Some(text), Some(id)) = (tools.dxil.as_ref(), source.as_deref(), reused.as_ref().and_then(|p| p.file_stem())) {
                let paths: Vec<&Path> = reused.iter().chain(&normalised).map(|p| p.as_path()).chain([dest.as_path()]).collect();
                crate::dxil_upgrade::enqueue(dir, &id.to_string_lossy(), &target, text, &paths);
            }
        };
        if let Some((bytes, from)) = found {
            upgrade(crate::dxil_upgrade::marker(from).exists());
            // Into this build's cache as a compile would store it (renamed in whole).
            let (copy, stamp) = (work.join("reused.cso"), work.join("reused.sha256"));
            fs::write(&copy, &bytes).and_then(|_| fs::write(&stamp, sha256(&bytes)))
                .and_then(|_| fs::rename(&copy, &dest)).and_then(|_| fs::rename(&stamp, &checksum))
                .map_err(|e| format!("store {}: {e}", dest.display()))?;
            remember(cache, key, &bytes);
            publish(&bytes)?;
            return Ok(Outcome::Reused(translate_ms));
        }
        let dxc = |opt: &str, secs: u64, give_up: &dyn Fn() -> bool| {
            let mut dxc = Command::new(&tools.dxc);
            dxc.arg("-T").arg(&target).arg(opt);
            for d in &tools.defines {
                dxc.arg("-D").arg(d);
            }
            if target != "lib_6_3" {
                dxc.args(["-E", "main"]);
            }
            // In-process (src/dxc.rs): the same arguments without the file
            // names, no dxc.exe start and dxcompiler.dll load per program. Not
            // for -O1: its 10 s cut-off cannot kill an in-process compile, and
            // the 0577d56 prewarm (in-process from 570e0aa) held 9.4 GB private
            // in the launcher itself, no dxc children, 53 threads for 10 jobs:
            // every -O1 past its time kept compiling (up to ~0.7 GB each) beside
            // its -O0 retry, uncounted, so the worker ceiling held the prewarm at
            // 0.3 programs/s. dxc.exe takes -O1 and is killed at its time.
            if crate::dxc::enabled() && opt != "-O1" {
                let args = dxc.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
                let source = fs::read(&hlsl).map_err(|e| ToolError::Other(format!("read {}: {e}", hlsl.display())))?;
                return match crate::dxc::compile(&tools.dxc, source, args, Duration::from_secs(secs), give_up, false) {
                    Ok(bytes) => fs::write(&output, bytes).map_err(|e| ToolError::Other(format!("write {}: {e}", output.display()))),
                    Err(crate::dxc::Error::Compiler(msg)) => Err(ToolError::Exit(1, msg.chars().take(600).collect::<String>().trim().to_owned())),
                    Err(crate::dxc::Error::Other(msg)) => Err(ToolError::Other(msg)),
                };
            }
            run_until(dxc.arg("-Fo").arg(&output).arg(&hlsl), Duration::from_secs(secs), give_up)
        };
        // Large skinning and edge-emit programs can spend minutes in DXC's optimiser;
        // unoptimised DXIL compiles in seconds and the driver optimises it again.
        let dxc_started = std::time::Instant::now();
        // test112: dxc was 98% of shader time (401 s for 252 compiles, one took
        // 92 s), starving pipelines the title waited on. -O1 now gets
        // NEONBRIDGE_DXC_O1_SECS (10) before the -O0 retry, not 120.
        let o1_secs = std::env::var("NEONBRIDGE_DXC_O1_SECS").ok().and_then(|v| v.trim().parse().ok()).filter(|&s| s > 0)
            .unwrap_or(if PREWARM_ONLY.load(Ordering::Relaxed) { 120 } else { 10 });
        // test130/131 died at the scene load (the title's own watchdog, game
        // +0x23cf620 "GPU Loc Sig") with its render jobs blocked in pipeline
        // binds on these replies: 14 of 229 compiles spent their 10 s on an
        // -O1 pass that timed out and was discarded (140 of 315 s of dxc).
        // Idle-machine timings: 727f287e -O1 8.8 s vs -O0 0.5 s; a64767c5
        // -O1 65 s vs -O0 8.3 s. A request a title thread waits on (its
        // urgent/ marker, d3d12_xs writes it) goes straight to -O0, and an -O1
        // pass under way ends when the marker appears.
        // NEONBRIDGE_DXC_URGENT_O0=0 keeps -O1 first for every request.
        let urgent_o0 = std::env::var("NEONBRIDGE_DXC_URGENT_O0").as_deref() != Ok("0");
        let marker = session.join("urgent").join(key);
        let urgent = || urgent_o0 && marker.exists();
        // The prewarm compiles at -O0 straight away: the f95f59a cold prewarm
        // ran 14 workers at 1.9 programs/s (7.4 s a program), 83% of them
        // through dxc, most of that the -O1 pass (8.8 s vs 0.5 s at -O0 on
        // 727f287e; up to its 10 s cut-off, then -O0 again). The driver
        // optimises the DXIL again, as for the urgent path above.
        // Opt-in only (NEONBRIDGE_PREWARM_DXC_O0=1, prewarm_o0).
        // A dxc failure or timeout is kept beside the HLSL's reuse path
        // (`.dxcfail`), so the same HLSL is not sent to dxc again on every
        // launch (test152 sat at 1336/1338 on two programs a full prewarm of
        // the same rev had already timed out on); changed HLSL is a new path.
        // NEONBRIDGE_DXC_RETRY_FAILED=1 retries them.
        // Only a compile error is kept: dxc's own "error:" text on stderr. A
        // kill (taskkill while a prewarm stops: exit 1, empty stderr), a
        // timeout or a silent exit is not a verdict on the HLSL; marks written
        // for those before (293a605) are dropped as they are found. The
        // prewarm's tail cut-off, not this mark, keeps slow programs off the launch.
        let compile_error = |m: &str| m.contains("error:") || m.contains("error X");
        let failed_mark = reused.as_ref().map(|p| p.with_extension("dxcfail"));
        if let Some(reason) = failed_mark.as_ref().and_then(|p| fs::read_to_string(p).ok()) {
            if !compile_error(&reason) {
                let _ = failed_mark.as_ref().map(fs::remove_file);
            } else if std::env::var("NEONBRIDGE_DXC_RETRY_FAILED").as_deref() != Ok("1") {
                return Err(format!("{} (kept from an earlier run for this HLSL; NEONBRIDGE_DXC_RETRY_FAILED=1 retries)", reason.trim()));
            }
        }
        let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Acquire));
        let keep_failure = |reason: &str| {
            if let Some(p) = &failed_mark {
                let _ = fs::write(p, reason);
            }
        };
        let first = if o0_first || urgent() { Err(ToolError::Other("timed out".into())) } else { dxc("-O1", o1_secs, &|| urgent() || cancelled()) };
        let mut dxil_o0 = false;
        let compiled = match first {
            Err(ToolError::Other(msg)) if msg == "timed out" && !cancelled() => {
                let _ = fs::remove_file(&output);
                dxil_o0 = true;
                dxc("-O0", 120, &cancelled)
            }
            other => other,
        };
        match compiled {
            Ok(()) => {}
            Err(ToolError::Other(msg)) if msg == "timed out" && cancelled() =>
                return Err("dxc: left to the title's request at the prewarm's end (NEONBRIDGE_PREWARM_TAIL_S)".into()),
            Err(ToolError::Exit(c, msg)) => {
                let reason = format!("dxc failed ({c}): {msg}");
                if compile_error(&msg) {
                    keep_failure(&reason);
                }
                return Err(reason);
            }
            Err(ToolError::Other(msg)) => {
                return Err(format!("dxc: {msg}"));
            }
        }
        let dxc_ms = dxc_started.elapsed().as_millis() as u64;
        let bytes = fs::read(&output).map_err(|e| format!("dxc wrote no output: {e}"))?;
        if !has_dxil_container(&bytes) {
            return Err("compiler returned an invalid DXIL container".into());
        }
        let stamp = work.join("sha256");
        fs::write(&stamp, sha256(&bytes)).map_err(|e| e.to_string())?;
        for path in reused.iter().chain(&normalised) {
            // -O0 never over an -O1 result (another worker's, since the lookup missed).
            let up = crate::dxil_upgrade::marker(path);
            if dxil_o0 {
                if path.exists() && !up.exists() {
                    continue;
                }
                let _ = fs::write(&up, b"");
            }
            // Written stamp-first under a temporary name: a reader sees both or neither.
            let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
            if fs::write(&tmp, &bytes).and_then(|_| fs::write(path.with_extension("sha256"), sha256(&bytes)))
                .and_then(|_| fs::rename(&tmp, path)).is_err() {
                let _ = fs::remove_file(&tmp);
            } else if !dxil_o0 {
                // -O1 over an -O0 entry: no longer one.
                let _ = fs::remove_file(&up);
            }
        }
        fs::rename(&output, &dest).map_err(|e| format!("store {}: {e}", dest.display()))?;
        fs::rename(&stamp, &checksum).map_err(|e| e.to_string())?;
        upgrade(dxil_o0);
        remember(cache, key, &bytes);
        publish(&bytes)?;
        Ok(Outcome::Compiled(translate_ms, dxc_ms))
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
    // The title asked for it: mark it for the next launch's seen prewarm.
    if let Some(seen) = archive.parent().map(|r| r.join("seen")) {
        let _ = fs::create_dir_all(&seen);
    }
    let dest = archive.join(name);
    if fs::symlink_metadata(&dest).is_ok() {
        mark_seen(archive, name);
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
    mark_seen(archive, name);
}

/// `seen/<name>` for an archived request: a hard link to its archive copy, so
/// census and offline traces reading seen/ get the request's bytes (they were
/// 0-byte name markers; only the name was ever read here). A directory
/// request (a link pair) or a failed link keeps the empty marker.
/// NEONBRIDGE_SEEN_LINKS=0 writes empty markers only.
fn mark_seen(archive: &Path, name: &str) {
    let Some(seen) = archive.parent().map(|r| r.join("seen")) else { return };
    let (src, dst) = (archive.join(name), seen.join(name));
    if fs::metadata(&dst).is_ok_and(|m| m.len() > 0) {
        return;
    }
    let linked = std::env::var("NEONBRIDGE_SEEN_LINKS").as_deref() != Ok("0")
        && fs::metadata(&src).is_ok_and(|m| m.is_file())
        && { let _ = fs::remove_file(&dst); fs::hard_link(&src, &dst).is_ok() };
    if !linked && fs::symlink_metadata(&dst).is_err() {
        let _ = fs::write(&dst, b"");
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

/// Request names the title asked for in earlier launches (`<cache_root>/seen`).
/// The first launch with this marker list seeds it from the keys every
/// recent toolchain's cache dir holds a translation for, matched against
/// the archive (all of those came from title requests while
/// TUXBOX_PREWARM=0).
fn seen_requests(cache_root: &Path, archive: &Path) -> std::collections::HashSet<String> {
    let seen = cache_root.join("seen");
    let names = |dir: &Path| fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>()).unwrap_or_default();
    let marked: std::collections::HashSet<String> = names(&seen).into_iter().filter(|n| valid_request(n).is_some()).collect();
    if !marked.is_empty() {
        for n in &marked {
            mark_seen(archive, n);
        }
        return marked;
    }
    // The four newest toolchain dirs: older ones include whole-archive
    // builds (--build-shaders), ~3000 programs the title never asked for.
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(cache_root.join("runtime")).into_iter().flatten().flatten()
        .filter(|e| is_key(&e.file_name().to_string_lossy()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut keys = std::collections::HashSet::new();
    for (_, dir) in dirs.iter().take(4) {
        for name in names(dir) {
            if let Some(key) = name.strip_suffix(".cso") {
                keys.insert(key.to_owned());
            }
        }
    }
    let found: std::collections::HashSet<String> = names(archive).into_iter()
        .filter(|n| valid_request(n).is_some_and(|(k, _)| keys.contains(k))).collect();
    if fs::create_dir_all(&seen).is_ok() {
        for n in &found {
            mark_seen(archive, n);
        }
    }
    found
}

/// Translate archived requests into the cache before the title starts, so its
/// pipelines load translations instead of waiting on the worker. `only`
/// limits it to those request names.
///
/// Every 10 s it reports done/total, the rate and the time left to stdout and
/// `log`; no job starts after `deadline` (the rest are translated when the
/// title asks for them).
#[allow(clippy::too_many_arguments)]
fn prewarm(tools: &Tools, cache: &Path, archive: &Path, scratch: &Path, workers: usize, stats: &Stats,
    only: Option<&std::collections::HashSet<String>>, label: &str, deadline: std::time::Instant, log: &(dyn Fn(String) + Sync),
    stop: Option<&AtomicBool>) {
    let Ok(entries) = fs::read_dir(archive) else { return };
    let jobs: Vec<(PathBuf, String, String)> = entries.flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        if only.is_some_and(|o| !o.contains(&name)) {
            return None;
        }
        let (key, ext) = valid_request(&name)?;
        Some((e.path(), key.to_owned(), ext.to_owned()))
    }).collect();
    if jobs.is_empty() || fs::create_dir_all(scratch).is_err() {
        return;
    }
    let next = AtomicUsize::new(0);
    let running = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let finished = AtomicBool::new(false);
    let workers_done = AtomicUsize::new(0);
    // Set once every program is taken and the last ones have run
    // NEONBRIDGE_PREWARM_TAIL_S (20; 0 waits for all) more: their dxc is ended and
    // the title asks for them while it runs, so a few stragglers never hold the launch.
    let tail = AtomicBool::new(false);
    let tail_secs = std::env::var("NEONBRIDGE_PREWARM_TAIL_S").ok().and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(if PREWARM_ONLY.load(Ordering::Relaxed) { 0 } else { 20 });
    let total = jobs.len();
    let say = |line: String| {
        println!("[launch] {line}");
        log(line);
    };
    say(format!("{label}: {total} programs to translate {} (stops taking new ones {} s from now; NEONBRIDGE_PREWARM_MAX_S)",
        if stop.is_some() { "while the game runs" } else { "before the game starts" }, deadline.saturating_duration_since(std::time::Instant::now()).as_secs()));
    let started = std::time::Instant::now();
    thread::scope(|scope| {
        scope.spawn(|| {
            // The bar (loading screen, or one in-place console line under
            // --prewarm) follows real counts every 200 ms, its time left from
            // a moving average of the rate (crate::progress); the log keeps
            // one plain line every 10 s, which triage parses.
            let mut meter = crate::progress::Meter::new(total);
            let mut last = std::time::Instant::now();
            let mut taken_at: Option<std::time::Instant> = None;
            while !finished.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(200));
                if tail_secs > 0 && next.load(Ordering::Relaxed) >= total && !tail.load(Ordering::Relaxed) {
                    let at = *taken_at.get_or_insert_with(std::time::Instant::now);
                    if at.elapsed() >= Duration::from_secs(tail_secs) {
                        say(format!("{label}: the last {} programs left to the title's requests after {tail_secs} s (NEONBRIDGE_PREWARM_TAIL_S)", running.load(Ordering::Relaxed)));
                        tail.store(true, Ordering::Release);
                    }
                }
                meter.sample(done.load(Ordering::Relaxed));
                if stop.is_none() {
                    crate::progress::report(label, &meter, 0.4, 0.6);
                }
                if last.elapsed() < Duration::from_secs(10) || crate::progress::inline() {
                    continue;
                }
                last = std::time::Instant::now();
                let n = meter.done();
                let rate = meter.rate().unwrap_or(0.0);
                let eta = meter.eta().map_or_else(|| "unknown".into(), |e| format!("{:.0} s", e.as_secs_f64()));
                say(format!("{label}: {n}/{total} done ({:.1}%), {rate:.1}/s, {eta} left at this rate; {} translated, {} dxc reused, {} cached, {} unsupported, {} failed; {} running",
                    100.0 * n as f64 / total as f64, stats.prewarmed.load(Ordering::Relaxed), stats.reused.load(Ordering::Relaxed),
                    stats.cached.load(Ordering::Relaxed), stats.unsupported.load(Ordering::Relaxed), stats.failed.load(Ordering::Relaxed),
                    running.load(Ordering::Relaxed)));
            }
        });
        for _ in 0..workers {
            scope.spawn(|| {
                while std::time::Instant::now() < deadline
                    && !stop.is_some_and(|s| s.load(Ordering::Acquire))
                    && let Some((path, key, ext)) = jobs.get(next.fetch_add(1, Ordering::Relaxed))
                {
                    // In-process compiles abandoned past their time still run and hold
                    // memory (a4ba782): each one counts as a job until it ends.
                    while (!room_for_programs(running.load(Ordering::Acquire) + crate::dxc::abandoned())
                        || running.load(Ordering::Acquire) + crate::dxc::abandoned() >= workers)
                        && !stop.is_some_and(|s| s.load(Ordering::Acquire)) {
                        thread::sleep(Duration::from_millis(250));
                    }
                    running.fetch_add(1, Ordering::AcqRel);
                    let outcome = compile(path, key, ext, tools, cache, scratch, prewarm_o0(), Some(&tail));
                    note_times(&outcome);
                    running.fetch_sub(1, Ordering::AcqRel);
                    let counter = match outcome {
                        Outcome::Compiled(..) => &stats.prewarmed,
                        Outcome::Reused(_) => &stats.reused,
                        Outcome::Cached => &stats.cached,
                        Outcome::Unsupported(m) => { stats.note("unsupported", &m); &stats.unsupported }
                        Outcome::Failed(m) => {
                            // Every failure named by key, so any one can be replayed
                            // (the summary groups by reason only).
                            println!("[launch] Xbox shader {key}: {m}");
                            stats.note("failed", &m);
                            &stats.failed
                        }
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                    done.fetch_add(1, Ordering::Relaxed);
                    let _ = fs::remove_file(scratch.join(format!("{key}.cso")));
                }
                if workers_done.fetch_add(1, Ordering::AcqRel) + 1 == workers {
                    finished.store(true, Ordering::Release);
                }
            });
        }
    });
    if stop.is_none() {
        let mut meter = crate::progress::Meter::new(total);
        meter.sample(done.load(Ordering::Relaxed));
        crate::progress::report(label, &meter, 0.4, 0.6);
    }
    crate::progress::finish();
    let n = done.load(Ordering::Relaxed);
    if n < total {
        say(format!("{label}: stopped at NEONBRIDGE_PREWARM_MAX_S with {n}/{total} done in {:.0} s; the other {} are translated when the title asks for them",
            started.elapsed().as_secs_f64(), total - n));
    } else {
        say(format!("{label}: all {total} done in {:.0} s", started.elapsed().as_secs_f64()));
    }
    say(format!("{label}: {}", job_peaks()));
    say(format!("{label}: {}", job_times(workers)));
    let _ = fs::remove_dir_all(scratch);
}

/// Whether the prewarm skips dxc's -O1 pass (see compile). Opt-in,
/// NEONBRIDGE_PREWARM_DXC_O0=1: -O0 DXIL runs slower on the GPU and the
/// intro is GPU-bound, so it waits for an -O1 upgrade path.
fn prewarm_o0() -> bool {
    std::env::var("NEONBRIDGE_PREWARM_DXC_O0").as_deref() == Ok("1")
}

/// Parallel translations (xshader, then dxc) for the prewarm and for the
/// title's requests. The prewarm runs before the game starts, so it takes
/// every core but two (test143: 8 of 16 threads gave about 1.8 programs a
/// second, ten minutes for a changed translator). While the game runs the
/// title needs the cores, so its requests keep at most 8 as before.
/// NEONBRIDGE_SHADER_THREADS sets both.
fn worker_counts() -> (usize, usize) {
    if let Some(n) = std::env::var("NEONBRIDGE_SHADER_THREADS").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|&n| n > 0) {
        return (n, n);
    }
    let cores = thread::available_parallelism().map_or(4, |n| n.get());
    (cores.saturating_sub(2).max(2), cores.clamp(2, 8))
}

/// How long the prewarm may hold the launch, both passes together
/// (NEONBRIDGE_PREWARM_MAX_S, 600; 0 no limit). After an xshader change every
/// cached translation is stale and the full archive is ~80k programs: test142
/// sat 15 minutes in the prewarm with no output and the game not started.
fn prewarm_deadline() -> std::time::Instant {
    let secs = std::env::var("NEONBRIDGE_PREWARM_MAX_S").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(600);
    let secs = if secs == 0 { 10 * 365 * 24 * 3600 } else { secs };
    std::time::Instant::now() + Duration::from_secs(secs)
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
        let switches = xshader_switches();
        if !switches.is_empty() {
            log(format!("shader cache keyed by the xshader switches {}", switches.join(" ")));
        }
        let id = cache.file_name().unwrap().to_string_lossy().into_owned();
        fs::write(session_dir.join("compiler-id.tmp"), &id)
            .and_then(|_| fs::rename(session_dir.join("compiler-id.tmp"), session_dir.join("compiler-id")))
            .map_err(|e| e.to_string())?;
        // d3d12_xs counts this process (and its xshader and dxc) toward its compile ceiling.
        let _ = fs::write(session_dir.join("worker-pid"), std::process::id().to_string());
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats::default());
        let (prewarm_workers, workers) = worker_counts();
        log(format!("shader workers: {prewarm_workers} for the prewarm, {workers} while the game runs (NEONBRIDGE_SHADER_THREADS sets both)"));
        let archive = cache_root.join("requests");
        let _ = fs::create_dir_all(&archive);
        // test113-116 all said "0 prewarmed, N compiled, 0 from cache": runs
        // set TUXBOX_PREWARM=0 (the full archive is ~80k programs) and xshader
        // changed between runs, so each run started on an empty cache dir and
        // the world's pipelines waited on hundreds of compiles. The programs
        // the title asked for before (a few hundred) are translated first,
        // whatever TUXBOX_PREWARM says; with an unchanged toolchain they are
        // cache hits. NEONBRIDGE_PREWARM_SEEN=0 turns it off.
        let deadline = prewarm_deadline();
        let stale = stale_on();
        // Seen programs with a translation from an older build only: the game
        // starts on it and they are translated again in the background.
        let mut stale_seen = std::collections::HashSet::new();
        if stale {
            let seeded = seed_last(&cache_root, &cache);
            if seeded > 0 {
                log(format!("stale shaders: {seeded} last good translations kept from the build caches on disk"));
            }
        }
        if std::env::var("NEONBRIDGE_PREWARM_SEEN").as_deref() != Ok("0") {
            let started = std::time::Instant::now();
            let mut seen = seen_requests(&cache_root, &archive);
            if stale {
                stale_seen = seen.iter().filter(|n| valid_request(n).is_some_and(|(key, _)| {
                    !cache.join(format!("{key}.cso")).exists() && last_good(&cache, key).is_some()
                })).cloned().collect();
                seen.retain(|n| !stale_seen.contains(n));
                log(format!("seen prewarm: {} programs have only an older xshader's translation: the game starts on it and they are translated again while it runs (NEONBRIDGE_STALE_SHADERS=0 waits for them)",
                    stale_seen.len()));
            }
            prewarm(&tools, &cache, &archive, &private.join("prewarm-seen"), prewarm_workers, &stats, Some(&seen), "seen prewarm", deadline, &log, None);
            log(format!("seen prewarm: {} programs the title asked for before, {} translated, {} with dxc output reused ({} matched only without comments and spacing), {} already cached, in {:.1} s (NEONBRIDGE_PREWARM_SEEN=0 off)",
                seen.len(), stats.prewarmed.load(Ordering::Relaxed), stats.reused.load(Ordering::Relaxed), NORMALISED_REUSES.load(Ordering::Relaxed),
                stats.cached.load(Ordering::Relaxed), started.elapsed().as_secs_f64()));
        }
        // TUXBOX_PREWARM=0 serves the title's requests at once, without
        // building the archive first. With stale translations served it runs
        // in the background after the stale programs.
        let full = std::env::var("TUXBOX_PREWARM").as_deref() != Ok("0");
        if full && !stale {
            prewarm(&tools, &cache, &archive, &private.join("prewarm"), prewarm_workers, &stats, None, "full prewarm (TUXBOX_PREWARM=0 skips)", deadline, &log, None);
        }
        let log = Arc::new(log);
        // -O0 DXIL from the prewarm (and from before) is recompiled at -O1 while the game runs.
        if let Some(dxil) = tools.dxil.clone() {
            let log = log.clone();
            crate::dxil_upgrade::start(tools.dxc.clone(), tools.defines.clone(), dxil, move |line| log(line));
        }
        let tools = Arc::new(tools);
        let background = (stale && (full || !stale_seen.is_empty())).then(|| {
            let (tools, cache, archive, scratch, stats, log, stop) =
                (tools.clone(), cache.clone(), archive.clone(), private.join("prewarm-stale"), stats.clone(), log.clone(), stop.clone());
            // Half the game-time workers: the title's own requests keep the rest.
            let n = workers.div_ceil(2).max(1);
            thread::Builder::new().name("shader-stale".into()).spawn(move || {
                let forever = std::time::Instant::now() + Duration::from_secs(10 * 365 * 24 * 3600);
                if !stale_seen.is_empty() {
                    prewarm(&tools, &cache, &archive, &scratch, n, &stats, Some(&stale_seen), "stale retranslation", forever, &*log, Some(&stop));
                }
                if full {
                    prewarm(&tools, &cache, &archive, &scratch.with_file_name("prewarm"), n, &stats, None, "full prewarm in the background (TUXBOX_PREWARM=0 skips)", forever, &*log, Some(&stop));
                }
                log(stale_summary());
            }).ok()
        }).flatten();
        let (s, st) = (stop.clone(), stats.clone());
        let handle = thread::Builder::new().name("shader-worker".into()).spawn(move || {
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
                        // A request a title thread waits on is never held by the ceiling.
                        let urgent = first.contains(&key);
                        while active.load(Ordering::Acquire) >= workers
                            || (!urgent && !room_for_programs(active.load(Ordering::Acquire)))
                        {
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
                        if first.contains(&key) {
                            // test111 had no line showing whether urgent requests were served.
                            static SERVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                            let n = SERVED.fetch_add(1, Ordering::Relaxed) + 1;
                            if n <= 8 || n.is_power_of_two() {
                                log(format!("urgent Xbox shader {key}: a title thread waits on it, served ahead of the queue ({n} so far; NEONBRIDGE_URGENT_SHADERS=0 off)"));
                            }
                        }
                        active.fetch_add(1, Ordering::AcqRel);
                        archive_request(&path, &name, &archive);
                        let (tools, cache, session, st, log, busy, active) =
                            (tools.clone(), cache.clone(), session_dir.clone(), st.clone(), log.clone(), busy.clone(), active.clone());
                        thread::spawn(move || {
                            // A program translated by an older build only is
                            // answered with that at once, then translated:
                            // the fresh one serves the next request.
                            let served_stale = stale && !cache.join(format!("{key}.cso")).exists()
                                && last_good(&cache, &key).is_some_and(|bytes| {
                                    let tmp = session.join(format!("{key}.{}.stale", std::process::id()));
                                    fs::write(&tmp, &bytes).and_then(|_| fs::rename(&tmp, session.join(format!("{key}.cso")))).is_ok()
                                });
                            if served_stale {
                                let _ = fs::remove_file(session.join("urgent").join(&key));
                                let n = STALE_SERVED.fetch_add(1, Ordering::Relaxed) + 1;
                                if n <= 8 || n.is_power_of_two() {
                                    log(format!("Xbox shader {key}: served an older xshader's translation while the new one is made ({n} so far; NEONBRIDGE_STALE_SHADERS=0 waits)"));
                                }
                            }
                            let outcome = compile(&path, &key, &ext, &tools, &cache, &session, false, None);
                            match &outcome {
                                Outcome::Compiled(xs, dxc) => {
                                    // Measures whether dxc dominates (a direct DXIL writer would only save the dxc part).
                                    st.compiled.fetch_add(1, Ordering::Relaxed);
                                    log(format!("compiled Xbox shader {key} (xshader {xs} ms, dxc {dxc} ms)"));
                                }
                                Outcome::Reused(xs) => {
                                    let n = st.reused.fetch_add(1, Ordering::Relaxed) + 1;
                                    if n <= 8 || n.is_power_of_two() {
                                        log(format!("Xbox shader {key}: xshader {xs} ms, its HLSL compiled before: dxc skipped ({n} so far, {} of them matched only without comments and spacing; NEONBRIDGE_DXIL_REUSE=0 off)",
                                            NORMALISED_REUSES.load(Ordering::Relaxed)));
                                    }
                                }
                                Outcome::Cached => { st.cached.fetch_add(1, Ordering::Relaxed); }
                                Outcome::Unsupported(m) => { st.unsupported.fetch_add(1, Ordering::Relaxed); log(format!("Xbox shader {key}: unsupported: {m}")); }
                                Outcome::Failed(m) => { st.failed.fetch_add(1, Ordering::Relaxed); log(format!("Xbox shader {key}: {m}")); }
                            }
                            if matches!(outcome, Outcome::Unsupported(_) | Outcome::Failed(_)) {
                                // The title holds the older translation already.
                                if !served_stale {
                                    let _ = fs::write(session.join(format!("{key}.failed")), b"");
                                }
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
        Ok(Self { stop, _background: background, handle: Some(handle), stats, private })
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

