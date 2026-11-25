//! Submission scheduling logic that needs no GPU: which earlier submission
//! a CPU access must wait for, and which of a title's barriers the host
//! list still needs. Used by `victory-gpu` behind `VICTORY_SYNC_PRECISE`
//! and `VICTORY_BARRIER_MERGE`; kept here so it is tested on every host.

/// `[from, to)` byte ranges, in order, joined where they overlap or touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ranges(Vec<(usize, usize)>);

impl Ranges {
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Adds `more` (empty ranges are left out).
    pub fn add(&mut self, more: impl IntoIterator<Item = (usize, usize)>) {
        self.0.extend(more.into_iter().filter(|r| r.0 < r.1));
        self.0.sort_unstable();
        let mut out: Vec<(usize, usize)> = Vec::with_capacity(self.0.len());
        for &(from, to) in &self.0 {
            match out.last_mut() {
                Some(last) if from <= last.1 => last.1 = last.1.max(to),
                _ => out.push((from, to)),
            }
        }
        self.0 = out;
    }

    /// Whether `[from, to)` shares a byte with one of the ranges (one that
    /// only touches an end does not).
    pub fn touches(&self, from: usize, to: usize) -> bool {
        if from >= to {
            return false;
        }
        let at = self.0.partition_point(|r| r.1 <= from);
        self.0.get(at).is_some_and(|r| r.0 < to)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[(usize, usize)] {
        &self.0
    }
}

/// What submissions on the card may still access, by the fence value each
/// one signals when it is done (values grow with submission order on one
/// queue).
///
/// A CPU access to memory asks [`InFlight::wait_for`]: the one value to
/// wait for so that no unfinished submission touches what it accesses, or
/// nothing. Waiting for exactly that value, not for the card's last one,
/// leaves later submissions running.
#[derive(Debug, Default)]
pub struct InFlight {
    /// (fence value, ranges), in the order recorded.
    entries: Vec<(u64, Ranges)>,
}

impl InFlight {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Submission `fence` accesses `ranges` until it is done.
    pub fn record(&mut self, fence: u64, ranges: impl IntoIterator<Item = (usize, usize)>) {
        let mut set = Ranges::new();
        set.add(ranges);
        if set.is_empty() {
            return;
        }
        match self.entries.iter_mut().find(|(f, _)| *f == fence) {
            Some((_, r)) => r.add(set.0),
            None => self.entries.push((fence, set)),
        }
    }

    /// The card has reached `completed`: submissions at or below it are
    /// done.
    pub fn retire(&mut self, completed: u64) {
        self.entries.retain(|(f, _)| *f > completed);
    }

    /// The highest fence value of an unfinished submission (above
    /// `completed`) that accesses part of `[from, to)`, if any.
    pub fn wait_for(&self, from: usize, to: usize, completed: u64) -> Option<u64> {
        self.entries
            .iter()
            .filter(|(f, r)| *f > completed && r.touches(from, to))
            .map(|(f, _)| *f)
            .max()
    }

    /// Submissions still tracked.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One barrier of a title's call, as far as merging needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    /// A state transition of a resource's subresource (`u32::MAX`: all).
    Transition { resource: usize, subresource: u32, before: u32, after: u32 },
    /// An unordered access barrier on one resource, or on all (`0`).
    Uav { resource: usize },
    /// Anything else (aliasing): always kept, in place.
    Other,
}

/// Whether a D3D12 resource state (bits) only reads: vertex/constant,
/// index, depth read, shader resource (both), indirect argument, copy
/// source, shading-rate source. `COMMON` (0) and every writing state are
/// not.
pub fn read_only(state: u32) -> bool {
    const READS: u32 = 0x1 | 0x2 | 0x20 | 0x40 | 0x80 | 0x200 | 0x800 | 0x0100_0000;
    state != 0 && state & !READS == 0
}

/// What a host list does with one call's barriers ([`plan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// For each barrier given: kept, with the states it now has (a
    /// transition folded with a later one of the same subresource takes its
    /// `after`), or dropped (`None`).
    pub keep: Vec<Option<Barrier>>,
    /// Whether an unordered access barrier on all resources is added at the
    /// end (the call asked for one through `global` and has none kept).
    pub add_global: bool,
    /// Whether unordered access writes are still unordered after the call
    /// (the list's flag for its next barrier).
    pub pending: bool,
}

/// Merges one call's `barriers`. `global`: the call also needs every
/// earlier shader write finished (a barrier on a buffer, which the host
/// cannot name). `pending`: the list recorded work that may write through
/// unordered access since its last barrier on all resources.
///
/// - Without pending writes no unordered access barrier is needed: there is
///   nothing to wait for.
/// - With them, one barrier on all resources at most; it covers the
///   barriers on single resources, which are then dropped.
/// - Transitions of one subresource within the call fold into one (from
///   the first `before` to the last `after`); a fold back to where it
///   started is dropped.
pub fn plan(barriers: &[Barrier], global: bool, pending: bool) -> Plan {
    let mut keep: Vec<Option<Barrier>> = barriers.iter().copied().map(Some).collect();
    // Transitions: the later one of a subresource folds into the first.
    for i in 0..keep.len() {
        let Some(Barrier::Transition { resource, subresource, before, after }) = keep[i] else {
            continue;
        };
        let mut after_now = after;
        let mut folded: Vec<usize> = Vec::new();
        for (j, later) in keep.iter().enumerate().skip(i + 1) {
            if let Some(Barrier::Transition { resource: r, subresource: s, before: b, after: a }) = *later
                && r == resource
                && s == subresource
                && b == after_now
            {
                after_now = a;
                folded.push(j);
            }
        }
        if folded.is_empty() {
            continue;
        }
        // Back where it started: nothing to do for a read-only state; from
        // a state that writes, the pair also orders the writes before it
        // against those after it, so it stays as it was.
        if before == after_now && !read_only(before) {
            continue;
        }
        for j in folded {
            keep[j] = None;
        }
        keep[i] = (before != after_now).then_some(Barrier::Transition {
            resource,
            subresource,
            before,
            after: after_now,
        });
    }
    let uav = |b: &Option<Barrier>| matches!(b, Some(Barrier::Uav { .. }));
    let all = |b: &Option<Barrier>| matches!(b, Some(Barrier::Uav { resource: 0 }));
    if !pending {
        for b in keep.iter_mut().filter(|b| uav(b)) {
            *b = None;
        }
        return Plan { keep, add_global: false, pending: false };
    }
    let wants_all = global || keep.iter().any(all);
    if wants_all {
        // The first barrier on all resources stays where the title put it.
        let first = keep.iter().position(all);
        for (i, b) in keep.iter_mut().enumerate() {
            if uav(b) && Some(i) != first {
                *b = None;
            }
        }
        return Plan { keep, add_global: first.is_none(), pending: false };
    }
    // Single resources: each once.
    let mut seen: Vec<usize> = Vec::new();
    for b in keep.iter_mut() {
        if let Some(Barrier::Uav { resource }) = *b {
            if seen.contains(&resource) {
                *b = None;
            } else {
                seen.push(resource);
            }
        }
    }
    Plan { keep, add_global: false, pending: true }
}

/// Something a dispatch reads or writes, as far as ordering needs it:
/// title memory by address (`[from, to)`, buffers through their host
/// copies) or a whole object (a texture, by the title's pointer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Bytes(usize, usize),
    Object(usize),
}

/// What one dispatch accesses. `unknown`: it may access anything (title
/// memory by address, a view picked by an index the shader computes, a
/// texture whose memory other textures share); it then conflicts with
/// everything pending.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub reads: Vec<Key>,
    pub writes: Vec<Key>,
    pub unknown: bool,
}

impl Access {
    pub fn read(&mut self, key: Key) {
        self.reads.push(key);
    }

    pub fn write(&mut self, key: Key) {
        self.writes.push(key);
    }

    /// The writes become reads: the shaders cannot write (their unordered
    /// views are only read).
    pub fn read_only(&mut self) {
        let writes = std::mem::take(&mut self.writes);
        self.reads.extend(writes);
    }
}

/// The keys accessed since a list's last barrier on all resources.
#[derive(Clone, Debug, Default)]
struct KeySet {
    bytes: Ranges,
    objects: Vec<usize>,
    /// Everything (an access not known, or work not tracked).
    all: bool,
}

/// Past this many objects a set is "everything": cheap and safe.
const MOST_OBJECTS: usize = 256;

impl KeySet {
    fn touches(&self, key: Key) -> bool {
        self.all
            || match key {
                Key::Bytes(from, to) => self.bytes.touches(from, to),
                Key::Object(o) => self.objects.contains(&o),
            }
    }

    fn touches_any(&self, keys: &[Key]) -> bool {
        keys.iter().any(|&k| self.touches(k))
    }

    fn add(&mut self, keys: &[Key]) {
        if self.all {
            return;
        }
        let mut bytes = Vec::new();
        for &k in keys {
            match k {
                Key::Bytes(from, to) => bytes.push((from, to)),
                Key::Object(o) if !self.objects.contains(&o) => self.objects.push(o),
                Key::Object(_) => {}
            }
        }
        if !bytes.is_empty() {
            self.bytes.add(bytes);
        }
        if self.objects.len() > MOST_OBJECTS {
            self.set_all();
        }
    }

    fn set_all(&mut self) {
        *self = Self { all: true, ..Self::default() };
    }

    fn is_empty(&self) -> bool {
        !self.all && self.bytes.is_empty() && self.objects.is_empty()
    }
}

/// Whether access `a` (later) and one of `keys` accessed by `earlier`
/// ([`KeySet`]s of reads and writes) must be ordered: a write after a read
/// or a write, or a read after a write.
fn conflict(reads: &KeySet, writes: &KeySet, a: &Access) -> bool {
    if a.unknown {
        return !(reads.is_empty() && writes.is_empty());
    }
    writes.touches_any(&a.reads) || writes.touches_any(&a.writes) || reads.touches_any(&a.writes)
}

/// Per host list: which of the title's waits for earlier shader work a
/// direct dispatch actually needs (`VICTORY_BARRIER_HAZARD`).
///
/// A title barrier asking for earlier shader work to finish is not recorded
/// where it stands: it is held ([`Hazards::request_all`],
/// [`Hazards::request`]) until a dispatch comes whose accesses
/// ([`Access`]) conflict with something accessed since the last barrier
/// on all resources; then one barrier on all resources goes in before
/// that dispatch ([`Hazards::op`] says so). Dispatches that do not
/// conflict run without it, and their accesses join the pending ones (the
/// barrier, if one comes later, orders them too: only ever more order than
/// asked for). Any other work (draws, copies, indirect execution, the
/// list's end) records the held barrier first if anything is pending
/// ([`Hazards::flush`]) and counts as accessing everything.
///
/// The console runs dispatches back to back without waiting unless told
/// to, so a title never depends on order it did not ask for: without a
/// held request no barrier is ever added, as before.
#[derive(Clone, Debug)]
pub struct Hazards {
    reads: KeySet,
    writes: KeySet,
    /// The title asked for all earlier shader work to finish.
    wanted_all: bool,
    /// The title asked for earlier work on these to finish (a barrier on
    /// one buffer or texture).
    wanted: Vec<Key>,
}

impl Default for Hazards {
    fn default() -> Self {
        Self::new()
    }
}

impl Hazards {
    /// A list's start: what lists before it in the same submission did is
    /// not known, so everything is pending.
    pub fn new() -> Self {
        let mut h = Self { reads: KeySet::default(), writes: KeySet::default(), wanted_all: false, wanted: Vec::new() };
        h.reads.set_all();
        h.writes.set_all();
        h
    }

    /// Whether a request is held.
    pub fn held(&self) -> bool {
        self.wanted_all || !self.wanted.is_empty()
    }

    /// The title asks for every earlier shader access to finish.
    pub fn request_all(&mut self) {
        self.wanted_all = true;
    }

    /// The title asks for earlier accesses of `key` to finish.
    pub fn request(&mut self, key: Key) {
        if !self.wanted_all && !self.wanted.contains(&key) {
            self.wanted.push(key);
        }
        if self.wanted.len() > MOST_OBJECTS {
            self.wanted.clear();
            self.wanted_all = true;
        }
    }

    /// A dispatch accessing `a` is about to be recorded. True: a barrier on
    /// all resources must be recorded before it (and is taken as done).
    pub fn op(&mut self, a: &Access) -> bool {
        let needed = self.held()
            && (if self.wanted_all {
                conflict(&self.reads, &self.writes, a)
            } else {
                self.wanted.iter().any(|&k| {
                    let later_reads = a.unknown || a.reads.iter().any(|&r| overlaps(r, k));
                    let later_writes = a.unknown || a.writes.iter().any(|&w| overlaps(w, k));
                    (later_writes && (self.reads.touches(k) || self.writes.touches(k)))
                        || (later_reads && self.writes.touches(k))
                })
            });
        if needed {
            self.done();
        }
        if a.unknown {
            self.reads.set_all();
            self.writes.set_all();
        } else {
            self.reads.add(&a.reads);
            self.writes.add(&a.writes);
        }
        needed
    }

    /// Work not tracked comes next. True: the held barrier must be recorded
    /// now. Afterwards everything is pending (that work may access
    /// anything).
    pub fn flush(&mut self) -> bool {
        let needed = self.held() && !(self.reads.is_empty() && self.writes.is_empty());
        self.done();
        self.reads.set_all();
        self.writes.set_all();
        needed
    }

    /// A barrier on all resources was recorded: nothing pending, nothing
    /// held.
    pub fn done(&mut self) {
        self.reads = KeySet::default();
        self.writes = KeySet::default();
        self.wanted_all = false;
        self.wanted.clear();
    }
}

fn overlaps(a: Key, b: Key) -> bool {
    match (a, b) {
        (Key::Bytes(f1, t1), Key::Bytes(f2, t2)) => f1 < t2 && f2 < t1,
        (Key::Object(x), Key::Object(y)) => x == y,
        _ => false,
    }
}

/// Enhanced barrier access bits (`D3D12_BARRIER_ACCESS`) that write.
pub const ACCESS_WRITES: u32 = 0x8 // render target
    | 0x10 // unordered access
    | 0x20 // depth-stencil write
    | 0x100 // stream output
    | 0x400 // copy dest
    | 0x1000 // resolve dest
    | 0x8000; // ray-tracing structure write
/// `D3D12_BARRIER_ACCESS_UNORDERED_ACCESS`.
pub const ACCESS_UNORDERED: u32 = 0x10;
/// `D3D12_BARRIER_ACCESS_NO_ACCESS`.
pub const ACCESS_NONE: u32 = 0x8000_0000;
/// Every `D3D12_BARRIER_ACCESS` bit known (video ones included).
pub const ACCESS_KNOWN: u32 = 0x00ff_ffff | ACCESS_NONE;
/// Every `D3D12_BARRIER_SYNC` bit known (video ones and `SPLIT` included).
pub const SYNC_KNOWN: u32 = 0x01ff_ffff | 0x1000_0000 | 0x8000_0000;

/// What one enhanced barrier (a global, buffer or texture one: sync and
/// access before and after) asks of the host, taken literally
/// (`VICTORY_BARRIER_LITERAL`): whether earlier shader work must finish
/// before later shader work. `None`: the bits are not ones D3D12 knows (the
/// console's layout is not the one assumed): the caller does as before.
///
/// - Nothing before or nothing after (`SYNC_NONE`, `NO_ACCESS`): nothing
///   to order.
/// - Reads before and reads after: nothing to order.
/// - A write on either side (or `COMMON`, which may be anything): wait.
pub fn barrier_waits(sync_before: u32, sync_after: u32, access_before: u32, access_after: u32) -> Option<bool> {
    if (sync_before | sync_after) & !SYNC_KNOWN != 0 || (access_before | access_after) & !ACCESS_KNOWN != 0 {
        return None;
    }
    if sync_before == 0 || sync_after == 0 || (access_before | access_after) & ACCESS_NONE != 0 {
        return Some(false);
    }
    let writes = |access: u32| access == 0 || access & ACCESS_WRITES != 0;
    Some(writes(access_before) || writes(access_after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_join_and_touch_only_where_they_share_bytes() {
        let mut r = Ranges::new();
        r.add([(40, 50), (10, 20), (20, 25), (45, 60), (7, 7)]);
        assert_eq!(r.as_slice(), &[(10, 25), (40, 60)]);
        for (from, to, hit) in [
            (0, 10, false),
            (0, 11, true),
            (24, 25, true),
            (25, 40, false),
            (25, 41, true),
            (59, 200, true),
            (60, 100, false),
            (30, 30, false),
        ] {
            assert_eq!(r.touches(from, to), hit, "{from}..{to}");
        }
        assert!(!Ranges::new().touches(0, usize::MAX));
    }

    #[test]
    fn a_read_waits_for_the_last_unfinished_submission_that_touches_it() {
        let mut f = InFlight::new();
        f.record(5, [(0x1000, 0x2000)]);
        f.record(6, [(0x8000, 0x9000)]);
        f.record(7, [(0x1800, 0x1900), (0x3000, 0x4000)]);
        // Untouched memory: no wait at all.
        assert_eq!(f.wait_for(0x2000, 0x3000, 0), None);
        assert_eq!(f.wait_for(0x5000, 0x8000, 0), None);
        // Only the submission that touches it, not the card's last one.
        assert_eq!(f.wait_for(0x1000, 0x1100, 0), Some(5));
        assert_eq!(f.wait_for(0x8800, 0x8801, 0), Some(6));
        // Two touch it: the later one (the queue runs them in order).
        assert_eq!(f.wait_for(0x1000, 0x1801, 0), Some(7));
        // Done submissions no longer count, retired or not.
        assert_eq!(f.wait_for(0x1000, 0x1801, 7), None);
        assert_eq!(f.wait_for(0x1000, 0x1801, 6), Some(7));
        f.retire(5);
        assert_eq!(f.len(), 2);
        assert_eq!(f.wait_for(0x1000, 0x1100, 0), None);
        f.retire(7);
        assert!(f.is_empty());
    }

    #[test]
    fn one_submission_recorded_twice_is_one_entry() {
        let mut f = InFlight::new();
        f.record(3, [(0, 10)]);
        f.record(3, [(20, 30)]);
        f.record(4, []);
        assert_eq!(f.len(), 1);
        assert_eq!(f.wait_for(25, 26, 0), Some(3));
    }

    const UAV_ALL: Barrier = Barrier::Uav { resource: 0 };

    fn t(resource: usize, before: u32, after: u32) -> Barrier {
        Barrier::Transition { resource, subresource: u32::MAX, before, after }
    }

    #[test]
    fn without_pending_writes_no_unordered_barrier_is_kept() {
        let p = plan(&[UAV_ALL, Barrier::Uav { resource: 9 }, t(1, 8, 64)], true, false);
        assert_eq!(p.keep, vec![None, None, Some(t(1, 8, 64))]);
        assert!(!p.add_global);
        assert!(!p.pending);
    }

    #[test]
    fn a_buffer_barrier_adds_one_barrier_on_all() {
        let p = plan(&[t(1, 8, 64)], true, true);
        assert_eq!(p.keep, vec![Some(t(1, 8, 64))]);
        assert!(p.add_global);
        assert!(!p.pending);
    }

    #[test]
    fn one_barrier_on_all_covers_the_others() {
        let p = plan(
            &[Barrier::Uav { resource: 3 }, UAV_ALL, UAV_ALL, Barrier::Uav { resource: 4 }, Barrier::Other],
            true,
            true,
        );
        assert_eq!(p.keep, vec![None, Some(UAV_ALL), None, None, Some(Barrier::Other)]);
        assert!(!p.add_global);
        assert!(!p.pending);
    }

    #[test]
    fn barriers_on_single_resources_stay_once_and_leave_writes_pending() {
        let a = Barrier::Uav { resource: 3 };
        let b = Barrier::Uav { resource: 4 };
        let p = plan(&[a, b, a], false, true);
        assert_eq!(p.keep, vec![Some(a), Some(b), None]);
        assert!(!p.add_global);
        assert!(p.pending);
    }

    #[test]
    fn transitions_of_one_subresource_fold() {
        // 1: 8 -> 64 -> 0x400 becomes 8 -> 0x400; 2: there and back from
        // a read-only state goes.
        let p = plan(&[t(1, 8, 64), t(2, 0x40, 0x800), t(1, 64, 0x400), t(2, 0x800, 0x40)], false, false);
        assert_eq!(p.keep, vec![Some(t(1, 8, 0x400)), None, None, None]);
        // There and back from a writing state (unordered access) orders the
        // writes on either side: both stay.
        let p = plan(&[t(3, 8, 64), t(3, 64, 8)], false, false);
        assert_eq!(p.keep, vec![Some(t(3, 8, 64)), Some(t(3, 64, 8))]);
        assert!(read_only(0x40 | 0x80) && !read_only(0) && !read_only(0x8) && !read_only(0x40 | 0x400));
        // Another subresource, or a chain that does not continue, stays.
        let s = Barrier::Transition { resource: 1, subresource: 0, before: 64, after: 8 };
        let p = plan(&[t(1, 8, 64), s, t(1, 4, 8)], false, false);
        assert_eq!(p.keep, vec![Some(t(1, 8, 64)), Some(s), Some(t(1, 4, 8))]);
    }

    #[test]
    fn nothing_given_nothing_added_unless_a_global_barrier_is_asked_for() {
        assert_eq!(plan(&[], false, true), Plan { keep: vec![], add_global: false, pending: true });
        assert_eq!(plan(&[], true, true), Plan { keep: vec![], add_global: true, pending: false });
        assert_eq!(plan(&[], true, false), Plan { keep: vec![], add_global: false, pending: false });
    }
}

#[cfg(test)]
mod hazard_tests {
    use super::*;

    const A: Key = Key::Bytes(0x1000, 0x2000);
    const B: Key = Key::Bytes(0x3000, 0x4000);
    const T: Key = Key::Object(0xaa);
    const U: Key = Key::Object(0xbb);

    fn access(reads: &[Key], writes: &[Key]) -> Access {
        Access { reads: reads.to_vec(), writes: writes.to_vec(), unknown: false }
    }

    /// A list after a barrier: nothing pending.
    fn quiet() -> Hazards {
        let mut h = Hazards::new();
        h.done();
        h
    }

    #[test]
    fn a_new_list_orders_its_first_requested_dispatch() {
        // What lists before it did is not known.
        let mut h = Hazards::new();
        h.request_all();
        assert!(h.op(&access(&[A], &[])));
        // Without a request nothing is added, even with everything pending.
        let mut h = Hazards::new();
        assert!(!h.op(&access(&[A], &[B])));
    }

    #[test]
    fn independent_dispatches_need_no_barrier() {
        let mut h = quiet();
        assert!(!h.op(&access(&[A], &[T])));
        h.request_all();
        // Reads A again, writes another texture: no conflict.
        assert!(!h.op(&access(&[A], &[U])));
        // The request is still held: a later conflicting dispatch gets it.
        assert!(h.held());
        assert!(h.op(&access(&[T], &[])));
        assert!(!h.held());
    }

    #[test]
    fn read_after_write_write_after_read_and_write_after_write_conflict() {
        // RAW
        let mut h = quiet();
        h.op(&access(&[], &[A]));
        h.request_all();
        assert!(h.op(&access(&[Key::Bytes(0x1800, 0x1900)], &[])));
        // WAR
        let mut h = quiet();
        h.op(&access(&[T], &[]));
        h.request_all();
        assert!(h.op(&access(&[], &[T])));
        // WAW
        let mut h = quiet();
        h.op(&access(&[], &[B]));
        h.request_all();
        assert!(h.op(&access(&[], &[Key::Bytes(0x3fff, 0x5000)])));
        // RAR: none; touching ends only: none.
        let mut h = quiet();
        h.op(&access(&[A], &[]));
        h.request_all();
        assert!(!h.op(&access(&[A], &[Key::Bytes(0x2000, 0x3000)])));
    }

    #[test]
    fn without_a_request_conflicts_are_the_titles_and_left_alone() {
        let mut h = quiet();
        h.op(&access(&[], &[A]));
        assert!(!h.op(&access(&[A], &[A])));
    }

    #[test]
    fn unknown_access_conflicts_with_anything_pending() {
        let mut h = quiet();
        h.request_all();
        // Nothing pending: even an unknown dispatch runs as is.
        let unknown = Access { unknown: true, ..Access::default() };
        assert!(!h.op(&unknown));
        // Now everything is pending.
        h.request_all();
        assert!(h.op(&access(&[B], &[])));
        h.op(&access(&[], &[A]));
        h.request_all();
        assert!(h.op(&unknown));
    }

    #[test]
    fn a_barrier_on_one_resource_orders_only_that_resource() {
        let mut h = quiet();
        h.op(&access(&[], &[A, T]));
        h.request(T);
        // Reads A (written before): the title only asked about T.
        assert!(!h.op(&access(&[A], &[])));
        // Reads T: ordered.
        assert!(h.op(&access(&[T], &[])));
        // A buffer range request matches overlapping bytes.
        let mut h = quiet();
        h.op(&access(&[], &[A]));
        h.request(Key::Bytes(0x1f00, 0x1f10));
        assert!(!h.op(&access(&[B], &[U])));
        // Bytes written before but outside the barrier's: not asked for.
        assert!(!h.op(&access(&[Key::Bytes(0x1000, 0x1001)], &[])));
        assert!(h.op(&access(&[Key::Bytes(0x1f08, 0x1f09)], &[])));
    }

    #[test]
    fn read_only_shaders_do_not_write_what_they_bind_for_unordered_access() {
        let mut h = quiet();
        h.op(&access(&[A], &[]));
        h.request_all();
        let mut a = access(&[], &[A]);
        a.read_only();
        assert_eq!(a.reads, vec![A]);
        assert!(!h.op(&a));
    }

    #[test]
    fn other_work_records_a_held_barrier_and_leaves_everything_pending() {
        let mut h = quiet();
        // Nothing held: nothing recorded.
        assert!(!h.flush());
        // Everything is pending now (the work may touch anything).
        h.request_all();
        assert!(h.flush());
        // Held, but nothing pending: nothing to wait for.
        let mut h = quiet();
        h.request_all();
        assert!(!h.flush());
        assert!(!h.held());
    }

    #[test]
    fn many_objects_become_everything() {
        let mut h = quiet();
        for o in 0..=MOST_OBJECTS + 1 {
            h.op(&access(&[], &[Key::Object(o + 0x10_000)]));
        }
        h.request_all();
        assert!(h.op(&access(&[Key::Object(1)], &[])));
        let mut h = quiet();
        for o in 0..=MOST_OBJECTS + 1 {
            h.request(Key::Object(o));
        }
        assert!(h.held());
        h.op(&access(&[], &[A]));
        // Too many named: taken as a request for all.
        h.request(Key::Object(1));
        assert!(h.op(&access(&[A], &[])));
    }

    #[test]
    fn a_literal_barrier_waits_only_where_a_side_writes() {
        const COMPUTE: u32 = 0x80;
        const SRV: u32 = 0x80;
        const UAV: u32 = ACCESS_UNORDERED;
        // UAV -> SRV, UAV -> UAV, SRV -> UAV: wait.
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, UAV, SRV), Some(true));
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, UAV, UAV), Some(true));
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, SRV, UAV), Some(true));
        // SRV -> SRV | indirect argument: nothing.
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, SRV, SRV | 0x200), Some(false));
        // COMMON may be anything.
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, 0, SRV), Some(true));
        // Nothing before or after.
        assert_eq!(barrier_waits(0, COMPUTE, UAV, UAV), Some(false));
        assert_eq!(barrier_waits(COMPUTE, 0, UAV, UAV), Some(false));
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, ACCESS_NONE, UAV), Some(false));
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, UAV, ACCESS_NONE), Some(false));
        // Bits D3D12 does not have: not understood.
        assert_eq!(barrier_waits(0x0200_0000, COMPUTE, UAV, SRV), None);
        assert_eq!(barrier_waits(COMPUTE, COMPUTE, 0x0100_0000, SRV), None);
    }
}
