//! The shader build, run on its own thread and reported phase by phase.
//!
//! World engine: `victory warm` translates every shader into the cache and makes the compute
//! pipelines (about 3 min); when the runtime uses them (`engine::PRECOMPILED_GRAPHICS`),
//! `victory precompile-graphics` then makes the title's graphics pipelines in chunks of
//! `engine::CHUNK` records, each chunk saved when it ends.
//! Classic engine: `xcl shaders <pack> --precompile --pipelines --only cs`, which already skips
//! what is in its cache.
//!
//! Resumable: finished phases and chunks are written to a state file in the cache, keyed by the
//! engine's hash, and are not run again. With "skip slow shaders", a step that prints nothing
//! for `slow_secs` is stopped and its shaders are left to compile on demand in game.
use crate::{engine, flow, LogKind, Message};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the UI shows while the build runs.
#[derive(Clone, Debug)]
pub struct Progress {
    pub phase: u8,
    pub phases: u8,
    pub name: &'static str,
    pub done: u64,
    pub total: u64,
    /// What is being worked on now, and since when.
    pub current: String,
    pub current_since: Instant,
    pub last_output: Instant,
    /// 0..1 over all phases.
    pub overall: f32,
    /// Shaders or records left to compile on demand in game.
    pub on_demand: u64,
    pub started: Instant,
    pub jobs: usize,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
pub struct State {
    pub hash: String,
    /// World: `victory warm` finished.
    #[serde(default)]
    pub warm: bool,
    /// World: the first graphics record not done yet.
    #[serde(default)]
    pub next: u64,
    /// World: graphics records in the title's cache, once a chunk came back short.
    #[serde(default)]
    pub total: Option<u64>,
    /// World: chunks (first record) stopped as slow; Classic: one entry, the count left.
    #[serde(default)]
    pub on_demand: Vec<u64>,
}

impl State {
    pub fn load(path: &Path, hash: &str) -> State {
        let s: State = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if s.hash == hash {
            s
        } else {
            State { hash: hash.to_string(), ..Default::default() }
        }
    }

    fn save(&self, path: &Path) {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(j) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, j);
        }
    }
}

pub fn state_path(cache_dir: &str, world: bool) -> PathBuf {
    Path::new(cache_dir.trim()).join(if world { "build-world.json" } else { "build-classic.json" })
}

pub struct Job {
    pub world: bool,
    pub exe: PathBuf,
    pub runtime: PathBuf,
    pub game: PathBuf,
    pub cache_dir: String,
    pub hash: String,
    /// The World engine's environment: the selected preset's, so translated shaders land in the
    /// cache the play runs use, under the same switches.
    pub env: Vec<(&'static str, String)>,
    /// Some(N): stop a step that prints nothing for N seconds; its shaders compile in game.
    pub slow_secs: Option<u64>,
    pub stop: Arc<AtomicBool>,
    pub tx: Sender<Message>,
}

enum Outcome {
    Exit(Option<i32>),
    Stopped,
    Idle,
}

pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// Free physical memory in MB.
#[cfg(windows)]
pub fn free_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a correctly sized, zeroed MEMORYSTATUSEX.
    (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then(|| m.ullAvailPhys / (1024 * 1024))
}

#[cfg(not(windows))]
pub fn free_mb() -> Option<u64> {
    None
}

fn log(tx: &Sender<Message>, s: impl Into<String>) {
    let _ = tx.send(Message::Log(s.into(), LogKind::Info));
}

/// Runs one engine process, streaming its lines to the log and to `on_line`.
fn run(mut cmd: Command, job: &Job, idle: Option<Duration>, mut on_line: impl FnMut(&str)) -> Result<Outcome, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW (output is piped to the Logs page), BELOW_NORMAL_PRIORITY_CLASS
        cmd.creation_flags(0x0800_0000 | 0x0000_4000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", job.exe.display()))?;
    let (ltx, lrx) = channel::<String>();
    for s in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)]
        .into_iter()
        .flatten()
    {
        let ltx = ltx.clone();
        std::thread::spawn(move || crate::split_lines(s, |l| {
            let _ = ltx.send(l);
        }));
    }
    drop(ltx);
    let mut last = Instant::now();
    let mut exited: Option<Option<i32>> = None;
    loop {
        match lrx.recv_timeout(Duration::from_millis(250)) {
            Ok(l) => {
                last = Instant::now();
                on_line(&l);
                let _ = job.tx.send(Message::Log(l, LogKind::Out));
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(code) = exited {
                    return Ok(Outcome::Exit(code));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        on_line("");
        if job.stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Outcome::Stopped);
        }
        if idle.is_some_and(|d| last.elapsed() >= d) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Outcome::Idle);
        }
        if exited.is_none() {
            if let Ok(Some(s)) = child.try_wait() {
                exited = Some(s.code());
            }
        }
    }
}

/// Runs the whole build; the result is a one-line summary or the reason it did not finish.
pub fn build(job: Job) {
    let r = if job.world { world(&job) } else { classic(&job) };
    let _ = job.tx.send(Message::BuildEnded(r));
}

fn send(job: &Job, p: &Progress) {
    let _ = job.tx.send(Message::Build(p.clone()));
}

fn world(job: &Job) -> Result<String, String> {
    let path = state_path(&job.cache_dir, true);
    let mut st = State::load(&path, &job.hash);
    let phases = if engine::PRECOMPILED_GRAPHICS { 2 } else { 1 };
    let warm_share = if phases == 2 { 0.3 } else { 1.0 };
    let now = Instant::now();
    let mut p = Progress {
        phase: 1,
        phases,
        name: "Translating shaders",
        done: 0,
        total: engine::WARM_TOTAL,
        current: String::new(),
        current_since: now,
        last_output: now,
        overall: 0.,
        on_demand: st.on_demand.len() as u64 * engine::CHUNK,
        started: now,
        jobs: 1,
    };
    let base = |c: &mut Command| {
        c.current_dir(&job.runtime);
        for (k, v) in &job.env {
            c.env(k, v);
        }
    };
    if st.warm {
        log(&job.tx, "Shaders already translated for this engine build: skipping `victory warm`.");
    } else {
        p.jobs = engine::jobs(cores(), free_mb(), 512);
        let mut cmd = Command::new(&job.exe);
        base(&mut cmd);
        cmd.args(engine::warm_args(&job.game, p.jobs));
        log(&job.tx, format!("> victory.exe warm \"{}\" --jobs {}  (phase 1 of {phases}: translate every shader, about 3 min; already translated ones are read from the cache)", job.game.display(), p.jobs));
        p.current = "translating (the engine reports every 2,000 shaders)".into();
        send(job, &p);
        let mut tick = Instant::now();
        let out = run(cmd, job, None, |l| {
            if let Some(n) = engine::parse_warm(l) {
                p.done = n.min(p.total);
                p.current = format!("translating shaders {}–{}", n, (n + 2000).min(p.total));
                p.current_since = Instant::now();
            }
            if let Some((t, r)) = engine::parse_warm_end(l) {
                p.done = t + r;
                p.total = p.total.max(p.done);
            }
            if !l.is_empty() {
                p.last_output = Instant::now();
            }
            p.overall = warm_share * p.done as f32 / p.total.max(1) as f32;
            if !l.is_empty() || tick.elapsed() >= Duration::from_secs(1) {
                tick = Instant::now();
                send(job, &p);
            }
        })?;
        match out {
            Outcome::Exit(Some(0)) => {
                st.warm = true;
                st.save(&path);
            }
            Outcome::Exit(c) => return Err(format!("victory warm failed (exit code {c:?}); see Logs")),
            Outcome::Stopped | Outcome::Idle => return Err("Stopped. Press Build shaders again to resume.".into()),
        }
    }
    if phases == 2 {
        p.phase = 2;
        p.name = "Building graphics pipelines";
        p.total = st.total.unwrap_or(engine::GRAPHICS_TOTAL);
        p.jobs = engine::jobs(cores(), free_mb(), 1024);
        let idle = job.slow_secs.map(Duration::from_secs);
        while st.total.is_none_or(|t| st.next < t) {
            let skip = st.next;
            let mut cmd = Command::new(&job.exe);
            base(&mut cmd);
            cmd.args(engine::graphics_args(&job.game, p.jobs, skip, engine::CHUNK));
            log(&job.tx, format!("> victory.exe precompile-graphics --jobs {} --skip {skip} --limit {}  (phase 2 of 2)", p.jobs, engine::CHUNK));
            p.current = format!("pipeline records {}–{}", skip + 1, skip + engine::CHUNK);
            p.current_since = Instant::now();
            p.done = skip;
            send(job, &p);
            let mut records = None;
            let mut tick = Instant::now();
            let out = run(cmd, job, idle, |l| {
                if let Some(n) = engine::parse_graphics(l) {
                    p.done = skip + n;
                }
                if let Some(n) = engine::parse_graphics_end(l) {
                    records = Some(n);
                }
                if !l.is_empty() {
                    p.last_output = Instant::now();
                }
                p.overall = warm_share + (1. - warm_share) * p.done as f32 / p.total.max(1) as f32;
                if !l.is_empty() || tick.elapsed() >= Duration::from_secs(1) {
                    tick = Instant::now();
                    send(job, &p);
                }
            })?;
            match out {
                Outcome::Exit(Some(0)) => {
                    let n = records.unwrap_or(engine::CHUNK);
                    if n < engine::CHUNK {
                        st.total = Some(skip + n);
                        p.total = skip + n;
                    }
                    st.next = skip + n.max(1).min(engine::CHUNK);
                }
                Outcome::Idle => {
                    log(&job.tx, format!("Records {}–{} printed nothing for {} s: left to compile in game.", skip + 1, skip + engine::CHUNK, job.slow_secs.unwrap_or(0)));
                    st.on_demand.push(skip);
                    st.next = skip + engine::CHUNK;
                    p.on_demand += engine::CHUNK;
                }
                Outcome::Exit(c) => return Err(format!("victory precompile-graphics failed (exit code {c:?}); see Logs")),
                Outcome::Stopped => {
                    st.save(&path);
                    return Err("Stopped. Press Build shaders again to resume.".into());
                }
            }
            st.save(&path);
        }
    }
    Ok(if st.on_demand.is_empty() {
        format!("Shaders built in {}.", flow::duration(p.started.elapsed().as_secs()))
    } else {
        format!("Shaders built in {}; {} pipeline chunks compile in game.", flow::duration(p.started.elapsed().as_secs()), st.on_demand.len())
    })
}

fn classic(job: &Job) -> Result<String, String> {
    let path = state_path(&job.cache_dir, false);
    let mut st = State::load(&path, &job.hash);
    let pack = job.game.join("shaders.rpf");
    if !pack.is_file() {
        return Err("No shaders.rpf in the game folder.".into());
    }
    // Each of the biggest driver compiles takes about 1.5 GB.
    let pipes = engine::jobs(cores(), free_mb(), 1536).min(8);
    let dxc = (cores() / 2).max(2);
    let mut cmd = Command::new(&job.exe);
    cmd.current_dir(&job.runtime)
        .env("XCL_NO_WATCHDOG", "1")
        .env("XCL_DXC_DIR", &job.runtime)
        .env("XCL_DXIL_CACHE", crate::game::dxil_dir(&job.cache_dir))
        .env("XCL_DXC_THREADS", dxc.to_string())
        .env("XCL_PIPELINE_THREADS", pipes.to_string())
        .arg("shaders")
        .arg(&pack)
        .args(["--precompile", "--pipelines", "--only", "cs"]);
    log(&job.tx, format!("> xcl.exe shaders shaders.rpf --precompile --pipelines --only cs  ({dxc} compile threads, {pipes} driver compiles at once; cached shaders are skipped)"));
    let now = Instant::now();
    let mut p = Progress {
        phase: 1,
        phases: 1,
        name: "Compiling shaders and pipelines",
        done: 0,
        total: 0,
        current: "reading the shader pack".into(),
        current_since: now,
        last_output: now,
        overall: 0.,
        on_demand: 0,
        started: now,
        jobs: pipes,
        };
    send(job, &p);
    let idle = job.slow_secs.map(Duration::from_secs);
    let mut tick = Instant::now();
    let mut last_done = 0;
    let out = run(cmd, job, idle, |l| {
        if let Some((d, t, _)) = flow::parse_shader(l) {
            p.done = d;
            p.total = t;
            if d != last_done {
                last_done = d;
                p.current = format!("shaders after {d} (the largest take the driver 6–190 s each)");
                p.current_since = Instant::now();
            }
        }
        if !l.is_empty() {
            p.last_output = Instant::now();
        }
        p.overall = p.done as f32 / p.total.max(1) as f32;
        if !l.is_empty() || tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            send(job, &p);
        }
    })?;
    match out {
        Outcome::Exit(Some(0)) => {
            st.on_demand.clear();
            st.save(&path);
            Ok(format!("Shaders built in {}.", flow::duration(p.started.elapsed().as_secs())))
        }
        Outcome::Idle => {
            let left = p.total.saturating_sub(p.done);
            st.on_demand = vec![left];
            st.save(&path);
            log(&job.tx, format!("Nothing finished for {} s: the remaining {left} shaders compile in game (a rebuild resumes them).", job.slow_secs.unwrap_or(0)));
            Ok(format!("Shaders built; {left} slow ones compile in game."))
        }
        Outcome::Stopped => Err("Stopped. Press Build shaders again to resume (finished shaders are kept).".into()),
        Outcome::Exit(c) => Err(format!("xcl shaders failed (exit code {c:?}); see Logs")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_resets_for_another_engine_build() {
        let dir = std::env::temp_dir().join(format!("sp-state-{}", std::process::id()));
        let path = dir.join("s.json");
        let s = State { hash: "a".into(), warm: true, next: 3000, total: None, on_demand: vec![1000] };
        s.save(&path);
        assert_eq!(State::load(&path, "a"), s);
        assert_eq!(State::load(&path, "b"), State { hash: "b".into(), ..Default::default() });
        let _ = std::fs::remove_dir_all(dir);
    }
}
