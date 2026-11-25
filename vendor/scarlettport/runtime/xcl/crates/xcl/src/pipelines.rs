//! `xcl pipelines`: compiles a title's pipelines in its own process, from the journal a run
//! writes (`hle::d3d12x::pso_journal`): the DXIL cache and the GPU driver's cache get what the
//! title asks for, so its pipelines are ready when it binds them – before the next run, or
//! during this one (`--watch` follows the journal as the title writes it, at below-normal
//! priority). Pipelines the title used come first. What is done is kept next to the journal
//! (`pipelines.done`, for this XCL binary), so a stopped run goes on where it was.
//! It must be the same xcl.exe as the title's: the translations' cache key holds the binary's
//! hash, and the driver keeps its cache per program.

use crate::hle::d3d12x::pso_journal::{self, Replay};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const HELP: &str = "\
xcl pipelines <game folder | title data folder | pipelines.bin> [--watch] [--threads N] [--limit N]
  Compiles the pipelines a run of the title asked for (<data>\\cache\\pipelines.bin) outside
  the title: the DXIL cache and the driver's cache. --watch: also while the title runs,
  following what it adds (stop with Ctrl+C). --threads: pipelines at once (default: half the
  cores, at below-normal priority; XCL_PIPELINE_THREADS caps the driver compiles, default 4, 2 with --watch).";

/// The journal of `target`: the file itself, a data folder with one, or a game folder.
fn journal_of(target: &Path) -> Result<PathBuf, String> {
    if target.is_file() && target.extension().is_some_and(|e| e.eq_ignore_ascii_case("bin")) {
        return Ok(target.to_path_buf());
    }
    let own = pso_journal::path_in(target);
    if own.is_file() {
        return Ok(own);
    }
    let (_, _, title) = crate::launcher::resolve_location(target)?;
    let j = pso_journal::path_in(&crate::launcher::data_root().join(&title));
    if j.is_file() {
        Ok(j)
    } else {
        Err(format!("no pipeline journal at {} – run the title once with this XCL (`xcl run`) to write it", j.display()))
    }
}

/// This binary (size and time written): what `pipelines.done` is valid for.
fn binary_key() -> u64 {
    let m = std::env::current_exe().and_then(std::fs::metadata);
    m.map_or(0, |m| {
        let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
        crate::gpu::shader::fnv64(&[m.len().to_le_bytes(), t.to_le_bytes()].concat())
    })
}

#[derive(Default)]
struct Queue {
    /// pipelines the title built, then the others (journal order)
    used: VecDeque<u64>,
    rest: VecDeque<u64>,
    /// done, or taken by a worker
    taken: HashSet<u64>,
    shaders: HashMap<u64, Arc<Vec<u8>>>,
    pipes: HashMap<u64, (u8, Arc<Vec<u8>>)>,
    was_used: HashSet<u64>,
    /// the memory windows of the run (`run` sets them: `pso::set_mem_windows`); other runs' are left out
    windows: u8,
    other_windows: usize,
    /// the journal: bytes read, and where its whole frames end
    buf: Vec<u8>,
    parsed: usize,
    closed: bool,
}

impl Queue {
    /// Read what was added to the journal; the new work.
    fn ingest(&mut self, file: &mut std::fs::File) -> usize {
        // a torn last frame may be cut and written over by the next run: read it again
        self.buf.truncate(self.parsed);
        // a journal made again (shorter than what was read): from its start
        if file.metadata().is_ok_and(|m| m.len() < self.buf.len() as u64) {
            self.buf.clear();
            self.parsed = 0;
        }
        let start = self.buf.len();
        let _ = file.seek(std::io::SeekFrom::Start(start as u64));
        let _ = file.read_to_end(&mut self.buf);
        if self.buf.len() == start && self.parsed != 0 {
            return 0;
        }
        if self.parsed == 0 {
            if !self.buf.starts_with(pso_journal::MAGIC) {
                return 0;
            }
            self.parsed = pso_journal::MAGIC.len();
        }
        let mut new = 0;
        let mut frames = Vec::new();
        self.parsed = pso_journal::for_each_frame_from(&self.buf, self.parsed, |f| frames.push((f.tag, f.windows, f.hash, f.payload.to_vec())));
        if self.windows == 0 {
            // the last run's (a run writes all its pipelines again when its windows differ)
            self.windows = frames.iter().rev().find(|f| matches!(f.0, b'G' | b'C')).map_or(0, |f| f.1.max(1));
        }
        for (tag, windows, hash, payload) in frames {
            match tag {
                b'S' => {
                    self.shaders.insert(hash, Arc::new(payload));
                }
                b'G' | b'C' => {
                    if windows.max(1) != self.windows {
                        self.other_windows += 1;
                        continue;
                    }
                    self.pipes.insert(hash, (tag, Arc::new(payload)));
                    if !self.taken.contains(&hash) {
                        if self.was_used.contains(&hash) {
                            self.used.push_back(hash);
                        } else {
                            self.rest.push_back(hash);
                        }
                        new += 1;
                    }
                }
                b'U' => {
                    if self.was_used.insert(hash) && self.pipes.contains_key(&hash) && !self.taken.contains(&hash) {
                        self.used.push_back(hash);
                    }
                }
                _ => {}
            }
        }
        new
    }

    fn next(&mut self) -> Option<(u64, Option<Replay>)> {
        while let Some(h) = self.used.pop_front().or_else(|| self.rest.pop_front()) {
            if !self.taken.insert(h) {
                continue;
            }
            let (tag, payload) = self.pipes.get(&h)?.clone();
            let frame = pso_journal::Frame { tag, windows: self.windows, hash: h, payload: &payload };
            return Some((h, pso_journal::replay(&frame, |s| self.shaders.get(&s).map(|b| b.to_vec()))));
        }
        None
    }

    fn left(&self) -> usize {
        self.used.len() + self.rest.len()
    }
}

pub fn run(target: &Path, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return 0;
    }
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<usize>().ok());
    let watch = args.iter().any(|a| a == "--watch");
    let limit = opt("--limit").unwrap_or(usize::MAX);
    let journal = match journal_of(target) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("xcl pipelines: {e}");
            return 2;
        }
    };
    let dev = match device(&adapter_setting(&journal)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("xcl pipelines: no GPU device: {e}");
            return 2;
        }
    };
    // a background job: the title's threads and the user's programs first, and about half the
    // cores (DXC's pool too, unless XCL_DXC_THREADS says otherwise) – the PC stays usable
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS};
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
    let half = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).max(1);
    if std::env::var_os("XCL_DXC_THREADS").is_none() {
        std::env::set_var("XCL_DXC_THREADS", half.to_string());
    }
    // `wait_for_memory` keeps 6 GB of commit free, as next to a running title: the tool runs in
    // the background of a PC in use, and a title's last pipelines take GBs each in DXC
    crate::gpu::recomp::dxc::set_memory_reserve(6);
    let gpu_most = std::env::var("XCL_PIPELINE_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(if watch { 2 } else { 4usize }).max(1);
    let threads = opt("--threads").unwrap_or(half).max(1);

    let mut file = match std::fs::File::open(&journal) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("xcl pipelines: {}: {e}", journal.display());
            return 2;
        }
    };
    let q = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
    // what an earlier run of this binary finished
    let done_path = journal.with_extension("done");
    let key = binary_key();
    let mut done_file = match open_done(&done_path, key) {
        Ok((f, done)) => {
            q.0.lock().unwrap().taken.extend(done);
            f
        }
        Err(e) => {
            eprintln!("xcl pipelines: {}: {e}", done_path.display());
            return 2;
        }
    };
    // what a run that ended without its summary (a crash in the driver or DXC) was making: left
    // to the title, so the same pipeline does not end every run
    let trying_path = journal.with_extension("trying");
    let crashed: Vec<u64> = {
        let t = std::fs::read(&trying_path).unwrap_or_default();
        let mut g = q.0.lock().unwrap();
        t.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).filter(|h| g.taken.insert(*h)).collect()
    };
    for h in &crashed {
        let _ = done_file.write_all(&h.to_le_bytes());
    }
    if !crashed.is_empty() {
        eprintln!("xcl pipelines: the last run ended while making {} pipelines: left to the title", crashed.len());
    }
    let trying = match std::fs::File::create(&trying_path) {
        Ok(f) => Mutex::new(f),
        Err(e) => {
            eprintln!("xcl pipelines: {}: {e}", trying_path.display());
            return 2;
        }
    };
    let (already, first) = {
        let mut g = q.0.lock().unwrap();
        let first = g.ingest(&mut file);
        // the HLSL (and so the cache keys) of the run's memory windows
        crate::gpu::recomp::pso::set_mem_windows(g.windows as u32);
        (g.taken.len(), first)
    };
    {
        let g = q.0.lock().unwrap();
        println!(
            "{}: {first} pipelines to make ({} the title used), {already} done before, {} shaders; {} memory windows; {threads} threads, {gpu_most} driver compiles at once{}",
            journal.display(),
            g.used.len(),
            g.shaders.len(),
            g.windows,
            if watch { "; following the journal (Ctrl+C stops)" } else { "" }
        );
    }

    let t0 = std::time::Instant::now();
    let (made, failed, skipped) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let reasons: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
    let done_tx = Mutex::new(&mut done_file);
    let gpu = (Mutex::new(0usize), Condvar::new());
    let started = AtomicUsize::new(0);
    std::thread::scope(|s| {
        if watch {
            let q = q.clone();
            s.spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let mut g = q.0.lock().unwrap();
                if g.closed {
                    return;
                }
                let new = g.ingest(&mut file);
                // (a journal that was empty when the tool started)
                crate::gpu::recomp::pso::set_mem_windows(g.windows as u32);
                if new > 0 || !g.used.is_empty() {
                    q.1.notify_all();
                }
            });
        }
        for k in 0..threads {
            let (q, dev, made, failed, skipped, reasons, done_tx, gpu, started, trying) = (&q, &dev, &made, &failed, &skipped, &reasons, &done_tx, &gpu, &started, &trying);
            let _ = std::thread::Builder::new().name(format!("xcl-warm-{k}")).stack_size(64 << 20).spawn_scoped(s, move || loop {
                let item = {
                    let mut g = q.0.lock().unwrap();
                    loop {
                        if started.load(Ordering::Relaxed) >= limit {
                            g.closed = true;
                            q.1.notify_all();
                            return;
                        }
                        if let Some(it) = g.next() {
                            started.fetch_add(1, Ordering::Relaxed);
                            break it;
                        }
                        if !watch || g.closed {
                            return;
                        }
                        g = q.1.wait(g).unwrap();
                    }
                };
                let (hash, replay) = item;
                let _ = trying.lock().unwrap().write_all(&hash.to_le_bytes());
                let r = match &replay {
                    None => Err("not readable from the journal".to_string()),
                    // the driver's compiles, few at once (the biggest take ~1.5 GB each)
                    Some(r) => unsafe { crate::hle::d3d12x::real::warm(dev, r, || DriverSlot::take(gpu, gpu_most)) },
                };
                // done: made, or left to the title; a failure is tried again by the next run
                let done = match r {
                    Ok(()) => {
                        made.fetch_add(1, Ordering::Relaxed);
                        true
                    }
                    Err(e) if e.contains("left to the title") => {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        true
                    }
                    Err(e) => {
                        let reason: String = e.lines().find(|l| l.contains("error")).unwrap_or(e.lines().next().unwrap_or("")).chars().take(160).collect();
                        *reasons.lock().unwrap().entry(reason).or_default() += 1;
                        failed.fetch_add(1, Ordering::Relaxed);
                        false
                    }
                };
                if done {
                    let _ = done_tx.lock().unwrap().write_all(&hash.to_le_bytes());
                }
                let n = made.load(Ordering::Relaxed) + failed.load(Ordering::Relaxed) + skipped.load(Ordering::Relaxed);
                if n % 100 == 0 {
                    let left = q.0.lock().unwrap().left();
                    let rate = n as f64 / t0.elapsed().as_secs_f64().max(0.001);
                    eprintln!("  {n} done, {left} left ({rate:.1}/s, ~{:.0} min to go)", left as f64 / rate.max(0.001) / 60.0);
                }
            });
        }
    });
    // ended by itself: nothing was cut short
    drop(trying);
    let _ = std::fs::remove_file(&trying_path);
    let g = q.0.lock().unwrap();
    println!(
        "made {}, failed {}, left to the title (GS / HS / DS) {}, time {:.1} s",
        made.into_inner(),
        failed.into_inner(),
        skipped.into_inner(),
        t0.elapsed().as_secs_f64()
    );
    if g.other_windows > 0 {
        println!("  {} pipelines from a run with other memory windows left out", g.other_windows);
    }
    let mut r: Vec<_> = reasons.into_inner().unwrap().into_iter().collect();
    r.sort_by(|a, b| b.1.cmp(&a.1));
    for (why, n) in r.iter().take(10) {
        println!("  {n}× {why}");
    }
    0
}

/// A driver compile's turn (`XCL_PIPELINE_THREADS` at once), given back when dropped.
struct DriverSlot<'a>(&'a (Mutex<usize>, Condvar));

impl<'a> DriverSlot<'a> {
    fn take(gpu: &'a (Mutex<usize>, Condvar), most: usize) -> DriverSlot<'a> {
        let mut n = gpu.0.lock().unwrap();
        while *n >= most {
            n = gpu.1.wait(n).unwrap();
        }
        *n += 1;
        DriverSlot(gpu)
    }
}

impl Drop for DriverSlot<'_> {
    fn drop(&mut self) {
        *self.0 .0.lock().unwrap() -= 1;
        self.0 .1.notify_one();
    }
}

/// The GPU the title's runs use: its `xcl.toml`'s `d3d12_adapter` (or `XCL_ADAPTER`), as
/// `real::init` picks it – the driver's cache and the wave width are the adapter's.
fn adapter_setting(journal: &Path) -> String {
    let toml = journal.parent().and_then(Path::parent).map(|d| d.join("xcl.toml"));
    match toml.filter(|t| t.is_file()).map(|t| crate::config::Config::load_or_create(&t)) {
        Some(Ok(c)) => c.d3d12_adapter,
        _ => "auto".into(),
    }
}

fn device(adapter: &str) -> Result<windows::Win32::Graphics::Direct3D12::ID3D12Device, String> {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    use windows::Win32::Graphics::Dxgi::*;
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(|e| e.to_string())?;
        let sel = crate::gpu::adapter::Selector::from_env_or(adapter);
        let (adapter, _) = crate::gpu::adapter::candidates(&factory, &sel).into_iter().next().ok_or_else(|| crate::gpu::adapter::no_match(&factory, &sel))?;
        crate::gpu::agility::create_device(&adapter, D3D_FEATURE_LEVEL_12_0).map_err(|e| e.to_string())
    }
}

/// Opens `pipelines.done` for `key` (this binary): the hashes it holds, and the file positioned
/// after them. A file of another binary starts over; a torn last record is cut off.
fn open_done(path: &Path, key: u64) -> std::io::Result<(std::fs::File, Vec<u64>)> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ: a second `xcl pipelines` on this journal is refused, not interleaved
    let mut f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(1).open(path)?;
    let mut old = Vec::new();
    f.read_to_end(&mut old)?;
    let done: Vec<u64> = if old.len() >= 8 && old[..8] == key.to_le_bytes() {
        old[8..].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
    } else {
        Vec::new()
    };
    if done.is_empty() {
        f.set_len(0)?;
        f.seek(std::io::SeekFrom::Start(0))?;
        f.write_all(&key.to_le_bytes())?;
    } else {
        let end = 8 + 8 * done.len() as u64;
        f.set_len(end)?;
        f.seek(std::io::SeekFrom::Start(end))?;
    }
    Ok((f, done))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_torn_frame_written_over_is_read_again() {
        let path = std::env::temp_dir().join(format!("xcl_pipelines_torn_{}.bin", std::process::id()));
        let mut j = pso_journal::MAGIC.to_vec();
        j.extend(frame(b'C', 1, &[0; 8]));
        let good = j.len();
        j.extend(&frame(b'C', 2, &[0; 8])[..9]); // a run killed mid-frame
        std::fs::write(&path, &j).unwrap();
        let mut f = std::fs::File::open(&path).unwrap();
        let mut q = Queue::default();
        assert_eq!(q.ingest(&mut f), 1);
        // the next run cuts the torn frame and writes on from there
        j.truncate(good);
        j.extend(frame(b'C', 3, &[0; 8]));
        j.extend(frame(b'C', 4, &[0; 8]));
        std::fs::write(&path, &j).unwrap();
        assert_eq!(q.ingest(&mut f), 2, "3 and 4, read from where the torn frame began");
        drop(f);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn done_file_of_another_binary_is_replaced_and_a_torn_record_cut() {
        let path = std::env::temp_dir().join(format!("xcl-done-test-{}.done", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (mut f, done) = open_done(&path, 1).unwrap();
        assert!(done.is_empty());
        f.write_all(&7u64.to_le_bytes()).unwrap();
        f.write_all(&[9, 9, 9]).unwrap(); // a record cut short
        drop(f);
        let (mut f, done) = open_done(&path, 1).unwrap();
        assert_eq!(done, [7]);
        f.write_all(&8u64.to_le_bytes()).unwrap();
        drop(f);
        assert_eq!(std::fs::read(&path).unwrap().len(), 24, "the torn record was cut, 8 follows 7");
        let (mut f, done) = open_done(&path, 2).unwrap();
        assert!(done.is_empty(), "another binary's progress is not reused");
        f.write_all(&5u64.to_le_bytes()).unwrap();
        drop(f);
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(bytes, [2u64.to_le_bytes(), 5u64.to_le_bytes()].concat(), "the old file was replaced, not appended to");
    }

    fn frame(tag: u8, hash: u64, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![tag, 4, 0, 0];
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(&hash.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn used_pipelines_first_done_ones_skipped_journal_followed() {
        let path = std::env::temp_dir().join(format!("xcl_pipelines_queue_{}.bin", std::process::id()));
        let mut j = pso_journal::MAGIC.to_vec();
        for h in [1u64, 2, 3] {
            j.extend(frame(b'C', h, &[0; 8]));
        }
        j.extend(frame(b'U', 3, &[]));
        // half a frame: written later
        let tail = frame(b'U', 2, &[]);
        j.extend(&tail[..5]);
        std::fs::write(&path, &j).unwrap();
        let mut f = std::fs::File::open(&path).unwrap();
        let mut q = Queue::default();
        q.taken.insert(1);
        assert_eq!(q.ingest(&mut f), 2, "pipeline 1 was done before");
        assert_eq!(q.windows, 4);
        assert_eq!(q.next().map(|(h, _)| h), Some(3), "the used one first");
        // the rest of the torn frame arrives: 2 is used now
        let mut w = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        w.write_all(&tail[5..]).unwrap();
        drop(w);
        assert_eq!(q.ingest(&mut f), 0);
        assert_eq!(q.next().map(|(h, _)| h), Some(2));
        assert!(q.next().is_none(), "each pipeline once");
        drop(f);
        let _ = std::fs::remove_file(&path);
    }
}
