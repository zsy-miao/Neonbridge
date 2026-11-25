//! Ranges cut from a big block and given back: the bookkeeping behind
//! memory the host is asked for in large pieces instead of once per texture
//! (texture memory in [`crate::resource`], staging memory in
//! [`crate::texture`]).
//!
//! After shadowdog00's vi-pc-combo `0781f61` (GPL-3.0), whose measurement is
//! the reason: in the opening cutscene the pass over textures picked by
//! index spent 80-95% of its time asking the host for memory, one committed
//! resource per staging buffer and one per host texture. Ours besides: a
//! length that found no room is remembered until something is given back
//! (a pass may ask for a thousand ranges of many full blocks).

use std::collections::BTreeMap;

/// The free ranges of one block, by offset.
pub(crate) struct Ranges {
    free: BTreeMap<u64, u64>,
    /// Free bytes in all.
    room: u64,
    /// The shortest length refused since the last range given back, at the
    /// one alignment this block is asked for ([`take`](Self::take)).
    refused: u64,
    /// The first byte no range handed out has ever held.
    fresh: u64,
}

impl Ranges {
    pub(crate) fn new(size: u64) -> Ranges {
        Ranges {
            free: BTreeMap::from([(0, size)]),
            room: size,
            refused: u64::MAX,
            fresh: 0,
        }
    }

    /// Free bytes in all (an upper bound for what [`take`](Self::take) can
    /// give).
    pub(crate) fn room(&self) -> u64 {
        self.room
    }

    /// The offset of `len` bytes at a multiple of `align` (a power of two),
    /// the first that fits, and whether some of it held something before;
    /// `None` if nothing fits. A block is asked for one alignment only.
    pub(crate) fn take(&mut self, len: u64, align: u64) -> Option<(u64, bool)> {
        if len == 0 || len > self.room || len >= self.refused {
            return None;
        }
        let found = self.free.iter().find(|&(&at, &size)| {
            let start = at.next_multiple_of(align);
            start.checked_add(len).is_some_and(|end| end <= at + size)
        });
        let Some((&at, &size)) = found else {
            self.refused = len;
            return None;
        };
        let start = at.next_multiple_of(align);
        self.free.remove(&at);
        if start > at {
            self.free.insert(at, start - at);
        }
        if start + len < at + size {
            self.free.insert(start + len, at + size - (start + len));
        }
        self.room -= len;
        let used_before = start < self.fresh;
        self.fresh = self.fresh.max(start + len);
        Some((start, used_before))
    }

    /// Gives back `len` bytes at `offset` that [`take`](Self::take) gave,
    /// joined with free neighbours.
    pub(crate) fn give(&mut self, offset: u64, len: u64) {
        let (mut at, mut size) = (offset, len);
        if let Some((&before, &its)) = self.free.range(..offset).next_back()
            && before + its == offset
        {
            self.free.remove(&before);
            (at, size) = (before, its + size);
        }
        if let Some(&after) = self.free.get(&(offset + len)) {
            self.free.remove(&(offset + len));
            size += after;
        }
        self.free.insert(at, size);
        self.room += len;
        self.refused = u64::MAX;
    }

    #[cfg(test)]
    fn pieces(&self) -> Vec<(u64, u64)> {
        self.free.iter().map(|(&a, &s)| (a, s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Ranges;

    #[test]
    fn takes_first_fit_and_joins_what_is_given_back() {
        let mut r = Ranges::new(1 << 20);
        let a = r.take(4096, 4096).unwrap().0;
        let b = r.take(8192, 4096).unwrap().0;
        let c = r.take(4096, 4096).unwrap().0;
        assert_eq!((a, b, c), (0, 4096, 12288));
        assert_eq!(r.room(), (1 << 20) - 16384);
        r.give(b, 8192);
        assert_eq!(r.pieces(), vec![(4096, 8192), (16384, (1 << 20) - 16384)]);
        assert_eq!(r.take(12288, 4096), Some((16384, false)));
        assert_eq!(r.take(8192, 4096), Some((4096, true)));
        r.give(16384, 12288);
        r.give(a, 4096);
        r.give(c, 4096);
        r.give(4096, 8192);
        assert_eq!(r.pieces(), vec![(0, 1 << 20)]);
        assert_eq!(r.room(), 1 << 20);
    }

    #[test]
    fn says_which_ranges_held_something_before() {
        let mut r = Ranges::new(65536);
        assert_eq!(r.take(8192, 4096), Some((0, false)));
        assert_eq!(r.take(8192, 4096), Some((8192, false)));
        r.give(8192, 8192);
        // Half of it was handed out before, half of it never.
        assert_eq!(r.take(16384, 4096), Some((8192, true)));
        assert_eq!(r.take(4096, 4096), Some((24576, false)));
    }

    #[test]
    fn remembers_a_refused_length_until_something_comes_back() {
        let mut r = Ranges::new(65536);
        let a = r.take(32768, 4096).unwrap().0;
        r.take(16384, 4096).unwrap();
        assert_eq!(r.take(32768, 4096), None);
        assert_eq!(r.take(40960, 4096), None);
        // Shorter ones still find room.
        assert_eq!(r.take(8192, 4096), Some((49152, false)));
        r.give(a, 32768);
        assert_eq!(r.take(32768, 4096), Some((0, true)));
    }

    #[test]
    fn keeps_the_alignment_and_refuses_what_does_not_fit() {
        let mut r = Ranges::new(1 << 20);
        assert_eq!(r.take(4096, 65536), Some((0, false)));
        assert_eq!(r.take(65536, 65536), Some((65536, false)));
        assert_eq!(r.pieces()[0], (4096, 61440));
        assert_eq!(r.take(0, 65536), None);
        assert_eq!(r.take((1 << 20) + 1, 65536), None);
    }
}
