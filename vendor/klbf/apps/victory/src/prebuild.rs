//! The pipeline pre-build (`victory translate --cache --pipelines`): every
//! pipeline of the title's shader pack made ahead of a run into the shader
//! cache and host pipeline library the run reads (`<cache>/shaders`,
//! `<cache>/pipelines`), so that a run compiles nothing the pack lists.
//! Ported from upstream 9c58a1c onto this tree's pipeline library.
//!
//! Each pack record becomes the creation stream the engine would hand the
//! device (`victory_game::pipelines`), which goes through the run's own
//! code (`victory_gpu::pipeline::Prebuild`): the host pipeline gets the
//! name a run gives it. Run it with the same executable and the same
//! translation switches as the game: the shader cache is keyed by both.
//!
//! - `VICTORY_SKIP_PIPELINES` (and the built-in list) is honoured as in a run.
//! - Resumable: the library is saved every [`SAVE_EVERY`] pipelines made
//!   (and every minute), and the keys it then holds are listed in
//!   `prebuilt-pipelines.txt` beside it (valid while the library file is
//!   at least the size on its first line: a run only adds to it); a next
//!   pre-build skips those.
//! - After each save the library is opened again from its file: a library
//!   holds every pipeline stored into it, and the host driver died after
//!   about 1,900 of them in one process (`amdxc64.dll`, RX 9070 XT).
//! - Each pipeline is printed (and flushed) before its host pipeline is
//!   made: if the host driver dies, the last lines name the culprit's shader
//!   hashes, for `VICTORY_SKIP_PIPELINES` (`--jobs 1` makes it the last one).
//! - The pipelines being made are also listed in `prebuild-inflight.txt`:
//!   after a crash, the next pre-build makes those first, one at a time; one
//!   that was made alone when the driver died goes to `prebuild-crashed.txt`
//!   and is skipped from then on. Running the pre-build again until it ends
//!   goes past every pipeline that kills the driver.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use victory_game::pipelines::{self, Named, Unnamed};
use victory_game::{sgd5, shaderpack};
use victory_gpu::pipeline::{self, Prebuild};
use victory_shader::{Container, FourCC, HwStage, native_programs};

use crate::shader::{MAX_FILE, Source, sources};

/// Where a native program's container is: file, source, and its stage.
type Found = (usize, usize, HwStage);

/// One pipeline to make: a graphics or compute record of the pack.
#[derive(Clone, Copy, Debug)]
enum Job {
    Graphics(usize),
    Compute(usize),
}

const LIBRARY_FILE: &str = "host-pipelines.bin";
const LIST_FILE: &str = "prebuilt-pipelines.txt";
/// The keys being made (left behind by a crash).
const INFLIGHT_FILE: &str = "prebuild-inflight.txt";
/// The keys the host driver died on, made alone: skipped.
const CRASHED_FILE: &str = "prebuild-crashed.txt";
/// Pipelines made between two saves of the library, at most.
const SAVE_EVERY: usize = 200;

pub fn run(paths: Vec<PathBuf>, jobs: Option<usize>) -> Result<()> {
    let paths = crate::default_paths(paths)?;
    let threads = jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let started = Instant::now();
    let cfg = victory_core::Config::load()?;
    let shaders_dir = cfg.cache_subdir("shaders")?;
    let dir = shaders_dir.with_file_name("pipelines");
    // As a run: translations in `shaders`, the library in `pipelines`.
    victory_hle::modules::d3d12x::use_shader_cache(shaders_dir);
    println!("cache        {}", dir.display());
    let files: Vec<(PathBuf, Vec<u8>)> = paths
        .iter()
        .flat_map(|root| walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok))
        .filter(|e| e.file_type().is_file() && e.metadata().is_ok_and(|m| m.len() <= MAX_FILE))
        .filter_map(|e| Some((e.path().to_owned(), std::fs::read(e.path()).ok()?)))
        .filter(|(_, d)| d.starts_with(shaderpack::MAGIC) || !sources(d).is_empty())
        .collect();
    let found = (files.iter())
        .filter(|(_, d)| d.starts_with(shaderpack::MAGIC) || sgd5::is_sgd5(d))
        .find_map(|(p, d)| Some((p, shaderpack::find(d)?)));
    let Some((pack_path, pack)) = found else {
        println!("pipelines    no shader pack among the files: nothing to pre-build");
        return Ok(());
    };
    let pack = shaderpack::Pack::parse(&pack).map_err(anyhow::Error::msg)?;
    let graphics: Vec<shaderpack::Graphics> = pack.graphics().collect();
    let compute: Vec<shaderpack::Compute> = pack.compute().collect();

    // Every container by the hash instructions its native program starts
    // with, which the pack's programs start with too.
    let sources: Vec<Vec<Source>> = files.iter().map(|(_, d)| sources(d)).collect();
    let programs: Mutex<HashMap<[u8; 24], Found>> = Mutex::new(HashMap::new());
    for (f, list) in sources.iter().enumerate() {
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads.min(list.len()) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(source) = list.get(i) else { break };
                        let Some(bytes) = source.bytes() else { continue };
                        for (id, stage) in program_ids(&bytes) {
                            lock(&programs).entry(id).or_insert((f, i, stage));
                        }
                    }
                });
            }
        });
    }
    let programs = programs.into_inner().unwrap_or_else(|e| e.into_inner());
    let databases: Vec<sgd5::Database> = files.iter().flat_map(|(_, d)| sgd5::databases(d)).collect();
    let named = Named::find(&pack, &databases);
    eprintln!(
        "{}: shader pack, {} graphics and {} compute pipelines; states named: {} depth-stencil, {} rasteriser, {} blend",
        pack_path.display(),
        graphics.len(),
        compute.len(),
        named.depth.len(),
        named.raster.len(),
        named.blend.len(),
    );

    let container = |code: &[u8]| -> Option<(Vec<u8>, HwStage)> {
        let &(f, i, stage) = programs.get(&shaderpack::program_id(code)?)?;
        let bytes = sources[f][i].bytes()?;
        // As the title passes it: the container's own size.
        let size = u32::from_le_bytes(bytes.get(0x18..0x1c)?.try_into().ok()?) as usize;
        Some((bytes.get(..size)?.to_vec(), stage))
    };
    // A job's pipeline, or why it has none. The stream points into
    // `first`/`pixel`, which the closure's caller keeps alive through `f`.
    let with = |job: Job, f: &mut dyn FnMut(Result<&Prebuild, String>)| {
        let g = match job {
            Job::Graphics(i) => &graphics[i],
            Job::Compute(i) => {
                return match container(compute[i].stage.program) {
                    Some((cs, _)) => f(Ok(&Prebuild::compute(&cs))),
                    None => f(Err("shader not found".into())),
                };
            }
        };
        let [ps, vs, gs, ..] = g.stages;
        if g.stages[3..].iter().any(Option::is_some) {
            return f(Err("stages other than vertex, mesh and pixel".into()));
        }
        let Some((first, stage)) = vs.or(gs).and_then(|s| container(s.program)) else {
            return f(Err("shader not found".into()));
        };
        let pixel = match ps {
            Some(ps) => match container(ps.program) {
                Some((c, _)) => Some(c),
                None => return f(Err("shader not found".into())),
            },
            None => None,
        };
        let mesh = match stage {
            HwStage::Ms => true,
            HwStage::Vs => false,
            _ => return f(Err("first stage not a vertex or mesh shader".into())),
        };
        match pipelines::stream(g, &named, &first, mesh, pixel.as_deref()) {
            Err(u) => f(Err(match u {
                Unnamed::DepthStencil => "depth-stencil state not named",
                Unnamed::Rasterizer => "rasteriser state not named",
                Unnamed::Blend => "blend state not named",
                Unnamed::Metadata => "metadata not read",
            }
            .to_owned())),
            // SAFETY: the stream points at `first`, `pixel` and its own
            // input elements, all alive for the call.
            Ok(stream) => f(Ok(&unsafe { Prebuild::graphics(stream.bytes()) })),
        }
    };

    let mut jobs: Vec<Job> = (0..compute.len()).map(Job::Compute).collect();
    jobs.extend((0..graphics.len()).map(Job::Graphics));
    let skipped: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
    let skip = |why: String| *lock(&skipped).entry(why).or_default() += 1;
    let done: HashSet<String> = read_list(&dir);
    let mut crashed = read_keys(&dir.join(CRASHED_FILE));
    let suspects = read_keys(&dir.join(INFLIGHT_FILE));
    if suspects.len() == 1 {
        // Made alone when the driver died: the culprit.
        crashed.extend(suspects.iter().cloned());
        let mut keys: Vec<&String> = crashed.iter().collect();
        keys.sort();
        let text: String = keys.iter().map(|k| format!("{k}\n")).collect();
        let _ = std::fs::write(dir.join(CRASHED_FILE), text);
    }
    if !crashed.is_empty() || !suspects.is_empty() {
        println!("crashes      {} pipelines skipped (the driver died on them), {} suspects made first, alone", crashed.len(), suspects.len());
    }
    let first: Mutex<Vec<Job>> = Mutex::new(Vec::new());
    let already = AtomicUsize::new(0);
    // First every record's key (no translation): listed ones are skipped,
    // what an earlier run drew with is made first.
    let todo: Mutex<Vec<(bool, Job)>> = Mutex::new(Vec::new());
    each(&jobs, threads, |job| {
        with(job, &mut |p| match p {
            Ok(p) if done.contains(&p.key()) => drop(already.fetch_add(1, Ordering::Relaxed)),
            Ok(p) if crashed.contains(&p.key()) => skip("the host driver died on it in an earlier pre-build".into()),
            Ok(p) if suspects.contains(&p.key()) => lock(&first).push(job),
            Ok(p) if p.skipped().is_some() => skip(format!("VICTORY_SKIP_PIPELINES ({})", p.skipped().unwrap_or_default())),
            Ok(p) => lock(&todo).push((!pipeline::wanted_early(&p.key()), job)),
            Err(why) => skip(why),
        })
    });
    let mut todo = todo.into_inner().unwrap_or_else(|e| e.into_inner());
    todo.sort_by_key(|&(later, _)| later);
    let todo: Vec<Job> = todo.into_iter().map(|(_, job)| job).collect();
    let first = first.into_inner().unwrap_or_else(|e| e.into_inner());
    println!(
        "pipelines    {} in the pack, {} already pre-built, {} to make on {threads} threads",
        jobs.len(),
        already.load(Ordering::Relaxed),
        todo.len() + first.len()
    );

    let (made0, loaded0) = pipeline::pipeline_counts();
    let pending: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut listed: Vec<String> = done.into_iter().collect();
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let out = Mutex::new(std::io::stdout());
    // Held by each make; a save takes it alone (the library is opened
    // again then).
    let gate = RwLock::new(());
    let inflight: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    let note_inflight = |key: &str, making: bool| {
        let mut keys = lock(&inflight);
        if making {
            keys.insert(key.to_owned());
        } else {
            keys.remove(key);
        }
        let text: String = keys.iter().map(|k| format!("{k}\n")).collect();
        // Written before the driver sees it; the system keeps it if we die.
        let _ = std::fs::write(dir.join(INFLIGHT_FILE), text);
    };
    let make_one = |job: Job| {
        with(job, &mut |p| {
            let p = match p {
                Ok(p) => p,
                Err(why) => return skip(why),
            };
            let key = p.key();
            {
                // Before the driver sees it: the culprit if it dies.
                let mut o = lock(&out);
                let _ = writeln!(o, "> {key} [{}]", p.shaders());
                let _ = o.flush();
            }
            let _making = gate.read().unwrap_or_else(|e| e.into_inner());
            note_inflight(&key, true);
            let made = p.make();
            note_inflight(&key, false);
            match made {
                Ok(()) => lock(&pending).push(key),
                Err(why) => skip(why),
            }
        });
        finished.fetch_add(1, Ordering::Relaxed);
    };
    // The suspects of a crash, alone: the one the driver dies on is named.
    first.iter().for_each(|&job| make_one(job));
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads.min(todo.len()))
            .map(|_| {
                s.spawn(|| {
                    while let Some(&job) = todo.get(next.fetch_add(1, Ordering::Relaxed)) {
                        make_one(job);
                    }
                })
            })
            .collect();
        let mut last_save = Instant::now();
        while !workers.iter().all(|w| w.is_finished()) {
            std::thread::sleep(Duration::from_millis(250));
            if last_save.elapsed() >= Duration::from_secs(60) || lock(&pending).len() >= SAVE_EVERY {
                {
                    let _alone = gate.write().unwrap_or_else(|e| e.into_inner());
                    save(&dir, &pending, &mut listed);
                }
                last_save = Instant::now();
                let (made, loaded) = pipeline::pipeline_counts();
                eprintln!(
                    "{}: {}/{} pipelines, {} made, {} found in the library, {:.0} s",
                    pack_path.display(),
                    finished.load(Ordering::Relaxed),
                    todo.len(),
                    made - made0,
                    loaded - loaded0,
                    started.elapsed().as_secs_f64()
                );
            }
        }
    });
    save(&dir, &pending, &mut listed);
    let _ = std::fs::remove_file(dir.join(INFLIGHT_FILE));
    let (made, loaded) = pipeline::pipeline_counts();
    println!(
        "pipelines    {} in the pack, {:.1} s: {} made, {} found in the library, {} already pre-built",
        jobs.len(),
        started.elapsed().as_secs_f64(),
        made - made0,
        loaded - loaded0,
        already.load(Ordering::Relaxed),
    );
    for (why, n) in lock(&skipped).iter() {
        println!("  {n:>7}  skipped: {why}");
    }
    Ok(())
}

/// Saves the library (and opens it again), then lists the keys made before
/// the save began.
fn save(dir: &Path, pending: &Mutex<Vec<String>>, listed: &mut Vec<String>) {
    let saved: Vec<String> = std::mem::take(&mut *lock(pending));
    pipeline::save_and_reopen_pipeline_cache();
    listed.extend(saved);
    let Ok(meta) = std::fs::metadata(dir.join(LIBRARY_FILE)) else { return };
    let text = format!("library {}\n{}\n", meta.len(), listed.join("\n"));
    let tmp = dir.join(LIST_FILE).with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(LIST_FILE));
    }
}

/// The keys an earlier pre-build listed, if the library still holds them:
/// it is at least as big as when they were listed (a run saving the library
/// after them only adds to it; one the driver refused starts empty).
fn read_list(dir: &Path) -> HashSet<String> {
    let (Ok(text), Ok(meta)) = (std::fs::read_to_string(dir.join(LIST_FILE)), std::fs::metadata(dir.join(LIBRARY_FILE)))
    else {
        return HashSet::new();
    };
    listed(&text, meta.len())
}

/// [`read_list`]'s reading of the list file `text` for a library of `len` bytes.
fn listed(text: &str, len: u64) -> HashSet<String> {
    let mut lines = text.lines();
    let size = lines.next().and_then(|l| l.strip_prefix("library ")).and_then(|n| n.parse::<u64>().ok());
    if size.is_none_or(|size| len < size) {
        return HashSet::new();
    }
    lines.filter(|l| !l.is_empty()).map(str::to_owned).collect()
}

/// The keys a file lists, one a line (none without the file).
fn read_keys(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Runs `f` on every job, on `threads` threads.
fn each(jobs: &[Job], threads: usize, f: impl Fn(Job) + Sync) {
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads.min(jobs.len()) {
            s.spawn(|| {
                while let Some(&job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    f(job);
                }
            });
        }
    });
}

/// The hash instructions each native program of a container starts with.
fn program_ids(bytes: &[u8]) -> Vec<([u8; 24], HwStage)> {
    let Ok(c) = Container::parse(bytes) else {
        return Vec::new();
    };
    let Some(part) = c.part(FourCC::DXIL) else {
        return Vec::new();
    };
    native_programs(part.data)
        .map(|programs| {
            (programs.iter())
                .filter_map(|p| Some((shaderpack::program_id(&p.code)?, p.stage)))
                .collect()
        })
        .unwrap_or_default()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_list_holds_while_the_library_only_grows() {
        let text = "library 100\na\nb\n";
        assert_eq!(super::listed(text, 100).len(), 2);
        assert_eq!(super::listed(text, 150).len(), 2);
        assert!(super::listed(text, 99).is_empty());
        assert!(super::listed("garbage\na\n", 100).is_empty());
    }
}
