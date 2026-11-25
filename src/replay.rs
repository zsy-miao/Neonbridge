//! `neonbridge replay-shaders <capture dir> [--xshader <exe>] [--against <capture dir>]`:
//! translate every shader a captured frame bound (TUXBOX_CAPTURE_FRAME's
//! pipelines.jsonl), offline, with xshader and dxc, and diff the outcomes
//! against the previous replay of that capture (or `--against` another).
//! Translator fixes iterate in seconds; a run only confirms them.
//!
//! Each shader's DXBC comes from the shader cache's request archive
//! (%TEMP%\tuxbox_shader_cache\requests\<sha256>.dxbc) or its refusal corpus
//! (runtime\refused). Results go to <capture>/replay/: <key>.hlsl and
//! replay.jsonl with one line per shader (outcome, reason, HLSL digest, the
//! frame's ran/dropped counts of the pipelines using it). The previous
//! replay.jsonl is kept as replay.prev.jsonl. NEONBRIDGE_REPLAY_DXC=0 stops
//! after xshader; NEONBRIDGE_REPLAY_JOBS sets the worker count.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::{fs, thread};

const XSHADER_UNSUPPORTED_HLSL: i32 = 65;

#[derive(Default, Clone)]
struct Uses {
    pipelines: usize,
    ran: u64,
    dropped: u64,
}

struct Result_ {
    key: String,
    outcome: &'static str,
    reason: String,
    hlsl: String,
    uses: Uses,
}

/// The string value after `"name":` in a flat JSON line written by capture.rs.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let at = line.find(&format!("\"{name}\":"))? + name.len() + 3;
    let rest = &line[at..];
    if let Some(rest) = rest.strip_prefix('"') {
        let mut end = 0;
        let bytes = rest.as_bytes();
        while end < bytes.len() && bytes[end] != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        rest.get(..end)
    } else {
        rest.split([',', '}']).next()
    }
}

fn shaders_of(line: &str) -> Vec<String> {
    let Some(at) = line.find("\"shaders\":[") else { return Vec::new() };
    let rest = &line[at + 11..];
    let list = rest.split(']').next().unwrap_or("");
    list.split(',').map(|s| s.trim().trim_matches('"').to_owned()).filter(|s| s.len() == 64).collect()
}

fn unescape(s: &str) -> String {
    s.replace("\\\"", "\"").replace("\\n", "\n").replace("\\\\", "\\")
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn run(cmd: &mut Command, timeout: Duration) -> Result<(), (Option<i32>, String)> {
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| (None, e.to_string()))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut s, &mut err);
                }
                let tail: String = err.trim().chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
                return Err((status.code(), tail));
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                return Err((None, "timed out".into()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err((None, e.to_string())),
        }
    }
}

fn find_dxbc(cache_root: &Path, key: &str) -> Option<PathBuf> {
    let name = format!("{key}.dxbc");
    [cache_root.join("requests").join(&name), cache_root.join("runtime").join("refused").join(&name)]
        .into_iter()
        .find(|p| p.is_file())
}

fn translate(key: &str, dxbc: &Path, out: &Path, xshader: &Path, dxc: Option<&Path>) -> (&'static str, String, String) {
    let hlsl = out.join(format!("{key}.hlsl"));
    let profile = out.join(format!("{key}.profile"));
    let _ = fs::remove_file(&hlsl);
    let translated = run(Command::new(xshader).arg(dxbc).arg("--hlsl-out").arg(&hlsl).arg("--profile-out").arg(&profile),
        Duration::from_secs(120));
    let digest = fs::read(&hlsl).map(|b| format!("{:x}", Sha256::digest(&b))[..16].to_owned()).unwrap_or_default();
    match translated {
        Ok(()) => {}
        Err((Some(XSHADER_UNSUPPORTED_HLSL), msg)) => return ("unsupported", msg, digest),
        Err((code, msg)) => return ("xshader failed", format!("{code:?}: {msg}"), digest),
    }
    let Some(dxc) = dxc else { return ("translated", String::new(), digest) };
    let target = fs::read_to_string(&profile).unwrap_or_default().trim().to_owned();
    let _ = fs::remove_file(&profile);
    let object = out.join(format!("{key}.cso"));
    let mut cmd = Command::new(dxc);
    cmd.arg("-T").arg(&target).arg("-O0");
    if target != "lib_6_3" {
        cmd.args(["-E", "main"]);
    }
    if crate::shaders::denorm_ftz_marked(&hlsl) {
        cmd.args(["-denorm", "ftz"]);
    }
    let result = run(cmd.arg("-Fo").arg(&object).arg(&hlsl), Duration::from_secs(120));
    let _ = fs::remove_file(&object);
    match result {
        Ok(()) => ("compiled", String::new(), digest),
        Err((code, msg)) => ("dxc failed", format!("{code:?}: {msg}"), digest),
    }
}

fn read_previous(path: &Path) -> BTreeMap<String, (String, String)> {
    let Ok(text) = fs::read_to_string(path) else { return BTreeMap::new() };
    text.lines()
        .filter_map(|l| Some((field(l, "shader")?.to_owned(), (field(l, "outcome")?.to_owned(), field(l, "hlsl").unwrap_or("").to_owned()))))
        .collect()
}

pub fn run_cli(args: &[OsString], default_xshader: impl FnOnce() -> Result<PathBuf, Box<dyn Error>>) -> Result<(), Box<dyn Error>> {
    let usage = "usage: neonbridge replay-shaders <capture dir> [--xshader <exe>] [--against <capture dir>]";
    let (mut capture, mut xshader, mut against) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--xshader") => xshader = it.next().map(PathBuf::from),
            Some("--against") => against = it.next().map(PathBuf::from),
            Some("-h" | "--help") => { println!("{usage}"); return Ok(()); }
            _ if capture.is_none() => capture = Some(PathBuf::from(a)),
            _ => return Err(format!("replay-shaders: unknown argument {}\n{usage}", a.to_string_lossy()).into()),
        }
    }
    let capture = capture.ok_or(usage)?;
    let pipelines = fs::read_to_string(capture.join("pipelines.jsonl"))
        .map_err(|e| format!("{}: {e} (a TUXBOX_CAPTURE_FRAME folder holds pipelines.jsonl)", capture.join("pipelines.jsonl").display()))?;
    let mut uses: BTreeMap<String, Uses> = BTreeMap::new();
    let mut unnamed = 0usize;
    for line in pipelines.lines() {
        let shaders = shaders_of(line);
        if shaders.is_empty() {
            unnamed += 1;
        }
        let ran = field(line, "ran").and_then(|v| v.parse().ok()).unwrap_or(0u64);
        let dropped = field(line, "dropped").and_then(|v| v.parse().ok()).unwrap_or(0u64);
        for key in shaders {
            let u = uses.entry(key).or_default();
            u.pipelines += 1;
            u.ran += ran;
            u.dropped += dropped;
        }
    }
    if unnamed != 0 {
        println!("[replay] {unnamed} pipelines name no shaders (run with TUXBOX_CAPTURE_FRAME set at startup or TUXBOX_CAPTURE_SHADERS=1)");
    }
    let xshader = match xshader.or_else(|| std::env::var_os("XSHADER").map(PathBuf::from)) {
        Some(p) => p,
        None => default_xshader()?,
    };
    let dxc = if std::env::var("NEONBRIDGE_REPLAY_DXC").as_deref() == Ok("0") { None } else {
        Some(crate::shaders::find_dxc().ok_or("no Windows SDK dxc.exe with dxil.dll (or set DXC, or NEONBRIDGE_REPLAY_DXC=0)")?)
    };
    let cache_root = std::env::temp_dir().join("tuxbox_shader_cache");
    let out = capture.join("replay");
    fs::create_dir_all(&out)?;
    let report = out.join("replay.jsonl");
    let previous_path = match &against {
        Some(other) => other.join("replay").join("replay.jsonl"),
        None => {
            let prev = out.join("replay.prev.jsonl");
            if report.is_file() {
                fs::rename(&report, &prev)?;
            }
            prev
        }
    };
    let previous = read_previous(&previous_path);
    println!("[replay] {} shaders from {}; xshader {}{}", uses.len(), capture.display(), xshader.display(),
        dxc.as_ref().map_or(" (no dxc)".into(), |d| format!(", dxc {}", d.display())));
    let started = Instant::now();
    let jobs = std::env::var("NEONBRIDGE_REPLAY_JOBS").ok().and_then(|v| v.parse().ok())
        .unwrap_or_else(|| thread::available_parallelism().map_or(4, |n| n.get())).max(1);
    let queue = Mutex::new(uses.into_iter().collect::<Vec<_>>());
    let results = Mutex::new(Vec::new());
    thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let Some((key, uses)) = queue.lock().unwrap().pop() else { break };
                let (outcome, reason, hlsl) = match find_dxbc(&cache_root, &key) {
                    Some(dxbc) => translate(&key, &dxbc, &out, &xshader, dxc.as_deref()),
                    None => ("no dxbc", "not in the request archive or refusal corpus".into(), String::new()),
                };
                results.lock().unwrap().push(Result_ { key, outcome, reason, hlsl, uses });
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    // Shaders whose pipelines dropped the most draws first.
    results.sort_by(|a, b| b.uses.dropped.cmp(&a.uses.dropped).then_with(|| a.key.cmp(&b.key)));
    let mut text = String::new();
    let mut totals: BTreeMap<&str, usize> = BTreeMap::new();
    let mut changed = Vec::new();
    for r in &results {
        *totals.entry(r.outcome).or_default() += 1;
        text.push_str(&format!(
            "{{\"shader\":\"{}\",\"outcome\":\"{}\",\"reason\":\"{}\",\"hlsl\":\"{}\",\"pipelines\":{},\"ran\":{},\"dropped\":{}}}\n",
            r.key, r.outcome, escape(&r.reason), r.hlsl, r.uses.pipelines, r.uses.ran, r.uses.dropped));
        match previous.get(&r.key) {
            Some((outcome, _)) if outcome != r.outcome => changed.push(format!("  {}: {} -> {}", &r.key[..16], outcome, r.outcome)),
            Some((_, hlsl)) if !hlsl.is_empty() && *hlsl != r.hlsl => changed.push(format!("  {}: HLSL changed ({})", &r.key[..16], r.outcome)),
            None if !previous.is_empty() => changed.push(format!("  {}: new ({})", &r.key[..16], r.outcome)),
            _ => {}
        }
    }
    fs::write(&report, text)?;
    let totals = totals.iter().map(|(k, v)| format!("{v} {k}")).collect::<Vec<_>>().join(", ");
    println!("[replay] {totals} in {:.1} s; {}", started.elapsed().as_secs_f64(), report.display());
    for r in results.iter().filter(|r| !matches!(r.outcome, "compiled" | "translated")).take(20) {
        println!("  {} {} (pipelines {}, dropped {}): {}", &r.key[..16], r.outcome, r.uses.pipelines, r.uses.dropped,
            unescape(&r.reason).lines().last().unwrap_or(""));
    }
    if previous.is_empty() {
        println!("[replay] no earlier replay to diff against");
    } else if changed.is_empty() {
        println!("[replay] no change against {}", previous_path.display());
    } else {
        println!("[replay] {} changed against {}:", changed.len(), previous_path.display());
        for line in &changed {
            println!("{line}");
        }
    }
    Ok(())
}
