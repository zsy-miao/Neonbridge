//! Bookkeeping for `VICTORY_COPY_BACK_ASYNC=1` (`victory-gpu`): what the
//! card's shaders wrote into the title's read-back rings is copied back
//! into title memory on a thread of its own, after the flight has landed,
//! instead of before the fences and bottom-of-pipe writes behind it.
//!
//! The title reads its read-back buffers through a ring
//! (`CpuReadBackShaderResourceBuffer`, constructor title+0x1adc2c0): three
//! buffers, the GPU writing slot `frame % 3` and the CPU reading the slot
//! written two frames before. Owners made with mode bit 0x8 get a ring of
//! one and read in the same frame. So a copy-back into a ring of three may
//! land up to a frame late; one into a ring of one may not.
//!
//! [`RingSizes`] tells the two apart from the creations alone: the creator
//! makes a ring's buffers one after another in one call (same caller frame,
//! same size), so a run of two or more such creations is a ring of three
//! (or more), and a lone one is a ring of one and is copied back as before.
//!
//! [`Backs`] is the queue of copy-backs not done yet, in the order their
//! flights landed: each with the present it landed after, its owner (the
//! host copy) and its ranges. It answers what a waiter needs: which
//! copy-backs must be done before a present (those that landed more than
//! `lag` presents ago), before a range is read or freed, before another
//! landing of the same owner, or all of them. Platform-neutral, so it is
//! tested on any host.

use std::collections::VecDeque;

/// One copy-back not done yet.
#[derive(Debug)]
pub struct Back<T> {
    pub id: u64,
    /// Presents made when its flight landed.
    pub present: u64,
    pub owner: usize,
    pub ranges: Vec<(usize, usize)>,
    pub work: T,
}

/// Copy-backs in landing order; done strictly in that order.
#[derive(Debug)]
pub struct Backs<T> {
    queue: VecDeque<Back<T>>,
    /// Taken by the worker, not done: id and its ranges (still written).
    taken: Vec<(u64, usize, Vec<(usize, usize)>)>,
    next: u64,
    /// Every copy-back with an id up to this one is done.
    done: u64,
}

impl<T> Default for Backs<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Backs<T> {
    pub const fn new() -> Self {
        Self { queue: VecDeque::new(), taken: Vec::new(), next: 0, done: 0 }
    }

    /// Queues a copy-back; its id.
    pub fn push(&mut self, present: u64, owner: usize, ranges: Vec<(usize, usize)>, work: T) -> u64 {
        self.next += 1;
        self.queue.push_back(Back { id: self.next, present, owner, ranges, work });
        self.next
    }

    /// The next copy-back to do, for the worker; it stays counted as not
    /// done (its ranges still written) until [`Backs::finish`].
    pub fn take(&mut self) -> Option<Back<T>> {
        let mut back = self.queue.pop_front()?;
        self.taken.push((back.id, back.owner, back.ranges.clone()));
        back.ranges = Vec::new();
        Some(back)
    }

    /// The worker did copy-back `id` (taken in order).
    pub fn finish(&mut self, id: u64) {
        self.taken.retain(|t| t.0 != id);
        let oldest_left = self.taken.iter().map(|t| t.0).chain(self.queue.front().map(|b| b.id)).min();
        self.done = match oldest_left {
            Some(first) => first - 1,
            None => self.next,
        };
    }

    /// Whether copy-back `id` is done.
    pub fn is_done(&self, id: u64) -> bool {
        id <= self.done
    }

    /// Whether nothing is left to do.
    pub fn idle(&self) -> bool {
        self.done == self.next
    }

    /// The last id that must be done before present `now` goes out: the
    /// copy-backs that landed at least `lag` presents before it. `None`
    /// if none is left.
    pub fn due_before_present(&self, now: u64, lag: u64) -> Option<u64> {
        self.pending()
            .filter(|(_, present, _, _)| present.saturating_add(lag) <= now)
            .map(|(id, ..)| id)
            .max()
    }

    /// The last id not done whose ranges share a byte with `[from, to)`.
    pub fn due_for_range(&self, from: usize, to: usize) -> Option<u64> {
        self.pending()
            .filter(|(_, _, _, ranges)| ranges.iter().any(|&(a, b)| a < to && from < b))
            .map(|(id, ..)| id)
            .max()
    }

    /// The last id not done of `owner`.
    pub fn due_for_owner(&self, owner: usize) -> Option<u64> {
        self.pending().filter(|(_, _, o, _)| *o == owner).map(|(id, ..)| id).max()
    }

    /// The last id queued, if any is not done.
    pub fn due_all(&self) -> Option<u64> {
        (!self.idle()).then_some(self.next)
    }

    /// Ranges still to be written into title memory, with their owner.
    pub fn written(&self) -> Vec<(usize, usize, usize)> {
        self.pending()
            .flat_map(|(_, _, owner, ranges)| ranges.iter().map(move |&(a, b)| (owner, a, b)))
            .collect()
    }

    /// Copy-backs not done (queued or taken).
    pub fn len(&self) -> usize {
        self.queue.len() + self.taken.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn pending(&self) -> impl Iterator<Item = (u64, u64, usize, &Vec<(usize, usize)>)> {
        self.taken
            .iter()
            .map(|(id, owner, ranges)| (*id, 0u64, *owner, ranges))
            .chain(self.queue.iter().map(|b| (b.id, b.present, b.owner, &b.ranges)))
    }
}

/// Learns the depth of read-back rings from their creations.
#[derive(Debug, Default)]
pub struct RingSizes {
    /// The last creation: its group key (thread, caller frame) and size,
    /// and the start of its buffer.
    last: Option<((u64, usize), u64, u64)>,
    /// Buffers by start: how many were made in their group.
    groups: std::collections::HashMap<u64, u64>,
    /// The starts in the current group.
    current: Vec<u64>,
}

impl RingSizes {
    pub fn new() -> Self {
        Self::default()
    }

    /// A read-back buffer at `start` of `size` bytes, made by the creator
    /// in the frame `key` names (thread, stack place of the creator's
    /// return address).
    pub fn created(&mut self, key: (u64, usize), start: u64, size: u64) {
        let same = matches!(self.last, Some((k, s, _)) if k == key && s == size);
        if !same {
            self.current.clear();
        }
        self.current.push(start);
        let n = self.current.len() as u64;
        for &s in &self.current {
            self.groups.insert(s, n);
        }
        self.last = Some((key, size, start));
    }

    /// Something else was made in between on `thread`: a group made on
    /// that thread ends.
    pub fn other(&mut self, thread: u64) {
        if matches!(self.last, Some(((t, _), _, _)) if t == thread) {
            self.last = None;
            self.current.clear();
        }
    }

    /// How many buffers were made with the one at `start` (0 unknown).
    pub fn depth(&self, start: u64) -> u64 {
        self.groups.get(&start).copied().unwrap_or(0)
    }

    /// Whether a copy-back into the buffer at `start` may land late: it
    /// is one of a ring of two or more.
    pub fn lagged(&self, start: u64) -> bool {
        self.depth(start) >= 2
    }

    /// The buffer at `start` is let go.
    pub fn forget(&mut self, start: u64) {
        self.groups.remove(&start);
        self.current.retain(|&s| s != start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_backs_are_done_in_landing_order() {
        let mut b: Backs<u32> = Backs::new();
        assert!(b.idle());
        let a = b.push(10, 1, vec![(0x1000, 0x2000)], 7);
        let c = b.push(10, 2, vec![(0x5000, 0x6000)], 8);
        assert!(!b.is_done(a) && !b.is_done(c));
        let first = b.take().unwrap();
        assert_eq!((first.id, first.work), (a, 7));
        // Taken but not done: still written.
        assert_eq!(b.written(), vec![(1, 0x1000, 0x2000), (2, 0x5000, 0x6000)]);
        assert_eq!(b.due_for_range(0x1800, 0x1900), Some(a));
        b.finish(a);
        assert!(b.is_done(a) && !b.is_done(c));
        assert_eq!(b.due_for_range(0x1800, 0x1900), None);
        let second = b.take().unwrap();
        b.finish(second.id);
        assert!(b.is_done(c) && b.idle() && b.is_empty());
        assert!(b.take().is_none());
    }

    #[test]
    fn a_present_waits_only_for_copy_backs_old_enough() {
        let mut b: Backs<()> = Backs::new();
        let a = b.push(5, 1, vec![(0, 16)], ());
        let c = b.push(6, 2, vec![(32, 48)], ());
        let d = b.push(7, 3, vec![(64, 80)], ());
        // Lag 1: present 7 needs what landed after presents 5 and 6.
        assert_eq!(b.due_before_present(7, 1), Some(c));
        assert_eq!(b.due_before_present(6, 1), Some(a));
        assert_eq!(b.due_before_present(5, 1), None);
        // Lag 0: everything landed by now.
        assert_eq!(b.due_before_present(7, 0), Some(d));
        assert_eq!(b.due_all(), Some(d));
        assert_eq!(b.due_for_owner(2), Some(c));
        assert_eq!(b.due_for_owner(9), None);
    }

    #[test]
    fn ranges_only_meet_when_they_share_a_byte() {
        let mut b: Backs<()> = Backs::new();
        let a = b.push(0, 1, vec![(0x100, 0x200), (0x400, 0x500)], ());
        assert_eq!(b.due_for_range(0x200, 0x400), None);
        assert_eq!(b.due_for_range(0x1ff, 0x201), Some(a));
        assert_eq!(b.due_for_range(0x4ff, 0x600), Some(a));
        assert_eq!(b.due_for_range(0x500, 0x600), None);
    }

    #[test]
    fn done_moves_only_past_what_is_finished() {
        let mut b: Backs<u8> = Backs::new();
        let a = b.push(0, 1, vec![], 0);
        let c = b.push(0, 1, vec![], 0);
        let x = b.take().unwrap();
        let y = b.take().unwrap();
        // The second finished first: the first is still not done, so
        // neither counts as done.
        b.finish(y.id);
        assert!(!b.is_done(a) && !b.is_done(c));
        b.finish(x.id);
        assert!(b.is_done(a) && b.is_done(c));
    }

    #[test]
    fn rings_are_told_from_lone_buffers() {
        let mut r = RingSizes::new();
        let (frame, other) = ((1, 0x7ff0), (1, 0x7e00));
        // A ring of three: same frame, same size, one after another.
        r.created(frame, 0x1000, 256);
        r.created(frame, 0x2000, 256);
        r.created(frame, 0x3000, 256);
        // A lone buffer from another caller.
        r.created(other, 0x4000, 64);
        // A lone buffer from the same frame after something else was made.
        r.other(2); // another thread: no effect
        r.created(other, 0x4800, 64);
        r.other(1);
        r.created(frame, 0x5000, 256);
        // Another ring, of a different size, right after: a group of its own.
        r.created(frame, 0x6000, 512);
        r.created(frame, 0x7000, 512);
        r.created(frame, 0x8000, 512);
        for s in [0x1000, 0x2000, 0x3000, 0x6000, 0x7000, 0x8000] {
            assert_eq!(r.depth(s), 3, "{s:#x}");
            assert!(r.lagged(s));
        }
        assert_eq!(r.depth(0x4000), 2); // 0x4000 and 0x4800: same frame and size
        assert_eq!(r.depth(0x4800), 2);
        assert_eq!(r.depth(0x5000), 1);
        assert!(!r.lagged(0x5000));
        assert_eq!(r.depth(0x9000), 0);
        r.forget(0x1000);
        assert_eq!(r.depth(0x1000), 0);
    }
}
