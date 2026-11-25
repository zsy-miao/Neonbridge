//! -O0 DXIL is a stopgap: it compiles in a fraction of -O1's time but runs
//! slower on the GPU. Every -O0 result (the prewarm's opt-in -O0, an urgent
//! request, an -O1 pass past its time) is stored with a `.o0` marker beside
//! each reuse path and queued here; a lowest-priority thread recompiles it at
//! -O1, during the run and at the next launch (the queue is on disk), and
//! writes the -O1 DXIL over the -O0 one, so the next pipeline built from it
//! (this run or the next) gets the optimised shader. Nothing writes -O0 over
//! a path without a marker. The refs' tiered caches do the same (xcl keeps
//! -O0 only as a size safety net; VIctory-fxceless serves, then rebuilds).
//! NEONBRIDGE_DXC_UPGRADE=0 leaves -O0 results as they are.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The marker beside a reuse path holding -O0 DXIL.
pub fn marker(path: &Path) -> PathBuf {
    path.with_extension("o0")
}

pub fn enabled() -> bool {
    crate::dxc::enabled() && std::env::var("NEONBRIDGE_DXC_UPGRADE").as_deref() != Ok("0")
}

fn queue(dxil: &Path) -> PathBuf {
    dxil.join("upgrade")
}

/// Queues `hlsl` for an -O1 compile under `target`; the result goes to each of
/// `paths` (with its `.sha256`), whose `.o0` markers it removes.
pub fn enqueue(dxil: &Path, id: &str, target: &str, hlsl: &[u8], paths: &[&Path]) {
    if !enabled() {
        return;
    }
    let dir = queue(dxil);
    let job = dir.join(format!("{id}.job"));
    if job.exists() || fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut text = format!("{target}\n");
    for p in paths {
        text.push_str(&p.to_string_lossy());
        text.push('\n');
    }
    // The source first: a job file names a complete source.
    let tmp = dir.join(format!("{id}.{}.tmp", std::process::id()));
    if fs::write(dir.join(format!("{id}.hlsl")), hlsl).and_then(|_| fs::write(&tmp, text)).and_then(|_| fs::rename(&tmp, &job)).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Starts the upgrade thread (one, lowest priority). It runs while the launcher does.
pub fn start(dxc: PathBuf, defines: Vec<String>, dxil: PathBuf, log: impl Fn(String) + Send + 'static) {
    if !enabled() {
        return;
    }
    let secs = std::env::var("NEONBRIDGE_DXC_UPGRADE_SECS").ok().and_then(|v| v.trim().parse().ok()).filter(|&s| s > 0).unwrap_or(300);
    let _ = std::thread::Builder::new().name("dxil-upgrade".into()).spawn(move || {
        let dir = queue(&dxil);
        let (mut done, mut failed) = (0usize, 0usize);
        loop {
            let jobs: Vec<PathBuf> = fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "job")).collect()).unwrap_or_default();
            if jobs.is_empty() {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            for job in jobs {
                match upgrade(&dxc, &defines, &job, secs) {
                    Ok(()) => done += 1,
                    Err(e) => {
                        failed += 1;
                        log(format!("dxil upgrade {}: {e}; the -O0 DXIL stays", job.file_stem().unwrap_or_default().to_string_lossy()));
                    }
                }
                let _ = fs::remove_file(&job);
                let _ = fs::remove_file(job.with_extension("hlsl"));
                let n = done + failed;
                if n.is_power_of_two() || n % 100 == 0 {
                    log(format!("dxil upgrade: {done} -O0 programs recompiled at -O1, {failed} failed (NEONBRIDGE_DXC_UPGRADE=0 off)"));
                }
            }
        }
    });
}

fn upgrade(dxc: &Path, defines: &[String], job: &Path, secs: u64) -> Result<(), String> {
    let text = fs::read_to_string(job).map_err(|e| e.to_string())?;
    let mut lines = text.lines();
    let target = lines.next().ok_or("empty job")?.to_owned();
    let paths: Vec<PathBuf> = lines.filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    let hlsl = fs::read(job.with_extension("hlsl")).map_err(|e| e.to_string())?;
    let mut args = vec!["-T".to_owned(), target.clone(), "-O1".to_owned()];
    for d in defines {
        args.extend(["-D".to_owned(), d.clone()]);
    }
    if target != "lib_6_3" {
        args.extend(["-E".to_owned(), "main".to_owned()]);
    }
    let bytes = match crate::dxc::compile(dxc, hlsl, args, Duration::from_secs(secs), &|| false, true) {
        Ok(b) => b,
        Err(crate::dxc::Error::Compiler(m) | crate::dxc::Error::Other(m)) => return Err(m.chars().take(300).collect()),
    };
    if !crate::shaders::has_dxil_container(&bytes) {
        return Err("invalid DXIL container".into());
    }
    let sum = crate::shaders::sha256_hex(&bytes);
    for path in &paths {
        // Only over -O0 output (or nothing): an -O1 result is never touched.
        if path.exists() && !marker(path).exists() {
            continue;
        }
        let tmp = path.with_extension(format!("{}.up", std::process::id()));
        if fs::write(&tmp, &bytes).and_then(|_| fs::write(path.with_extension("sha256"), &sum)).and_then(|_| fs::rename(&tmp, path)).is_ok() {
            let _ = fs::remove_file(marker(path));
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }
    Ok(())
}
