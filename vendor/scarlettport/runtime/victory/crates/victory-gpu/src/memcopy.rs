//! Switches and counters of the copy path's cheaper modes (branch
//! fx-mem-copy), each off by default and timed on a frame line of its own
//! ("memcopy per frame: ..."), so that a pair of runs with and without a
//! switch tells what it saved:
//!
//! - `VICTORY_KNOWN_WRITES=1`: the title's memory-to-memory copies
//!   (`CopyMemoryToMemoryX`, run at submission) say where they write. The
//!   submission's one write-watch asking ([`crate::watch::once`]) then
//!   learns those pages as written and stands, where it ended at every such
//!   copy and each later flush asked the kernel again; a submission sent
//!   ahead is held back by such a copy only if it touches what the flights
//!   write, as for the title's buffer copies, not always.
//! - `VICTORY_COPY_BACK_ONLY_READBACK=1` (`crate::readback`): the ranges
//!   shaders write go back to title memory only for the title's read-back
//!   buffers; counted here (ranges and bytes kept on the card).
//! - `VICTORY_TEXTURE_HASH_EVERY=n`: a guarded texture nothing was seen to
//!   write is hashed once every `n` frames as a check (8, as before; 0
//!   never). `VICTORY_FAST_HASH=1`: the change hash of texture memory is
//!   [`lane_hash`], not BLAKE3 (no attacker here, only a change to see).
//! - `VICTORY_TEXTURE_OVERLAP=1`: a submission's texture refreshes run on
//!   threads of their own while the submitting thread brings buffer copies
//!   up to date; they are waited for before any step that writes title
//!   memory and before the lists go to the card.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v.trim() == "1")
}

/// `VICTORY_KNOWN_WRITES=1`.
pub(crate) fn known_writes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| flag("VICTORY_KNOWN_WRITES"))
}

/// `VICTORY_TEXTURE_OVERLAP=1`.
pub(crate) fn texture_overlap() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| flag("VICTORY_TEXTURE_OVERLAP"))
}

/// `VICTORY_FAST_HASH=1`.
pub(crate) fn fast_hash() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| flag("VICTORY_FAST_HASH"))
}

/// `VICTORY_TEXTURE_HASH_EVERY` (default 8; 0 never).
pub(crate) fn hash_every() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| hash_every_of(std::env::var("VICTORY_TEXTURE_HASH_EVERY").ok().as_deref()))
}

fn hash_every_of(value: Option<&str>) -> u64 {
    value.and_then(|v| v.trim().parse().ok()).unwrap_or(8)
}

/// Whether a guarded texture at `start` that nothing was seen to write
/// takes its turn for a check hash at `frame` ([`hash_every`]).
pub(crate) fn check_turn(frame: u64, start: usize, every: u64) -> bool {
    every != 0 && frame.wrapping_add((start >> 16) as u64) % every == 0
}

/// Counters since the last line.
#[derive(Clone, Copy)]
pub(crate) enum M {
    /// Title memory-to-memory copies run at submission: with a known
    /// destination the asking stood; without, it ended.
    RunsKept,
    RunsEnded,
    /// Late submissions a memory-to-memory copy did not hold back
    /// (`VICTORY_KNOWN_WRITES`).
    RunsNotBlocking,
    /// Ranges shaders write that stay on the card (not a read-back
    /// buffer), and their bytes; ranges that go back.
    BackKept,
    BackKeptBytes,
    BackGoes,
    /// Of what stayed on the card: ranges the title wrote over afterwards
    /// (its bytes won, the page goes back from then on) and their bytes;
    /// bytes put into title memory for a reader there (`crate::shadow`'s
    /// owned ranges).
    OwnConflicts,
    OwnConflictBytes,
    OwnPut,
    /// Texture change hashes: count, bytes, microseconds (all threads).
    Hashes,
    HashBytes,
    HashUs,
    /// Check hashes left out (`VICTORY_TEXTURE_HASH_EVERY`).
    ChecksSkipped,
    /// Texture refreshes run beside the submitting thread, their time on
    /// those threads, and the time the submitting thread waited for them.
    Overlapped,
    OverlapUs,
    OverlapWaitUs,
}
const COUNTERS: usize = 16;
static COUNT: [AtomicU64; COUNTERS] = [const { AtomicU64::new(0) }; COUNTERS];

pub(crate) fn add(m: M, n: u64) {
    COUNT[m as usize].fetch_add(n, Relaxed);
}

/// Logs the counters per frame (after the frame-rate line), if any switch
/// of this module is on.
pub(crate) fn log(frames: f64) {
    let c: Vec<f64> = COUNT
        .iter()
        .map(|a| a.swap(0, Relaxed) as f64 / frames.max(1.0))
        .collect();
    let any = known_writes()
        || texture_overlap()
        || fast_hash()
        || hash_every() != 8
        || crate::readback::enabled();
    if !any {
        return;
    }
    let g = |m: M| c[m as usize];
    let (owned, owners) = crate::shadow::owned_now();
    tracing::info!(
        "memcopy per frame: title memory copies at submission {:.0} kept the asking, {:.0} ended it, \
         {:.0} late submissions not held back by them (VICTORY_KNOWN_WRITES={}); shader-written ranges \
         {:.0} kept on the card {:.1} MB, {:.0} copied back (VICTORY_COPY_BACK_ONLY_READBACK={}, {} read-back \
         buffers; {:.1} MB held in {} copies, {:.3} MB put into title memory for readers, {:.1} title writes \
         over them {:.3} MB); texture hashes {:.0} {:.1} MB {:.2} ms (VICTORY_FAST_HASH={}), {:.0} check hashes left \
         out (VICTORY_TEXTURE_HASH_EVERY={}); texture refreshes beside the submitting thread {:.0}, \
         {:.2} ms on their threads, {:.2} ms waited for (VICTORY_TEXTURE_OVERLAP={})",
        g(M::RunsKept),
        g(M::RunsEnded),
        g(M::RunsNotBlocking),
        u8::from(known_writes()),
        g(M::BackKept),
        g(M::BackKeptBytes) / 1e6,
        g(M::BackGoes),
        u8::from(crate::readback::enabled()),
        crate::readback::tagged(),
        owned as f64 / 1e6,
        owners,
        g(M::OwnPut) / 1e6,
        g(M::OwnConflicts),
        g(M::OwnConflictBytes) / 1e6,
        g(M::Hashes),
        g(M::HashBytes) / 1e6,
        g(M::HashUs) / 1e3,
        u8::from(fast_hash()),
        g(M::ChecksSkipped),
        hash_every(),
        g(M::Overlapped),
        g(M::OverlapUs) / 1e3,
        g(M::OverlapWaitUs) / 1e3,
        u8::from(texture_overlap()),
    );
}

/// The change hash of texture memory: BLAKE3, or [`lane_hash`] with
/// `VICTORY_FAST_HASH=1`. Only ever compared with hashes made the same way
/// in the same run.
pub(crate) fn content_hash(bytes: &[u8]) -> blake3::Hash {
    let started = std::time::Instant::now();
    let hash = if fast_hash() {
        blake3::Hash::from(lane_hash(bytes))
    } else {
        blake3::hash(bytes)
    };
    add(M::Hashes, 1);
    add(M::HashBytes, bytes.len() as u64);
    add(M::HashUs, started.elapsed().as_micros() as u64);
    hash
}

const LANES: usize = 8;
const K: [u64; LANES] = [
    0x9e37_79b9_7f4a_7c15,
    0xc2b2_ae3d_27d4_eb4f,
    0x1656_67b1_9e37_79f9,
    0x85eb_ca77_c2b2_ae63,
    0x27d4_eb2f_1656_67c5,
    0xff51_afd7_ed55_8ccd,
    0xc4ce_b9fe_1a85_ec53,
    0xd6e8_feb8_6659_fd93,
];

/// A 256-bit hash of `bytes` for telling whether memory changed, at memory
/// speed: eight independent lanes of 64-bit words, each step of a lane
/// `x -> (rotl(x, 23) ^ w) * k` with `k` odd. For a given state the step is
/// a bijection of the word and of the state, so a change of any one word
/// always changes its lane's final state; a change of several words is
/// missed only if their effects cancel exactly (about 2^-64 for data not
/// made to do so). The length and the tail bytes are mixed in last.
pub(crate) fn lane_hash(bytes: &[u8]) -> [u8; 32] {
    let mut lane = K;
    let mut blocks = bytes.chunks_exact(8 * LANES);
    for block in &mut blocks {
        for (i, l) in lane.iter_mut().enumerate() {
            let w = u64::from_le_bytes(block[8 * i..8 * i + 8].try_into().unwrap_or([0; 8]));
            *l = (l.rotate_left(23) ^ w).wrapping_mul(K[i] | 1);
        }
    }
    let rest = blocks.remainder();
    for (j, word) in rest.chunks(8).enumerate() {
        let mut w = [0u8; 8];
        w[..word.len()].copy_from_slice(word);
        let w = u64::from_le_bytes(w);
        lane[j] = (lane[j].rotate_left(23) ^ w).wrapping_mul(K[j] | 1);
    }
    lane[0] ^= bytes.len() as u64;
    // Finish: every lane into every output word.
    let mut out = [0u8; 32];
    for o in 0..4 {
        let mut x = (o as u64).wrapping_mul(K[7]) ^ bytes.len() as u64;
        for (i, l) in lane.iter().enumerate() {
            x = (x ^ l.rotate_left((7 * i + 13 * o) as u32 % 64)).wrapping_mul(K[(i + o) % LANES] | 1);
            x ^= x >> 29;
        }
        out[8 * o..8 * o + 8].copy_from_slice(&x.to_le_bytes());
    }
    out
}

/// Ranges `[start, end)` of memory, each with a count (one buffer made and
/// let go twice at the same place counts twice); overlaps are allowed.
#[derive(Default)]
pub(crate) struct RangeSet {
    ranges: std::collections::BTreeMap<(u64, u64), u32>,
    /// The longest range ever held: an overlap query looks back that far.
    longest: u64,
}

impl RangeSet {
    pub(crate) const fn new() -> RangeSet {
        RangeSet {
            ranges: std::collections::BTreeMap::new(),
            longest: 0,
        }
    }

    pub(crate) fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        *self.ranges.entry((start, end)).or_insert(0) += 1;
        self.longest = self.longest.max(end - start);
    }

    /// Takes one count of `[start, end)` out; false if it was not held.
    pub(crate) fn remove(&mut self, start: u64, end: u64) -> bool {
        match self.ranges.get_mut(&(start, end)) {
            Some(n) if *n > 1 => {
                *n -= 1;
                true
            }
            Some(_) => {
                self.ranges.remove(&(start, end));
                true
            }
            None => false,
        }
    }

    /// Takes one count of the range starting at `start` out, whatever its
    /// end (a buffer let go knows where it starts); false if none.
    pub(crate) fn remove_at(&mut self, start: u64) -> bool {
        let found = self.ranges.range((start, 0)..=(start, u64::MAX)).next().map(|(&k, _)| k);
        found.is_some_and(|(s, e)| self.remove(s, e))
    }

    /// Whether a held range meets `[start, end)`.
    pub(crate) fn overlaps(&self, start: u64, end: u64) -> bool {
        if start >= end {
            return false;
        }
        let from = start.saturating_sub(self.longest);
        self.ranges
            .range((from, 0)..(end, 0))
            .any(|(&(a, b), _)| a < end && start < b)
    }

    pub(crate) fn len(&self) -> usize {
        self.ranges.len()
    }
}

/// Work run beside the submitting thread and waited for as a group
/// ([`texture_overlap`]). Its threads are its own: a job may itself spread
/// work over `crate::shadow`'s threads (big textures are detiled in bands).
pub(crate) struct Group {
    left: Arc<(Mutex<usize>, Condvar)>,
}

struct Pool {
    jobs: Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send>>>,
    more: Condvar,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    static STARTED: std::sync::Once = std::sync::Once::new();
    let pool = POOL.get_or_init(|| Pool {
        jobs: Mutex::new(std::collections::VecDeque::new()),
        more: Condvar::new(),
    });
    STARTED.call_once(|| {
        let n = std::thread::available_parallelism().map_or(2, |n| n.get() / 6).clamp(1, 3);
        for i in 0..n {
            let _ = std::thread::Builder::new().name(format!("victory-texprep-{i}")).spawn(move || {
                let pool = POOL.get().expect("made above");
                loop {
                    let job = {
                        let mut jobs = pool.jobs.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(job) = jobs.pop_front() {
                                break job;
                            }
                            jobs = pool.more.wait(jobs).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    // A job's panic is its own: the thread goes on.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                }
            });
        }
    });
    pool
}

impl Default for Group {
    fn default() -> Group {
        Group {
            left: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }
}

impl Group {
    /// Runs `job` on the pool's threads.
    pub(crate) fn spawn(&self, job: impl FnOnce() + Send + 'static) {
        *self.left.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        let left = self.left.clone();
        let pool = pool();
        pool.jobs.lock().unwrap_or_else(|e| e.into_inner()).push_back(Box::new(move || {
            // Counted down even if the job panics.
            struct Done(Arc<(Mutex<usize>, Condvar)>);
            impl Drop for Done {
                fn drop(&mut self) {
                    let mut n = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
                    *n -= 1;
                    if *n == 0 {
                        self.0.1.notify_all();
                    }
                }
            }
            let _done = Done(left);
            job();
        }));
        pool.more.notify_one();
    }

    /// Whether jobs are still to finish.
    pub(crate) fn busy(&self) -> bool {
        *self.left.0.lock().unwrap_or_else(|e| e.into_inner()) > 0
    }

    /// Returns when every job spawned so far has run. While waiting this
    /// thread runs jobs still queued (any group's: they are all
    /// independent and all waited for by someone).
    pub(crate) fn join(&self) {
        loop {
            if !self.busy() {
                return;
            }
            let job = pool().jobs.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            match job {
                Some(job) => {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                }
                None => break,
            }
        }
        let mut n = self.left.0.lock().unwrap_or_else(|e| e.into_inner());
        while *n > 0 {
            n = self.left.1.wait(n).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lane_hash_sees_every_one_byte_change() {
        let mut data: Vec<u8> = (0..4099u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let base = lane_hash(&data);
        assert_eq!(base, lane_hash(&data), "the same bytes hash the same");
        for at in [0, 1, 7, 8, 63, 64, 65, 511, 2048, 4095, 4096, 4098] {
            let was = data[at];
            for flip in [1u8, 0x80, 0xff] {
                data[at] = was ^ flip;
                assert_ne!(lane_hash(&data), base, "byte {at} ^ {flip:#x}");
            }
            data[at] = was;
        }
        assert_eq!(lane_hash(&data), base);
    }

    #[test]
    fn the_lane_hash_tells_lengths_and_zeros_apart() {
        let zeros = vec![0u8; 256];
        let mut seen = std::collections::HashSet::new();
        for n in 0..=256 {
            assert!(seen.insert(lane_hash(&zeros[..n])), "length {n}");
        }
    }

    #[test]
    fn the_lane_hash_sees_swapped_words() {
        // Two lanes' words exchanged, and two words of one lane exchanged.
        let mut a = vec![0u8; 1024];
        a[0] = 1;
        a[8] = 2;
        let mut b = a.clone();
        b.swap(0, 8);
        assert_ne!(lane_hash(&a), lane_hash(&b));
        let mut c = a.clone();
        c[64] = 1;
        c[0] = 0;
        assert_ne!(lane_hash(&a), lane_hash(&c));
    }

    #[test]
    fn a_range_set_finds_overlaps_only() {
        let mut s = RangeSet::new();
        s.insert(0x1000, 0x2000);
        s.insert(0x10_0000, 0x50_0000);
        assert!(s.overlaps(0x1fff, 0x3000));
        assert!(!s.overlaps(0x2000, 0x3000), "end is exclusive");
        assert!(!s.overlaps(0x0, 0x1000), "start is inclusive only from the left");
        assert!(s.overlaps(0x4f_ffff, 0x60_0000));
        assert!(s.overlaps(0x20_0000, 0x20_0001), "inside a long range far past its start");
        assert!(!s.overlaps(0x50_0000, 0x60_0000));
        assert!(!s.overlaps(5, 5), "empty asks meet nothing");
    }

    #[test]
    fn a_range_set_counts_and_lets_go() {
        let mut s = RangeSet::new();
        s.insert(100, 200);
        s.insert(100, 200);
        s.insert(100, 150);
        assert_eq!(s.len(), 2);
        assert!(s.remove_at(100));
        assert!(s.remove_at(100));
        assert!(s.overlaps(120, 130), "one count of one range is left");
        assert!(s.remove_at(100));
        assert!(!s.overlaps(0, 1000));
        assert!(!s.remove_at(100));
        assert!(!s.remove(1, 2));
        s.insert(7, 7);
        assert_eq!(s.len(), 0, "empty ranges are not held");
    }

    #[test]
    fn check_turns_come_once_in_every_n_frames() {
        let start = 0x1234_0000;
        let turns = (0..64).filter(|&f| check_turn(f, start, 8)).count();
        assert_eq!(turns, 8);
        assert!((0..64).all(|f| !check_turn(f, start, 0)), "0 never checks");
        assert!((0..5).all(|f| check_turn(f, start, 1)));
        assert_eq!(hash_every_of(None), 8);
        assert_eq!(hash_every_of(Some(" 32 ")), 32);
        assert_eq!(hash_every_of(Some("x")), 8);
    }

    #[test]
    fn a_group_waits_for_all_its_jobs() {
        let group = Group::default();
        let sum = Arc::new(AtomicU64::new(0));
        for i in 1..=100u64 {
            let sum = sum.clone();
            group.spawn(move || {
                std::thread::sleep(std::time::Duration::from_micros(50));
                sum.fetch_add(i, Relaxed);
            });
        }
        group.join();
        assert_eq!(sum.load(Relaxed), 5050);
        assert!(!group.busy());
        // Joined again with nothing spawned: returns at once.
        group.join();
    }

    #[test]
    fn a_job_that_panics_is_still_counted_done() {
        let group = Group::default();
        group.spawn(|| panic!("a job's panic"));
        group.join();
        assert!(!group.busy());
    }
}
