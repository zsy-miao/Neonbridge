//! `victory warm`: the title's shaders translated into the shader cache
//! before a run (see docs/precompilacao-shaders.md).
//!
//! A run translates each shader when the title creates a pipeline with it,
//! on the pipeline workers, while the title waits up to its budget per
//! frame (past it the draw is dropped). The translation is a function of
//! the container's bytes and of this build alone (the cache key:
//! `victory_gpu::shader_cache`), so every container found in the title's
//! files can be translated ahead of time into the same entries. The run
//! then reads them back instead.
//!
//! Not covered: shaders a title makes or loads only in memory (still
//! translated during the run), and the host driver's compile of each
//! pipeline, which needs the graphics state only the title gives (the
//! pipeline library keeps those from the first run on).

use std::path::PathBuf;

use anyhow::Result;

#[cfg(windows)]
pub fn run(paths: Vec<PathBuf>, list: bool, jobs: Option<usize>) -> Result<()> {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use std::time::Instant;

    use victory_hle::modules::d3d12x;
    use victory_shader::Container;

    use crate::shader::{MAX_FILE, sources};

    let paths = crate::default_paths(paths)?;
    let cfg = victory_core::Config::load()?;
    let dir = cfg.cache_subdir("shaders")?;
    println!("shader cache {}", dir.display());
    println!(
        "note         entries are only valid for this victory.exe and its VICTORY_* settings;"
    );
    println!("             warm with the same build and settings you run the title with");
    d3d12x::use_shader_cache(dir);

    let threads =
        jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let seen = Mutex::new(HashSet::new());
    let failures: Mutex<BTreeMap<String, (usize, String)>> = Mutex::new(BTreeMap::new());
    let (found, cached, translated, failed) = (
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    );
    let started = Instant::now();
    for root in &paths {
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() || entry.metadata().map_or(0, |m| m.len()) > MAX_FILE {
                continue;
            }
            let Ok(data) = std::fs::read(entry.path()) else {
                continue;
            };
            let jobs = sources(&data);
            if jobs.is_empty() {
                continue;
            }
            let path = entry.path();
            let next = AtomicUsize::new(0);
            std::thread::scope(|s| {
                for _ in 0..threads.min(jobs.len()) {
                    s.spawn(|| {
                        while let Some(job) = jobs.get(next.fetch_add(1, Relaxed)) {
                            let Some(bytes) = job.bytes() else {
                                continue;
                            };
                            // The bytes a title passes: this container only,
                            // not the rest of the file after it.
                            let Ok(container) = Container::bytes_of(&bytes) else {
                                continue;
                            };
                            let new = seen
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(blake3::hash(container));
                            if !new {
                                continue;
                            }
                            found.fetch_add(1, Relaxed);
                            match d3d12x::warm_shader(container) {
                                Ok(false) => {
                                    cached.fetch_add(1, Relaxed);
                                }
                                Ok(true) => {
                                    translated.fetch_add(1, Relaxed);
                                }
                                Err(e) => {
                                    failed.fetch_add(1, Relaxed);
                                    let example = format!("{}{}", path.display(), job.label());
                                    if list {
                                        println!("does not translate: {example}: {e}");
                                    }
                                    let mut f = failures.lock().unwrap_or_else(|e| e.into_inner());
                                    f.entry(e).or_insert((0, example)).0 += 1;
                                }
                            }
                        }
                    });
                }
            });
        }
    }
    let (found, cached, translated, failed) = (
        found.into_inner(),
        cached.into_inner(),
        translated.into_inner(),
        failed.into_inner(),
    );
    println!(
        "containers   {found} distinct: {translated} translated now, {cached} already cached, \
         {failed} do not translate ({:.1} s)",
        started.elapsed().as_secs_f64()
    );
    let mut failures: Vec<_> = failures
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .collect();
    failures.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    for (reason, (n, example)) in failures.iter().take(20) {
        println!("  {n:>7}  {reason}  (e.g. {example})");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(_: Vec<PathBuf>, _: bool, _: Option<usize>) -> Result<()> {
    anyhow::bail!("warming the shader cache requires Windows (the host D3D12 device)")
}
