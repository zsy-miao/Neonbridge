//! Which of the title's memory is mapped, as the runtime itself mapped it.
//!
//! The title streams by mapping physical pages into placeholder
//! reservations (`XMemVirtualAlloc` with 64 KiB pages, then
//! `XMemMapPhysicalPages[Scatter]`): every such call goes through the
//! runtime, so it knows at all times which bytes of those reservations show
//! a page and which are holes. Asking Windows instead (`VirtualQuery`) walks
//! the page tables to the end of the region asked about, milliseconds a
//! question in the title's big placeholders, and a refresh of a buffer full
//! of holes asked once per hole: seconds a frame while the title streams.
//!
//! [`RegionMap`] holds the reservations and the views (each view as the
//! runtime mapped it: a view is one host allocation, so protection changes
//! cannot span two) and answers, for a range, which runs of it are mapped,
//! holes, or memory it does not know (memory not made through the
//! placeholder calls: the caller asks Windows as before). Platform-neutral,
//! so it is tested on any host.

use std::collections::BTreeMap;

/// What a run of bytes is, by the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Shows a mapped view.
    Mapped,
    /// Inside a placeholder reservation, no view: reads as nothing.
    Hole,
    /// Not in a reservation the map knows: ask the system.
    Unknown,
}

/// `[start, end)` of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

impl Run {
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Placeholder reservations and the views mapped in them.
#[derive(Debug, Default)]
pub struct RegionMap {
    /// Start -> end; disjoint, adjacent ones merged.
    reserved: BTreeMap<usize, usize>,
    /// Start -> end of each view as mapped; disjoint, never merged.
    views: BTreeMap<usize, usize>,
    /// Bumped by every change.
    generation: u64,
}

/// Removes `[at, end)` from a map of disjoint intervals, keeping the parts
/// of intervals outside it.
fn cut(map: &mut BTreeMap<usize, usize>, at: usize, end: usize) {
    if at >= end {
        return;
    }
    let mut hit: Vec<(usize, usize)> = Vec::new();
    if let Some((&s, &e)) = map.range(..=at).next_back()
        && e > at
    {
        hit.push((s, e));
    }
    hit.extend(map.range(at.saturating_add(1)..end).map(|(&s, &e)| (s, e)));
    for (s, e) in hit {
        map.remove(&s);
        if s < at {
            map.insert(s, at);
        }
        if e > end {
            map.insert(end, e);
        }
    }
}

/// The intervals of `map` that meet `[at, end)`, in order, unclipped.
fn meeting(map: &BTreeMap<usize, usize>, at: usize, end: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
    // `range` panics on a backward range: an empty one meets nothing.
    let (first, rest) = if at < end {
        (map.range(..=at).next_back().filter(|(_, e)| **e > at).map(|(&s, &e)| (s, e)), at + 1..end)
    } else {
        (None, 0..0)
    };
    first.into_iter().chain(map.range(rest).map(|(&s, &e)| (s, e)))
}

impl RegionMap {
    pub const fn new() -> Self {
        Self { reserved: BTreeMap::new(), views: BTreeMap::new(), generation: 0 }
    }

    /// Bumped by every change: a caller may keep an answer while it holds.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the map knows any reservation (else every answer is
    /// [`Kind::Unknown`], and callers need not ask).
    pub fn is_empty(&self) -> bool {
        self.reserved.is_empty() && self.views.is_empty()
    }

    /// `[at, at + len)` was reserved as a placeholder: holes until mapped.
    pub fn reserve(&mut self, at: usize, len: usize) {
        let end = at.saturating_add(len);
        if at >= end {
            return;
        }
        self.generation += 1;
        // Merge with what meets or touches it.
        let (mut lo, mut hi) = (at, end);
        let touching: Vec<(usize, usize)> = meeting(&self.reserved, at.saturating_sub(1), end.saturating_add(1)).collect();
        for (s, e) in touching {
            if e >= at && s <= end {
                self.reserved.remove(&s);
                lo = lo.min(s);
                hi = hi.max(e);
            }
        }
        self.reserved.insert(lo, hi);
    }

    /// `[at, at + len)` is no longer reserved (released, or reused for
    /// something else): neither holes nor views there.
    pub fn release(&mut self, at: usize, len: usize) {
        let end = at.saturating_add(len);
        if at >= end {
            return;
        }
        self.generation += 1;
        cut(&mut self.reserved, at, end);
        cut(&mut self.views, at, end);
    }

    /// `[at, at + len)` now shows one view (in place of whatever it showed).
    pub fn map(&mut self, at: usize, len: usize) {
        let end = at.saturating_add(len);
        if at >= end {
            return;
        }
        self.generation += 1;
        cut(&mut self.views, at, end);
        self.views.insert(at, end);
    }

    /// `[at, at + len)` shows nothing any more: the views over it are cut
    /// back to what lies outside it.
    pub fn unmap(&mut self, at: usize, len: usize) {
        let end = at.saturating_add(len);
        if at >= end {
            return;
        }
        self.generation += 1;
        cut(&mut self.views, at, end);
    }

    /// Whether any byte of `[at, at + len)` lies in a reservation or a view.
    pub fn knows_any(&self, at: usize, len: usize) -> bool {
        let end = at.saturating_add(len.max(1));
        meeting(&self.reserved, at, end).next().is_some() || meeting(&self.views, at, end).next().is_some()
    }

    /// `[at, at + len)` by kind, in order, covering it exactly. Adjacent
    /// mapped views are one run (a copy reads across them at once); with
    /// `by_view` each view is a run of its own (what a protection change
    /// can take in one call). Holes and unknown bytes are always merged.
    pub fn runs(&self, at: usize, len: usize, by_view: bool) -> Vec<Run> {
        let end = at.saturating_add(len);
        let mut out: Vec<Run> = Vec::new();
        if at >= end {
            return out;
        }
        let push = |out: &mut Vec<Run>, start: usize, stop: usize, kind: Kind, merge: bool| {
            if start >= stop {
                return;
            }
            if let Some(last) = out.last_mut()
                && last.kind == kind
                && last.end == start
                && merge
            {
                last.end = stop;
                return;
            }
            out.push(Run { start, end: stop, kind });
        };
        let gap = |out: &mut Vec<Run>, from: usize, to: usize| {
            let mut p = from;
            for (s, e) in meeting(&self.reserved, from, to) {
                let (s, e) = (s.max(from), e.min(to));
                push(out, p, s, Kind::Unknown, true);
                push(out, s, e, Kind::Hole, true);
                p = e;
            }
            push(out, p, to, Kind::Unknown, true);
        };
        let mut p = at;
        for (s, e) in meeting(&self.views, at, end) {
            let (s, e) = (s.max(at), e.min(end));
            gap(&mut out, p, s);
            push(&mut out, s, e, Kind::Mapped, !by_view);
            p = e;
        }
        gap(&mut out, p, end);
        out
    }

    /// The longest run of one kind holding `at`: the view, the hole between
    /// views of a reservation, or the unknown bytes up to the next thing the
    /// map knows (`usize::MAX` past the last).
    pub fn around(&self, at: usize) -> Run {
        if let Some((&s, &e)) = self.views.range(..=at).next_back()
            && e > at
        {
            return Run { start: s, end: e, kind: Kind::Mapped };
        }
        let view_before = self.views.range(..=at).next_back().map_or(0, |(_, &e)| e);
        let view_after = self.views.range(at..).next().map_or(usize::MAX, |(&s, _)| s);
        if let Some((&s, &e)) = self.reserved.range(..=at).next_back()
            && e > at
        {
            return Run { start: s.max(view_before), end: e.min(view_after), kind: Kind::Hole };
        }
        let reserved_before = self.reserved.range(..=at).next_back().map_or(0, |(_, &e)| e);
        let reserved_after = self.reserved.range(at..).next().map_or(usize::MAX, |(&s, _)| s);
        Run { start: view_before.max(reserved_before), end: view_after.min(reserved_after), kind: Kind::Unknown }
    }

    /// Whether every byte of `[at, at + len)` is known (mapped or a hole).
    pub fn knows_all(&self, at: usize, len: usize) -> bool {
        len > 0 && self.runs(at, len, false).iter().all(|r| r.kind != Kind::Unknown)
    }

    /// Whether all of `[at, at + len)` is mapped (`Some(true)`), some of it
    /// is a hole (`Some(false)`), or the map cannot say (`None`: part of it
    /// is unknown and the rest mapped).
    pub fn all_mapped(&self, at: usize, len: usize) -> Option<bool> {
        let mut unknown = false;
        for run in self.runs(at, len, false) {
            match run.kind {
                Kind::Hole => return Some(false),
                Kind::Unknown => unknown = true,
                Kind::Mapped => {}
            }
        }
        (!unknown).then_some(true)
    }
}

/// `[at, at + len)` in pieces of at most `max` bytes, cut at multiples of
/// `max` (so that pieces of overlapping ranges line up).
pub fn chunks(at: usize, len: usize, max: usize) -> impl Iterator<Item = (usize, usize)> {
    let end = at.saturating_add(len);
    let max = max.max(1);
    let mut p = at;
    std::iter::from_fn(move || {
        if p >= end {
            return None;
        }
        let next = (p / max).saturating_add(1).saturating_mul(max).min(end);
        let piece = (p, next);
        p = next;
        Some(piece)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: usize = 64 << 10;

    fn r(start: usize, end: usize, kind: Kind) -> Run {
        Run { start, end, kind }
    }

    #[test]
    fn unknown_memory_is_unknown() {
        let m = RegionMap::new();
        assert!(m.is_empty());
        assert_eq!(m.runs(10 * P, 3 * P, false), vec![r(10 * P, 13 * P, Kind::Unknown)]);
        assert_eq!(m.all_mapped(10 * P, P), None);
        assert!(!m.knows_any(10 * P, P));
    }

    #[test]
    fn a_reservation_is_holes_until_mapped() {
        let mut m = RegionMap::new();
        m.reserve(100 * P, 50 * P);
        assert_eq!(m.runs(100 * P, 50 * P, false), vec![r(100 * P, 150 * P, Kind::Hole)]);
        assert_eq!(m.all_mapped(100 * P, P), Some(false));
        // Straddling the reservation's edges.
        assert_eq!(
            m.runs(98 * P, 54 * P, false),
            vec![r(98 * P, 100 * P, Kind::Unknown), r(100 * P, 150 * P, Kind::Hole), r(150 * P, 152 * P, Kind::Unknown)]
        );
    }

    #[test]
    fn mapping_unmapping_and_remapping() {
        let mut m = RegionMap::new();
        m.reserve(0, 16 * P);
        m.map(4 * P, 4 * P);
        assert_eq!(
            m.runs(0, 16 * P, false),
            vec![r(0, 4 * P, Kind::Hole), r(4 * P, 8 * P, Kind::Mapped), r(8 * P, 16 * P, Kind::Hole)]
        );
        assert_eq!(m.all_mapped(5 * P, 2 * P), Some(true));
        // Unmapping one page in the middle cuts the view in two.
        m.unmap(5 * P, P);
        assert_eq!(
            m.runs(4 * P, 4 * P, true),
            vec![r(4 * P, 5 * P, Kind::Mapped), r(5 * P, 6 * P, Kind::Hole), r(6 * P, 8 * P, Kind::Mapped)]
        );
        // The runtime's unmap then maps what stays again (xmem `unmap`):
        // the same bytes, told as new views.
        m.map(4 * P, P);
        m.map(6 * P, 2 * P);
        assert_eq!(
            m.runs(4 * P, 4 * P, false),
            vec![r(4 * P, 5 * P, Kind::Mapped), r(5 * P, 6 * P, Kind::Hole), r(6 * P, 8 * P, Kind::Mapped)]
        );
        // Remapping over a view replaces it.
        m.map(4 * P, 4 * P);
        assert_eq!(m.runs(4 * P, 4 * P, true), vec![r(4 * P, 8 * P, Kind::Mapped)]);
        // Unmapping all of it leaves the reservation's holes.
        m.unmap(0, 16 * P);
        assert_eq!(m.runs(0, 16 * P, false), vec![r(0, 16 * P, Kind::Hole)]);
    }

    #[test]
    fn scattered_pages_coalesce_for_copies_not_for_protection() {
        let mut m = RegionMap::new();
        m.reserve(0, 64 * P);
        // XMemMapPhysicalPagesScatter: one view per page.
        for k in 8..24 {
            m.map(k * P, P);
        }
        assert_eq!(m.runs(8 * P, 16 * P, false), vec![r(8 * P, 24 * P, Kind::Mapped)]);
        let by_view = m.runs(8 * P, 16 * P, true);
        assert_eq!(by_view.len(), 16);
        assert!(by_view.iter().all(|run| run.kind == Kind::Mapped && run.len() == P));
        // Every other page unmapped: alternating runs, holes merged with
        // nothing but themselves.
        for k in (8..24).step_by(2) {
            m.unmap(k * P, P);
        }
        let runs = m.runs(8 * P, 16 * P, false);
        assert_eq!(runs.len(), 16);
        for (i, run) in runs.iter().enumerate() {
            assert_eq!(run.kind, if i % 2 == 0 { Kind::Hole } else { Kind::Mapped });
        }
    }

    #[test]
    fn ranges_cut_inside_views_and_holes() {
        let mut m = RegionMap::new();
        m.reserve(0, 8 * P);
        m.map(2 * P, 4 * P);
        // A range starting and ending inside pieces is clipped to itself.
        assert_eq!(
            m.runs(P + 100, 2 * P, false),
            vec![r(P + 100, 2 * P, Kind::Hole), r(2 * P, 3 * P + 100, Kind::Mapped)]
        );
        assert_eq!(m.runs(3 * P + 7, 13, false), vec![r(3 * P + 7, 3 * P + 20, Kind::Mapped)]);
        assert!(m.runs(5 * P, 0, false).is_empty());
    }

    #[test]
    fn reservations_merge_and_release_cuts() {
        let mut m = RegionMap::new();
        m.reserve(0, 4 * P);
        m.reserve(4 * P, 4 * P); // adjacent
        m.reserve(2 * P, 8 * P); // overlapping
        assert_eq!(m.runs(0, 12 * P, false), vec![r(0, 10 * P, Kind::Hole), r(10 * P, 12 * P, Kind::Unknown)]);
        m.map(3 * P, 2 * P);
        // Releasing the middle drops its views and holes.
        m.release(4 * P, 2 * P);
        assert_eq!(
            m.runs(0, 10 * P, false),
            vec![
                r(0, 3 * P, Kind::Hole),
                r(3 * P, 4 * P, Kind::Mapped),
                r(4 * P, 6 * P, Kind::Unknown),
                r(6 * P, 10 * P, Kind::Hole)
            ]
        );
        assert!(m.knows_any(5 * P, 2 * P));
        assert!(!m.knows_any(4 * P, 2 * P));
    }

    #[test]
    fn views_outside_reservations_are_still_mapped() {
        let mut m = RegionMap::new();
        m.map(20 * P, 2 * P);
        assert_eq!(
            m.runs(19 * P, 4 * P, false),
            vec![r(19 * P, 20 * P, Kind::Unknown), r(20 * P, 22 * P, Kind::Mapped), r(22 * P, 23 * P, Kind::Unknown)]
        );
        assert_eq!(m.all_mapped(19 * P, 4 * P), None);
        assert_eq!(m.all_mapped(20 * P, 2 * P), Some(true));
    }

    #[test]
    fn every_change_bumps_the_generation() {
        let mut m = RegionMap::new();
        let g = m.generation();
        m.reserve(0, P);
        m.map(0, P);
        m.unmap(0, P);
        m.release(0, P);
        assert_eq!(m.generation(), g + 4);
        // Empty ranges change nothing.
        m.map(0, 0);
        assert_eq!(m.generation(), g + 4);
    }

    #[test]
    fn runs_cover_the_range_exactly_whatever_the_sequence() {
        // A pseudo-random mapping history against a page-by-page model.
        let mut m = RegionMap::new();
        let pages = 96;
        let (mut reserved, mut mapped) = (vec![false; pages], vec![false; pages]);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..2000 {
            let a = next(pages);
            let n = 1 + next(8).min(pages - a - 1);
            match next(4) {
                0 => {
                    m.reserve(a * P, n * P);
                    reserved[a..a + n].fill(true);
                }
                1 => {
                    m.map(a * P, n * P);
                    mapped[a..a + n].fill(true);
                }
                2 => {
                    m.unmap(a * P, n * P);
                    mapped[a..a + n].fill(false);
                }
                _ => {
                    m.release(a * P, n * P);
                    reserved[a..a + n].fill(false);
                    mapped[a..a + n].fill(false);
                }
            }
            let model: Vec<Kind> = (0..pages)
                .map(|k| match (mapped[k], reserved[k]) {
                    (true, _) => Kind::Mapped,
                    (false, true) => Kind::Hole,
                    _ => Kind::Unknown,
                })
                .collect();
            let (from, len) = (next(pages), 1 + next(pages));
            let len = len.min(pages - from);
            let runs = m.runs(from * P, len * P, false);
            let mut p = from * P;
            for (i, run) in runs.iter().enumerate() {
                assert_eq!(run.start, p);
                assert!(run.end > run.start);
                if i > 0 {
                    assert_ne!(runs[i - 1].kind, run.kind, "adjacent runs of one kind are merged");
                }
                for k in run.start / P..run.end / P {
                    assert_eq!(model[k], run.kind, "page {k}");
                }
                p = run.end;
            }
            assert_eq!(p, (from + len) * P);
        }
    }

    #[test]
    fn around_finds_the_whole_run() {
        let mut m = RegionMap::new();
        m.reserve(10 * P, 20 * P);
        m.map(14 * P, 2 * P);
        m.map(16 * P, P);
        assert_eq!(m.around(12 * P), r(10 * P, 14 * P, Kind::Hole));
        assert_eq!(m.around(15 * P + 3), r(14 * P, 16 * P, Kind::Mapped));
        assert_eq!(m.around(16 * P), r(16 * P, 17 * P, Kind::Mapped));
        assert_eq!(m.around(17 * P), r(17 * P, 30 * P, Kind::Hole));
        assert_eq!(m.around(5 * P), r(0, 10 * P, Kind::Unknown));
        assert_eq!(m.around(30 * P), r(30 * P, usize::MAX, Kind::Unknown));
        assert!(m.knows_all(10 * P, 20 * P));
        assert!(!m.knows_all(9 * P, 2 * P));
        assert!(!m.knows_all(10 * P, 0));
    }

    #[test]
    fn chunks_cut_at_multiples() {
        let c: Vec<_> = chunks(P + 10, 3 * P, 2 * P).collect();
        assert_eq!(c, vec![(P + 10, 2 * P), (2 * P, 4 * P), (4 * P, 4 * P + 10)]);
        assert_eq!(chunks(5, 0, 8).count(), 0);
        assert_eq!(chunks(0, 8, 8).collect::<Vec<_>>(), vec![(0, 8)]);
    }
}
