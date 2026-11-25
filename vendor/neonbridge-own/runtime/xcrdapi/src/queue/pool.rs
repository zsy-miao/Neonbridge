//! TUXBOX_DSTORAGE_POOL (default on, =0 each queue runs its own reads,
//! TUXBOX_STORAGE_PARALLEL at a time): every queue's reads run on one pool of
//! workers, picked by weighted round robin over priority classes (about 10x
//! per level, counted in bytes) and bounded on the staging bytes in flight.
//!
//! Copied from VIctory-fxceless crates/victory-hle/src/modules/dstorage.rs
//! (`Scheduler`, `Lanes`, `Lane`, `Job`, `COST`, `STAGING`, `scheduler`,
//! `work`, `start`, `finish`, `submit`, `staging_bytes`; GPL-3.0, credit to
//! its authors), adapted only at the seams:
//! - Each queue's own thread (`consume`) keeps the order of its statuses,
//!   signals and events: it hands the pool a run of ready reads (`run`) and
//!   waits for them, where fxceless's lane also holds the status entries and
//!   writes them in `flush`. So a lane here holds reads only, and is dropped
//!   once empty (its clock comes back from `lane_now` when it returns).
//! - Real-time (our priority 3) shares the high class, as fxceless clamps.
//! - A read's result goes back to the waiting queue thread, which reports
//!   failures as before.
use super::*;
use std::collections::{BTreeSet, HashMap};

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TUXBOX_DSTORAGE_POOL").is_ok_and(|v| v.trim() == "0"))
}

/// Every queue's reads, run by a pool of workers.
struct Scheduler {
    lanes: Mutex<Lanes>,
    /// Reads to start, or staging space freed.
    work: Condvar,
    /// A read finished (for the queue threads waiting on their runs).
    finished: Condvar,
}

/// Priority weights per class (low, normal, high and real-time), as the
/// console's: about 10x per level, counted against request sizes.
const COST: [u64; 3] = [100, 10, 1];
/// The console's default staging buffer.
const STAGING: u64 = 32 << 20;

struct Lanes {
    lanes: HashMap<usize, Lane>,
    next_seq: u64,
    /// Bytes of staging held by running reads, and the limit.
    staged: u64,
    staging: u64,
    /// A start waited for staging space since the last finish.
    starved: bool,
    /// Weighted round robin: each class's virtual time (bytes x cost), and
    /// the time of the last start; within a class, the lane clock of the
    /// last start.
    clock: [u64; 3],
    now: u64,
    lane_now: [u64; 3],
    /// Results of finished reads, by sequence number, until their queue
    /// thread takes them.
    done: HashMap<u64, Result<(), HRESULT>>,
}

/// A queue's reads in the scheduler.
struct Lane {
    class: usize,
    /// Bytes started, for round robin among the lanes of a class.
    clock: u64,
    /// Submitted reads not started, and reads running.
    reads: VecDeque<(u64, Job)>,
    running: BTreeSet<u64>,
}

/// A read a worker runs.
#[derive(Clone, Copy)]
struct Job {
    shared: usize,
    entry: usize,
    size: u64,
    staging: u64,
}

fn scheduler() -> &'static Scheduler {
    static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();
    static STARTED: std::sync::Once = std::sync::Once::new();
    let scheduler = SCHEDULER.get_or_init(|| Scheduler {
        lanes: Mutex::new(Lanes {
            lanes: HashMap::new(),
            next_seq: 0,
            staged: 0,
            staging: STAGING,
            starved: false,
            clock: [0; 3],
            now: 0,
            lane_now: [0; 3],
            done: HashMap::new(),
        }),
        work: Condvar::new(),
        finished: Condvar::new(),
    });
    STARTED.call_once(|| {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        let started = (0..workers)
            .filter(|i| {
                std::thread::Builder::new()
                    .name(format!("TuxBox dstorage {i}"))
                    .spawn(|| work(scheduler))
                    .is_ok()
            })
            .count();
        log(format_args!("storage pool: {started} workers (TUXBOX_DSTORAGE_POOL=0 per-queue reads)"));
    });
    scheduler
}

fn work(scheduler: &'static Scheduler) {
    let mut lanes = lock(&scheduler.lanes);
    loop {
        let Some((lane, seq, job)) = lanes.start() else {
            lanes = scheduler.work.wait(lanes).unwrap_or_else(|e| e.into_inner());
            continue;
        };
        drop(lanes);
        // SAFETY: the queue thread that submitted the job waits in `run`
        // until its result is in, so its Shared and Entry are alive.
        let result = unsafe { run_read(&*(job.shared as *const Shared), &*(job.entry as *const Entry)) };
        lanes = lock(&scheduler.lanes);
        lanes.finish(lane, seq, &job, result);
        if std::mem::take(&mut lanes.starved) {
            scheduler.work.notify_all();
        }
        scheduler.finished.notify_all();
    }
}

impl Lanes {
    /// Starts the next read of the class with the least virtual time, from
    /// its lane that has started the fewest bytes, if the staging buffer
    /// has room for it (or nothing else holds any).
    fn start(&mut self) -> Option<(usize, u64, Job)> {
        let busy = || self.lanes.iter().filter(|(_, l)| !l.reads.is_empty());
        let (_, class) = busy().map(|(_, l)| (self.clock[l.class], l.class)).min()?;
        let (_, _, id) = busy()
            .filter(|(_, l)| l.class == class)
            .filter_map(|(&id, l)| Some((l.clock, l.reads.front()?.0, id)))
            .min()?;
        let lane = self.lanes.get_mut(&id)?;
        let &(seq, job) = lane.reads.front()?;
        if self.staged > 0 && self.staged + job.staging > self.staging {
            self.starved = true;
            return None;
        }
        lane.reads.pop_front();
        lane.running.insert(seq);
        self.staged += job.staging;
        let size = job.size.max(1);
        self.now = self.clock[class];
        self.clock[class] += size * COST[class];
        self.lane_now[class] = lane.clock;
        lane.clock += size;
        Some((id, seq, job))
    }

    fn finish(&mut self, lane: usize, seq: u64, job: &Job, result: Result<(), HRESULT>) {
        self.staged -= job.staging;
        if let Some(lane) = self.lanes.get_mut(&lane) {
            lane.running.remove(&seq);
        }
        self.done.insert(seq, result);
    }

    /// Moves a queue's reads to the workers: their sequence numbers.
    fn submit(&mut self, id: usize, class: usize, jobs: Vec<Job>) -> Vec<u64> {
        // A class that had nothing waiting starts from now, not from the
        // credit it left behind.
        if !self.lanes.values().any(|l| l.class == class && !l.reads.is_empty()) {
            self.clock[class] = self.clock[class].max(self.now);
        }
        let lane = self.lanes.entry(id).or_insert_with(|| Lane { class, clock: 0, reads: VecDeque::new(), running: BTreeSet::new() });
        if lane.reads.is_empty() {
            lane.clock = lane.clock.max(self.lane_now[class]);
        }
        let mut seqs = Vec::with_capacity(jobs.len());
        for job in jobs {
            let seq = self.next_seq;
            self.next_seq += 1;
            lane.reads.push_back((seq, job));
            seqs.push(seq);
        }
        seqs
    }
}

/// Staging bytes a read holds while it runs: what it buffers between the
/// file and the destination. Raw reads go straight to their destination,
/// memory sources need none (as on the console).
fn staging_bytes(read: &Read) -> u64 {
    let buffered = read.options & ZLIB != 0 || bcpack_mode(read.options) != 0 || read.options & DEST_PAGES != 0;
    if buffered && !matches!(read.source, Source::Memory(_)) {
        read.size as u64
    } else {
        0
    }
}

/// Runs a queue's ready reads on the pool and waits for all of them: their
/// results, in order.
pub(super) fn run(shared: &Shared, batch: &[Entry]) -> Vec<Result<(), HRESULT>> {
    let scheduler = scheduler();
    let jobs: Vec<Job> = batch
        .iter()
        .map(|entry| {
            let (size, staging) = match entry {
                Entry::Read(read) => (read.capacity.max(read.size) as u64, staging_bytes(read)),
                _ => (0, 0),
            };
            Job { shared: shared as *const Shared as usize, entry: entry as *const Entry as usize, size, staging }
        })
        .collect();
    let id = shared as *const Shared as usize;
    let class = shared.priority.min(2);
    let mut lanes = lock(&scheduler.lanes);
    let seqs = lanes.submit(id, class, jobs);
    scheduler.work.notify_all();
    while !seqs.iter().all(|s| lanes.done.contains_key(s)) {
        lanes = scheduler.finished.wait(lanes).unwrap_or_else(|e| e.into_inner());
    }
    let results = seqs.iter().map(|s| lanes.done.remove(s).unwrap_or(Err(E_FAIL))).collect();
    if lanes.lanes.get(&id).is_some_and(|l| l.reads.is_empty() && l.running.is_empty()) {
        lanes.lanes.remove(&id);
    }
    results
}
