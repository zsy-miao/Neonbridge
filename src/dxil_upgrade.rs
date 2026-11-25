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
    std::env::var("NEONBRIDGE_DXC_UPGRADE").as_deref() != Ok("0")
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
    remember(&dxc, &defines, &dxil);
    let secs = std::env::var("NEONBRIDGE_DXC_UPGRADE_SECS").ok().and_then(|v| v.trim().parse().ok()).filter(|&s| s > 0).unwrap_or(900);
    let _ = std::thread::Builder::new().name("dxil-upgrade".into()).spawn(move || {
        let dir = queue(&dxil);
        let (mut done, mut failed) = (0usize, 0usize);
        loop {
            let mut jobs: Vec<PathBuf> = fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "job")).collect()).unwrap_or_default();
            // Programs the title asked for before (<cache_root>/seen, the intro's
            // among them) first: their -O0 DXIL is what the next run draws with.
            let seen = dxil.parent().and_then(Path::parent).map(|root| root.join("seen"));
            jobs.sort_by_key(|job| !seen.as_ref().is_some_and(|seen| asked_before(job, seen)));
            // Not while the title runs (main.rs sets GAME_PID): the upgrade's
            // dxc competes with the game for CPU. NEONBRIDGE_DXC_UPGRADE_IN_GAME=1 runs it anyway.
            if game_running() {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            if jobs.is_empty() {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            for job in jobs {
                if game_running() {
                    break;
                }
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
    let source = job.with_extension("hlsl");
    let mut args = vec!["-T".to_owned(), target.clone(), "-O1".to_owned()];
    for d in defines {
        args.extend(["-D".to_owned(), d.clone()]);
    }
    if target != "lib_6_3" {
        args.extend(["-E".to_owned(), "main".to_owned()]);
    }
    if crate::shaders::denorm_ftz_marked(&source) {
        args.extend(["-denorm".to_owned(), "ftz".to_owned()]);
    }
    let bytes = if crate::dxc::enabled() {
        let hlsl = fs::read(&source).map_err(|e| e.to_string())?;
        match crate::dxc::compile(dxc, hlsl, args, Duration::from_secs(secs), &yield_now, true) {
            Ok(b) => b,
            Err(_) if yield_now() => return Err(YIELDED.into()),
            Err(crate::dxc::Error::Compiler(m) | crate::dxc::Error::Other(m)) => return Err(m.chars().take(300).collect()),
        }
    } else {
        // dxc.exe in the idle priority class: the game and the prewarm's own
        // compiles go first. Only NEONBRIDGE_DXC_UPGRADE_SECS ends it, not the
        // 10 s -O1 cut-off of a compile someone waits on.
        use std::os::windows::process::CommandExt;
        const IDLE_PRIORITY_CLASS: u32 = 0x40;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = job.with_extension("cso");
        let mut child = std::process::Command::new(dxc).args(&args).arg("-Fo").arg(&out).arg(&source)
            .creation_flags(IDLE_PRIORITY_CLASS | CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().map_err(|e| format!("dxc: {e}"))?;
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) => break status,
                None if yield_now() => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(YIELDED.into());
                }
                None if start.elapsed() > Duration::from_secs(secs) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("dxc -O1 past {secs} s (NEONBRIDGE_DXC_UPGRADE_SECS)"));
                }
                None => std::thread::sleep(Duration::from_millis(250)),
            }
        };
        let bytes = fs::read(&out);
        let _ = fs::remove_file(&out);
        if !status.success() {
            return Err(format!("dxc -O1 failed ({status})"));
        }
        bytes.map_err(|e| format!("dxc wrote no output: {e}"))?
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

/// Whether a job writes the build-cache entry of a program in `seen` (its
/// `<key>.cso` path, the request `<key>.dxbc` or `.link`).
fn asked_before(job: &Path, seen: &Path) -> bool {
    let Ok(text) = fs::read_to_string(job) else { return false };
    text.lines().skip(1).filter_map(|l| Path::new(l).file_stem()?.to_str().map(str::to_owned))
        .any(|key| seen.join(format!("{key}.dxbc")).exists() || seen.join(format!("{key}.link")).exists())
}

/// Whether the title started by this launcher is still running.
fn game_running() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if std::env::var("NEONBRIDGE_DXC_UPGRADE_IN_GAME").as_deref() == Ok("1") {
        return false;
    }
    let pid = crate::shaders::GAME_PID.load(std::sync::atomic::Ordering::Relaxed);
    if pid == 0 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(h);
        alive
    }
}

// Draining between runs. The in-launcher thread stops with the launcher, so
// the -O0 DXIL a run or prewarm left stayed -O0 (test156: 873 programs drew
// with it, each pipeline a long nvgpucomp64 compile). A launcher that exits
// with jobs queued starts `neonbridge --drain-upgrades`, detached, which works
// the queue at idle priority until it is empty, stepping aside (and killing
// its dxc) while any other neonbridge.exe runs: that one's own thread, or its
// game, has the machine. One drainer at a time (upgrade/drain.pid); its log
// is upgrade/drain.log. NEONBRIDGE_DXC_UPGRADE_DRAIN=0 off.

const DRAIN_SUFFIX: &str = "-drain.exe";
const YIELDED: &str = "stepped aside for a launcher";
static DRAINER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CONFIG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn drain_enabled() -> bool {
    enabled() && std::env::var("NEONBRIDGE_DXC_UPGRADE_DRAIN").as_deref() != Ok("0")
}

/// Programs still at -O0 waiting in the queue.
pub fn left(dxil: &Path) -> usize {
    fs::read_dir(queue(dxil)).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|e| e == "job")).count()).unwrap_or(0)
}

/// Writes what a drainer needs (dxc, defines) beside the queue.
fn remember(dxc: &Path, defines: &[String], dxil: &Path) {
    let dir = queue(dxil);
    let cfg = dir.join("drain.cfg");
    let mut text = format!("{}\n", dxc.display());
    for d in defines {
        text.push_str(d);
        text.push('\n');
    }
    if fs::create_dir_all(&dir).and_then(|_| fs::write(&cfg, text)).is_ok() {
        let _ = CONFIG.set(cfg);
    }
}

/// In the drainer: whether another launcher runs (checked every 2 s at most).
fn yield_now() -> bool {
    use std::sync::atomic::Ordering;
    use std::time::Instant;
    static LAST: std::sync::Mutex<Option<(Instant, bool)>> = std::sync::Mutex::new(None);
    if !DRAINER.load(Ordering::Relaxed) {
        return false;
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, answer)) = *last
        && at.elapsed() < Duration::from_secs(2)
    {
        return answer;
    }
    let answer = other_launcher();
    *last = Some((Instant::now(), answer));
    answer
}

fn other_launcher() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let me = std::process::id();
    // The drainer runs from its own copy (neonbridge-drain.exe): it yields to the launcher's name.
    let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .map(|n| n.replace(DRAIN_SUFFIX, ""))
        .unwrap_or_else(|| "neonbridge.exe".into());
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return true;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(260)]);
            // The game too: a run started without this launcher (or one whose
            // launcher already exited) still needs the machine. NEONBRIDGE_DRAIN_YIELD_GAME=0 off.
            let game = std::env::var("NEONBRIDGE_DRAIN_YIELD_GAME").as_deref() != Ok("0")
                && n.to_ascii_lowercase().starts_with("game_scarlett");
            if e.th32ProcessID != me && (n.eq_ignore_ascii_case(&exe) || game) {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}

fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(h);
        alive
    }
}

fn drainer_running(dir: &Path) -> bool {
    fs::read_to_string(dir.join("drain.pid")).ok().and_then(|p| p.trim().parse::<u32>().ok())
        .is_some_and(|p| p != std::process::id() && alive(p))
}

/// At a launcher's exit: starts the detached drainer when jobs are queued.
pub fn spawn_drainer() {
    let Some(cfg) = CONFIG.get() else { return };
    let Some(dir) = cfg.parent() else { return };
    let Some(dxil) = dir.parent() else { return };
    if !drain_enabled() || DRAINER.load(std::sync::atomic::Ordering::Relaxed) || left(dxil) == 0 || drainer_running(dir) {
        return;
    }
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const IDLE_PRIORITY_CLASS: u32 = 0x40;
    let Ok(exe) = std::env::current_exe() else { return };
    // Its own copy beside the queue: a build's `taskkill /IM neonbridge.exe`
    // (needed while the exe is locked) ended the drainer mid-queue, and a
    // drainer running the launcher's exe locked it against the next build.
    // NEONBRIDGE_DRAIN_COPY=0 runs the launcher's exe as before.
    let exe = if std::env::var("NEONBRIDGE_DRAIN_COPY").as_deref() == Ok("0") { exe } else {
        let stem = exe.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "neonbridge".into());
        let copy = dir.join(format!("{stem}{DRAIN_SUFFIX}"));
        match fs::copy(&exe, &copy) {
            Ok(_) => copy,
            Err(e) => {
                println!("[launch] dxil upgrade: could not copy the drainer to {} ({e}); it runs the launcher's exe", copy.display());
                exe
            }
        }
    };
    let spawn = |flags| std::process::Command::new(&exe).arg("--drain-upgrades").arg(dxil)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .creation_flags(flags).spawn();
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | IDLE_PRIORITY_CLASS;
    // Out of the launcher's job object when it allows, so closing it does not end the drain.
    let (child, broke_away) = match spawn(base | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(c) => (Ok(c), true),
        Err(_) => (spawn(base), false),
    };
    if let Ok(child) = child {
        if !broke_away {
            // test164: drainer 38264 vanished mid-queue (stale drain.pid, no
            // "drained" line): a job that refuses breakaway ends it with its owner.
            println!("[launch] dxil upgrade: this launcher's job does not allow breakaway; the drainer ends when that job closes (the next launcher's exit restarts it)");
        }
        println!("[launch] dxil upgrade: {} -O0 programs queued; draining them at -O1 in the background (pid {}, NEONBRIDGE_DXC_UPGRADE_DRAIN=0 off)",
            left(dxil), child.id());
    }
}

/// `neonbridge --drain-upgrades DXIL`: works the queue until it is empty.
pub fn drain(dxil: &Path) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let dir = queue(dxil);
    let text = fs::read_to_string(dir.join("drain.cfg")).map_err(|e| format!("drain.cfg: {e}"))?;
    let mut lines = text.lines();
    let dxc = PathBuf::from(lines.next().ok_or("drain.cfg: no dxc")?);
    let defines: Vec<String> = lines.filter(|l| !l.is_empty()).map(str::to_owned).collect();
    if drainer_running(&dir) {
        return Ok(());
    }
    let pid_file = dir.join("drain.pid");
    let _ = fs::write(&pid_file, std::process::id().to_string());
    DRAINER.store(true, Ordering::Relaxed);
    let mut stepped_aside = false;
    let secs = std::env::var("NEONBRIDGE_DXC_UPGRADE_SECS").ok().and_then(|v| v.trim().parse().ok()).filter(|&s| s > 0).unwrap_or(900);
    let log_path = dir.join("drain.log");
    let log = |line: String| {
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(f, "{line}");
        }
    };
    let (mut done, mut failed) = (0usize, 0usize);
    log(format!("drainer {} started: {} jobs", std::process::id(), left(dxil)));
    loop {
        let aside = yield_now();
        if aside != stepped_aside {
            stepped_aside = aside;
            log(format!("{} ({done} done, {} left)", if aside { "stepped aside: a launcher or the game runs" } else { "resumed: no launcher or game" }, left(dxil)));
        }
        if aside {
            std::thread::sleep(Duration::from_secs(10));
            continue;
        }
        let jobs: Vec<PathBuf> = fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "job")).collect()).unwrap_or_default();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            if yield_now() {
                break;
            }
            match upgrade(&dxc, &defines, &job, secs) {
                // The job stays queued for whoever runs next.
                Err(e) if e == YIELDED => break,
                Ok(()) => done += 1,
                Err(e) => {
                    failed += 1;
                    log(format!("{}: {e}; the -O0 DXIL stays", job.file_stem().unwrap_or_default().to_string_lossy()));
                }
            }
            let _ = fs::remove_file(&job);
            let _ = fs::remove_file(job.with_extension("hlsl"));
        }
    }
    log(format!("drained: {done} -O0 programs recompiled at -O1, {failed} failed"));
    let _ = fs::remove_file(&pid_file);
    Ok(())
}
