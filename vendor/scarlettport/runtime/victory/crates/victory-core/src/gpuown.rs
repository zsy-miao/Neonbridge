//! Bookkeeping for `VICTORY_COPY_BACK_ONLY_READBACK=1` (`victory-gpu`): what
//! shaders wrote into a host copy of title memory and was not copied back.
//!
//! With the switch, only the title's read-back buffers get what shaders
//! wrote copied back into title memory. Everywhere else the host copy holds
//! the truth and title memory holds the bytes from before: those bytes are
//! "owned" by the copy. Title memory is still where the runtime looks in
//! several places, and each of them put the stale bytes back (run R2a: the
//! world drawn, the picture black):
//!
//! - the copy's own refresh copies 64 KiB pages from title memory: a page
//!   the title wrote anywhere, and every page only partly inside a refreshed
//!   range, which is never noted as copied and so is copied every time;
//! - the title's buffer copies (`CopyBufferRegion`) run on the CPU, from
//!   title memory: a shader's result copied into a read-back buffer, or into
//!   another buffer shaders read, was copied stale;
//! - other host copies of the same memory, and textures in it, are filled
//!   from title memory.
//!
//! [`Owned`] keeps, per host copy, the owned byte ranges, each with the
//! write-watch asking it was declared after (its mark). A refresh leaves
//! them out; a page the title wrote after the mark gives the owned bytes on
//! it to the title (the title's bytes win, the page is "demoted": ranges
//! declared there are copied back from then on); a reader of title memory
//! gets the owned bytes written there first ("materialized"). [`Spans`]
//! finds the copies with owned bytes in a range. Platform-neutral, so it is
//! tested on any host.

use std::collections::{BTreeMap, BTreeSet};

/// The pages (of `page` bytes) `[start, end)` touches: first and one past
/// the last.
pub fn pages(start: usize, end: usize, page: usize) -> (usize, usize) {
    (start / page, end.div_ceil(page))
}

/// The parts of `[start, end)` that none of `ranges` holds, in order.
pub fn subtract(start: usize, end: usize, ranges: impl IntoIterator<Item = (usize, usize)>) -> Vec<(usize, usize)> {
    let mut parts = vec![(start, end)];
    for (a, e) in ranges {
        if a >= e {
            continue;
        }
        let mut rest = Vec::with_capacity(parts.len() + 1);
        for (from, to) in parts {
            if e <= from || a >= to {
                rest.push((from, to));
                continue;
            }
            if from < a {
                rest.push((from, a));
            }
            if e < to {
                rest.push((e, to));
            }
        }
        parts = rest;
    }
    parts.retain(|&(a, e)| a < e);
    parts.sort_unstable();
    parts
}

/// Owned byte ranges of title memory of one host copy (see the module).
#[derive(Debug)]
pub struct Owned {
    /// Start → (end, mark); the ranges do not overlap.
    ranges: BTreeMap<usize, (usize, u64)>,
    /// Pages where the title wrote over owned bytes.
    demoted: BTreeSet<usize>,
    page: usize,
}

impl Owned {
    /// Nothing owned; `page` is the write-watch unit (a power of two).
    pub fn new(page: usize) -> Owned {
        Owned {
            ranges: BTreeMap::new(),
            demoted: BTreeSet::new(),
            page: page.max(1),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Owned bytes.
    pub fn bytes(&self) -> usize {
        self.ranges.iter().map(|(&a, &(e, _))| e - a).sum()
    }

    /// The owned ranges (start, end, mark), in order.
    pub fn ranges(&self) -> Vec<(usize, usize, u64)> {
        self.ranges.iter().map(|(&a, &(e, m))| (a, e, m)).collect()
    }

    /// `[start, end)` is owned from now on, declared after asking `mark`
    /// (a later declaration of bytes owned already takes its mark).
    pub fn own(&mut self, start: usize, end: usize, mark: u64) {
        if start >= end {
            return;
        }
        self.release(start, end);
        self.ranges.insert(start, (end, mark));
    }

    /// The owned ranges meeting `[start, end)`, cut to it, with their marks.
    pub fn within(&self, start: usize, end: usize) -> Vec<(usize, usize, u64)> {
        if start >= end {
            return Vec::new();
        }
        let first = self.ranges.range(..=start).next_back().map_or(start, |(&a, _)| a);
        self.ranges
            .range(first..end)
            .filter(|&(_, &(e, _))| e > start)
            .map(|(&a, &(e, m))| (a.max(start), e.min(end), m))
            .collect()
    }

    /// Whether an owned byte lies in `[start, end)`.
    pub fn overlaps(&self, start: usize, end: usize) -> bool {
        !self.within(start, end).is_empty()
    }

    /// `[start, end)` is owned no more; the bytes that were, in order.
    pub fn release(&mut self, start: usize, end: usize) -> Vec<(usize, usize)> {
        let hit = self.within(start, end);
        for &(a, e, _) in &hit {
            // The whole range that holds `[a, e)`: cut around it.
            let Some((&s, &(t, m))) = self.ranges.range(..=a).next_back() else {
                continue;
            };
            self.ranges.remove(&s);
            if s < a {
                self.ranges.insert(s, (a, m));
            }
            if e < t {
                self.ranges.insert(e, (t, m));
            }
        }
        hit.into_iter().map(|(a, e, _)| (a, e)).collect()
    }

    /// The title wrote: `written` holds pages and the asking that last
    /// found each written (as the write-watch journal gives them), for the
    /// pages of `[start, end)`. The owned bytes in `[start, end)` on a page
    /// written after their mark are the title's again: released and
    /// returned, and their pages demoted.
    pub fn title_wrote(&mut self, start: usize, end: usize, written: &[(usize, u64)]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for &(page, write) in written {
            let (lo, hi) = ((page * self.page).max(start), ((page + 1) * self.page).min(end));
            if lo >= hi {
                continue;
            }
            let stale: Vec<(usize, usize)> = self
                .within(lo, hi)
                .into_iter()
                .filter(|&(_, _, mark)| write > mark)
                .map(|(a, e, _)| (a, e))
                .collect();
            if stale.is_empty() {
                continue;
            }
            self.demoted.insert(page);
            for (a, e) in stale {
                out.extend(self.release(a, e));
            }
        }
        out
    }

    /// Owned bytes in `[start, end)` count from asking `mark` on (if later
    /// than their own): a write of ours there, asked about at `mark`, is
    /// not the title's.
    pub fn remark(&mut self, start: usize, end: usize, mark: u64) {
        for (a, e, m) in self.within(start, end) {
            if m < mark {
                self.own(a, e, mark);
            }
        }
    }

    /// Ranges declared on the pages of `[start, end)` are copied back.
    pub fn demote(&mut self, start: usize, end: usize) {
        let (first, last) = pages(start, end, self.page);
        self.demoted.extend(first..last);
    }

    /// Whether a page of `[start, end)` was demoted.
    pub fn demoted(&self, start: usize, end: usize) -> bool {
        if start >= end {
            return false;
        }
        let (first, last) = pages(start, end, self.page);
        self.demoted.range(first..last).next().is_some()
    }
}

/// Spans `[start, end)` of title memory, each named by a key (a host copy's
/// number) with a value; overlaps allowed, one span per key.
#[derive(Debug)]
pub struct Spans<V> {
    map: BTreeMap<(usize, u64), (usize, V)>,
    keys: BTreeMap<u64, usize>,
    /// The longest span ever held: an overlap query looks back that far.
    longest: usize,
}

impl<V> Default for Spans<V> {
    fn default() -> Self {
        Spans {
            map: BTreeMap::new(),
            keys: BTreeMap::new(),
            longest: 0,
        }
    }
}

impl<V> Spans<V> {
    pub const fn new() -> Spans<V> {
        Spans {
            map: BTreeMap::new(),
            keys: BTreeMap::new(),
            longest: 0,
        }
    }

    /// Notes `key` over `[start, end)` (once: a key held already stays).
    pub fn insert(&mut self, start: usize, end: usize, key: u64, value: V) {
        if start >= end || self.keys.contains_key(&key) {
            return;
        }
        self.keys.insert(key, start);
        self.map.insert((start, key), (end, value));
        self.longest = self.longest.max(end - start);
    }

    pub fn contains(&self, key: u64) -> bool {
        self.keys.contains_key(&key)
    }

    pub fn remove(&mut self, key: u64) -> Option<V> {
        let start = self.keys.remove(&key)?;
        self.map.remove(&(start, key)).map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The keys and values of the spans meeting `[start, end)`.
    pub fn overlapping(&self, start: usize, end: usize) -> Vec<(u64, &V)> {
        if start >= end {
            return Vec::new();
        }
        let from = start.saturating_sub(self.longest);
        self.map
            .range((from, 0)..(end, 0))
            .filter(|&(&(a, _), &(e, _))| a < end && start < e)
            .map(|(&(_, key), (_, v))| (key, v))
            .collect()
    }

    /// Every key and value.
    pub fn all(&self) -> Vec<(u64, &V)> {
        self.map.iter().map(|(&(_, key), (_, v))| (key, v)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: usize = 0x10000;

    #[test]
    fn owning_splits_and_releases() {
        let mut o = Owned::new(P);
        o.own(0x1000, 0x3000, 5);
        o.own(0x8000, 0x9000, 6);
        assert_eq!(o.bytes(), 0x3000);
        assert!(o.overlaps(0x2fff, 0x4000));
        assert!(!o.overlaps(0x3000, 0x8000), "ends are exclusive");
        assert_eq!(o.release(0x1800, 0x2000), vec![(0x1800, 0x2000)]);
        assert_eq!(o.ranges(), vec![(0x1000, 0x1800, 5), (0x2000, 0x3000, 5), (0x8000, 0x9000, 6)]);
        // Across two ranges and a gap.
        assert_eq!(o.release(0x1400, 0x8800), vec![(0x1400, 0x1800), (0x2000, 0x3000), (0x8000, 0x8800)]);
        assert_eq!(o.ranges(), vec![(0x1000, 0x1400, 5), (0x8800, 0x9000, 6)]);
        assert!(o.release(0x5000, 0x6000).is_empty());
        assert!(o.release(7, 7).is_empty());
    }

    #[test]
    fn a_later_declaration_takes_its_mark() {
        let mut o = Owned::new(P);
        o.own(0x1000, 0x3000, 5);
        o.own(0x2000, 0x4000, 9);
        assert_eq!(o.ranges(), vec![(0x1000, 0x2000, 5), (0x2000, 0x4000, 9)]);
        assert_eq!(o.within(0x1800, 0x2800), vec![(0x1800, 0x2000, 5), (0x2000, 0x2800, 9)]);
        o.own(0, 0x10000, 10);
        assert_eq!(o.ranges(), vec![(0, 0x10000, 10)]);
        o.own(5, 5, 11);
        assert_eq!(o.ranges().len(), 1, "empty ranges are not owned");
    }

    /// The case that blacked the picture: a small range shaders write
    /// (exposure, a few words) inside a 64 KiB page the refresh never takes
    /// whole. The refresh must leave the owned bytes alone, every time,
    /// until the title writes the page.
    #[test]
    fn a_refresh_leaves_owned_bytes_until_the_title_writes_their_page() {
        let mut o = Owned::new(P);
        let (a, e) = (0x2_1010, 0x2_1020);
        o.own(a, e, 7);
        // Refreshing the view [0x2_1000, 0x2_2000): what is copied.
        let refreshed = |o: &Owned| subtract(0x2_1000, 0x2_2000, o.within(0x2_1000, 0x2_2000).into_iter().map(|(a, e, _)| (a, e)));
        assert_eq!(refreshed(&o), vec![(0x2_1000, a), (e, 0x2_2000)]);
        // The journal found the page written at asking 7 (before the
        // declaration's refresh, or at it): not over the owned bytes.
        assert!(o.title_wrote(0x2_1000, 0x2_2000, &[(2, 7)]).is_empty());
        assert_eq!(refreshed(&o), vec![(0x2_1000, a), (e, 0x2_2000)]);
        assert!(!o.demoted(a, e));
        // Another page written later: nothing here.
        assert!(o.title_wrote(0x2_1000, 0x2_2000, &[(3, 9)]).is_empty());
        // The title writes the page after the declaration: its bytes win,
        // and ranges there go back from now on.
        assert_eq!(o.title_wrote(0x2_1000, 0x2_2000, &[(2, 8)]), vec![(a, e)]);
        assert_eq!(refreshed(&o), vec![(0x2_1000, 0x2_2000)]);
        assert!(o.demoted(a, e));
        assert!(o.demoted(0x2_0000, 0x2_0001), "the whole page");
        assert!(!o.demoted(0x3_0000, 0x3_1000));
    }

    #[test]
    fn a_title_write_takes_only_its_pages() {
        let mut o = Owned::new(P);
        // Owned across three pages, declared after asking 4.
        o.own(0x1_8000, 0x3_8000, 4);
        let back = o.title_wrote(0x1_0000, 0x4_0000, &[(1, 3), (2, 5)]);
        assert_eq!(back, vec![(0x2_0000, 0x3_0000)], "page 1's write came before");
        assert_eq!(o.ranges(), vec![(0x1_8000, 0x2_0000, 4), (0x3_0000, 0x3_8000, 4)]);
        // Asked over part of page 3 only: only that part is given back.
        assert_eq!(o.title_wrote(0x3_0000, 0x3_4000, &[(3, 6)]), vec![(0x3_0000, 0x3_4000)]);
        assert_eq!(o.ranges(), vec![(0x1_8000, 0x2_0000, 4), (0x3_4000, 0x3_8000, 4)]);
    }

    #[test]
    fn a_redeclared_range_is_not_given_back_for_an_older_write() {
        let mut o = Owned::new(P);
        o.own(0x1000, 0x2000, 4);
        // Declared again after asking 9 (the refresh before it saw the
        // page written at 6 and found nothing owned to give back then).
        o.own(0x1000, 0x2000, 9);
        assert!(o.title_wrote(0, P, &[(0, 6)]).is_empty());
        assert_eq!(o.title_wrote(0, P, &[(0, 10)]), vec![(0x1000, 0x2000)]);
    }

    /// Our own write into title memory (owned bytes put there for a reader)
    /// shows as a title write of the page at the next asking. Asked right
    /// after it, the bytes still owned on the page count from that asking,
    /// so it is not taken for the title's; a later title write still is.
    #[test]
    fn a_write_of_ours_is_not_the_titles() {
        let mut o = Owned::new(P);
        o.own(0x1000, 0x2000, 3);
        o.own(0x8000, 0x1_8000, 3);
        // Our write on page 0 was asked about at 6.
        o.remark(0, P, 6);
        assert_eq!(o.ranges(), vec![(0x1000, 0x2000, 6), (0x8000, 0x1_0000, 6), (0x1_0000, 0x1_8000, 3)]);
        assert!(o.title_wrote(0, 2 * P, &[(0, 6)]).is_empty(), "our own write");
        assert_eq!(o.title_wrote(0, 2 * P, &[(1, 6)]), vec![(0x1_0000, 0x1_8000)], "page 1 was not remarked");
        assert_eq!(o.title_wrote(0, 2 * P, &[(0, 7)]), vec![(0x1000, 0x2000), (0x8000, 0x1_0000)]);
        // Never moves a mark back.
        let mut o = Owned::new(P);
        o.own(0x1000, 0x2000, 9);
        o.remark(0, P, 4);
        assert_eq!(o.ranges(), vec![(0x1000, 0x2000, 9)]);
    }

    #[test]
    fn subtract_cuts_pieces_around_ranges() {
        assert_eq!(subtract(0, 100, []), vec![(0, 100)]);
        assert_eq!(subtract(0, 100, [(10, 20), (50, 60)]), vec![(0, 10), (20, 50), (60, 100)]);
        assert_eq!(subtract(0, 100, [(50, 60), (10, 20)]), vec![(0, 10), (20, 50), (60, 100)], "any order");
        assert_eq!(subtract(0, 100, [(0, 100)]), Vec::<(usize, usize)>::new());
        assert_eq!(subtract(10, 20, [(0, 15), (18, 30)]), vec![(15, 18)]);
        assert_eq!(subtract(10, 20, [(20, 30), (0, 10), (5, 5)]), vec![(10, 20)], "touching is not holding");
    }

    #[test]
    fn spans_find_overlaps_and_let_go() {
        let mut s: Spans<&str> = Spans::new();
        s.insert(0x1000, 0x2000, 1, "small");
        s.insert(0x10_0000, 0x50_0000, 2, "big");
        s.insert(0x1800, 0x1900, 3, "inside");
        s.insert(0, 0x10, 1, "again");
        assert_eq!(s.len(), 3, "one span per key");
        let keys = |v: Vec<(u64, &&str)>| v.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(keys(s.overlapping(0x1850, 0x1851)), vec![1, 3]);
        assert_eq!(keys(s.overlapping(0x20_0000, 0x20_0001)), vec![2], "far inside a long span");
        assert!(s.overlapping(0x2000, 0x10_0000).is_empty());
        assert!(s.overlapping(5, 5).is_empty());
        assert_eq!(s.remove(1), Some("small"));
        assert_eq!(s.remove(1), None);
        assert!(!s.contains(1));
        assert_eq!(keys(s.overlapping(0, u64::MAX as usize)), vec![3, 2]);
        assert_eq!(s.all().len(), 2);
    }
}
