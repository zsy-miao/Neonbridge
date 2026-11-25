//! How a batch of page mappings is cut into as few host operations as
//! possible (`VICTORY_PAGEMAP_RUNS=1`, `victory-hle` xmem and `victory-gpu`
//! queue `CopyPageMappingsBatchX`).
//!
//! The title's texture streamer maps its pages in batches: one
//! `XMemMapPhysicalPagesScatter(addresses, n, pages)` per flush, addresses
//! sorted and without neighbours that repeat, then one
//! `CopyPageMappingsBatchX` that maps the same pool pages at the tiled
//! resources' addresses. The runtime did both one 64 KiB page at a time:
//! a lock, a wait for reads in place, an unmap, a placeholder split, a map,
//! a commit and two rounds of notifications per page.
//!
//! [`scatter_groups`] cuts a Scatter batch into groups of consecutive
//! addresses that are all mapped or all unmapped: each group is then one
//! unmap pass and one map call over a range (which itself makes one view
//! per run of consecutive pages). The order of the entries is kept, so an
//! address named twice ends as the title's last entry says, as before.
//!
//! [`merge_alias`] joins the ranges of a page-mapping batch where the
//! destinations follow each other and the sources do too (kind 0 after kind
//! 0) or both unmap; a range of one page repeated (kind 1) stays one
//! operation of its own over all its destination pages.
//!
//! [`Notes`] collects the ranges whose mapping changed and the views made
//! during a batch, so the GPU side is told once per joined range instead of
//! once per page. Platform-neutral, so it is tested on any host.

/// Bytes per mapped page.
pub const PAGE: usize = 64 << 10;

/// Groups of a Scatter batch: `(first entry, entries)`, in order. Entries
/// `i` and `i + 1` share a group when `addresses[i + 1]` is one page after
/// `addresses[i]` and both map or both unmap (`unmaps(i)`).
pub fn scatter_groups(addresses: &[usize], unmaps: impl Fn(usize) -> bool) -> Vec<(usize, usize)> {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for i in 0..addresses.len() {
        let joins = i > 0
            && addresses[i - 1].checked_add(PAGE) == Some(addresses[i])
            && unmaps(i - 1) == unmaps(i);
        match groups.last_mut() {
            Some(g) if joins => g.1 += 1,
            _ => groups.push((i, 1)),
        }
    }
    groups
}

/// One operation of a page-mapping batch: map `len` bytes of what is shown
/// at `from` (or unmap, `from` `None`) at `to`. With `repeat` the one page
/// at `from` is shown at every page of `[to, to + len)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AliasOp {
    pub to: u64,
    pub from: Option<u64>,
    pub len: u64,
    pub repeat: bool,
}

impl AliasOp {
    /// Destination pages.
    pub fn pages(&self) -> u64 {
        self.len / PAGE as u64
    }
}

/// `ops` with neighbours joined where that maps the same: destinations that
/// follow each other, and either both unmap, or neither repeats and the
/// sources follow each other too. A repeated page joins only another range
/// repeating the same page right after it.
pub fn merge_alias(ops: &[AliasOp]) -> Vec<AliasOp> {
    let mut out: Vec<AliasOp> = Vec::with_capacity(ops.len());
    for &op in ops.iter().filter(|o| o.len > 0) {
        if let Some(last) = out.last_mut()
            && last.to.checked_add(last.len) == Some(op.to)
        {
            let joins = match (last.from, op.from) {
                (None, None) => true,
                (Some(a), Some(b)) if !last.repeat && !op.repeat => a.checked_add(last.len) == Some(b),
                (Some(a), Some(b)) if last.repeat && op.repeat => a == b,
                _ => false,
            };
            // A one-page range of either kind is the same as the other.
            let one = |o: &AliasOp| o.len == PAGE as u64;
            let joins = joins
                || matches!((last.from, op.from), (Some(a), Some(b))
                    if last.repeat && one(&op) && a == b)
                || matches!((last.from, op.from), (Some(a), Some(b))
                    if op.repeat && one(last) && a == b);
            if joins {
                if one(last) && op.repeat {
                    last.repeat = true;
                }
                last.len += op.len;
                continue;
            }
        }
        out.push(op);
    }
    out
}

/// What changed during a batch, for one round of notifications at its end.
#[derive(Default, Debug)]
pub struct Notes {
    /// Ranges whose mapping changed, `[from, to)`.
    changed: Vec<(usize, usize)>,
    /// Views made: address, bytes and what they show (opaque to this
    /// module: section and offset).
    views: Vec<(usize, usize, usize, u64)>,
}

impl Notes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn changed(&mut self, at: usize, len: usize) {
        if len > 0 {
            self.changed.push((at, at.saturating_add(len)));
        }
    }

    pub fn view(&mut self, at: usize, len: usize, section: usize, offset: u64) {
        self.views.push((at, len, section, offset));
    }

    /// The changed ranges, sorted and joined where they overlap or touch.
    pub fn changed_joined(&self) -> Vec<(usize, usize)> {
        let mut all = self.changed.clone();
        all.sort_unstable();
        let mut out: Vec<(usize, usize)> = Vec::with_capacity(all.len());
        for (from, to) in all {
            match out.last_mut() {
                Some(last) if from <= last.1 => last.1 = last.1.max(to),
                _ => out.push((from, to)),
            }
        }
        out
    }

    /// The views made that `alive(at, len)` says are still mapped as made
    /// (a later entry of the batch may have unmapped one again), in the
    /// order made.
    pub fn views_alive(&self, alive: impl Fn(usize, usize) -> bool) -> Vec<(usize, usize, usize, u64)> {
        let mut seen = std::collections::HashSet::new();
        // The last made at an address is the one that may still be there.
        let mut out: Vec<_> = self
            .views
            .iter()
            .rev()
            .filter(|v| seen.insert(v.0) && alive(v.0, v.1))
            .copied()
            .collect();
        out.reverse();
        out
    }

    /// Views noted.
    pub fn views(&self) -> usize {
        self.views.len()
    }

    /// Changes noted (before joining).
    pub fn changes(&self) -> usize {
        self.changed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: usize = PAGE;

    #[test]
    fn scatter_groups_join_consecutive_addresses_of_one_kind() {
        let base = 0x10_0000_0000usize;
        let addresses = [base, base + P, base + 2 * P, base + 5 * P, base + 6 * P, base + 7 * P, base + 8 * P];
        // Entry 4 unmaps: it cuts 3..=6 in three.
        let unmaps = |i: usize| i == 4;
        assert_eq!(scatter_groups(&addresses, unmaps), vec![(0, 3), (3, 1), (4, 1), (5, 2)]);
        // Two unmaps next to each other join.
        let unmaps = |i: usize| i == 4 || i == 5;
        assert_eq!(scatter_groups(&addresses, unmaps), vec![(0, 3), (3, 1), (4, 2), (6, 1)]);
    }

    #[test]
    fn scatter_groups_keep_order_and_repeats_apart() {
        let base = 0x2000_0000usize;
        // Not sorted, and one address twice: nothing joins backwards, and
        // the repeat stays its own group after the first.
        let addresses = [base + P, base, base + P, base + P];
        assert_eq!(scatter_groups(&addresses, |_| false), vec![(0, 1), (1, 2), (3, 1)]);
        assert!(scatter_groups(&[], |_| false).is_empty());
        // The end of the address space does not wrap into a join.
        assert_eq!(scatter_groups(&[usize::MAX - P + 1, 0], |_| false), vec![(0, 1), (1, 1)]);
    }

    #[test]
    fn scatter_groups_cover_every_entry_once() {
        let base = 0x4000_0000usize;
        let addresses: Vec<usize> = (0..100).map(|i| base + (i + i / 7) * P).collect();
        let groups = scatter_groups(&addresses, |i| i % 13 == 0);
        let mut next = 0;
        for (first, n) in &groups {
            assert_eq!(*first, next);
            assert!(*n > 0);
            next += n;
        }
        assert_eq!(next, addresses.len());
        assert!(groups.len() < addresses.len());
    }

    fn op(to: u64, from: Option<u64>, pages: u64, repeat: bool) -> AliasOp {
        AliasOp { to, from, len: pages * P as u64, repeat }
    }

    #[test]
    fn alias_runs_join_when_both_sides_follow() {
        let (d, s) = (0x80_0000_0000u64, 0x1_0000_0000u64);
        let p = P as u64;
        let ops = [
            op(d, Some(s), 2, false),
            op(d + 2 * p, Some(s + 2 * p), 1, false), // follows: joins
            op(d + 3 * p, Some(s + 9 * p), 1, false), // source jumps: new
            op(d + 4 * p, None, 2, false),            // unmap: new
            op(d + 6 * p, None, 3, false),            // unmap after unmap: joins
            op(d + 20 * p, None, 1, false),           // destination jumps: new
        ];
        assert_eq!(
            merge_alias(&ops),
            vec![
                op(d, Some(s), 3, false),
                op(d + 3 * p, Some(s + 9 * p), 1, false),
                op(d + 4 * p, None, 5, false),
                op(d + 20 * p, None, 1, false),
            ]
        );
    }

    #[test]
    fn a_repeated_page_joins_only_the_same_page() {
        let (d, s) = (0x80_0000_0000u64, 0x1_0000_0000u64);
        let p = P as u64;
        let ops = [
            op(d, Some(s), 4, true),
            op(d + 4 * p, Some(s), 2, true),       // same page repeated: joins
            op(d + 6 * p, Some(s), 1, false),      // one page of it: joins
            op(d + 7 * p, Some(s + p), 2, false),  // a run from the next page: new
            op(d + 9 * p, Some(s + p), 3, true),   // another page repeated: new
        ];
        assert_eq!(
            merge_alias(&ops),
            vec![op(d, Some(s), 7, true), op(d + 7 * p, Some(s + p), 2, false), op(d + 9 * p, Some(s + p), 3, true)]
        );
        // A one-page run then the same page repeated: one repeated range.
        assert_eq!(merge_alias(&[op(d, Some(s), 1, false), op(d + p, Some(s), 2, true)]), vec![op(d, Some(s), 3, true)]);
        // Empty ranges go.
        assert!(merge_alias(&[op(d, Some(s), 0, false)]).is_empty());
    }

    #[test]
    fn merged_ops_map_the_same_pages_as_the_ops() {
        // Page by page, what each destination page shows.
        fn expand(ops: &[AliasOp]) -> Vec<(u64, Option<u64>)> {
            let p = P as u64;
            ops.iter()
                .flat_map(|o| {
                    (0..o.pages()).map(move |k| (o.to + k * p, o.from.map(|f| if o.repeat { f } else { f + k * p })))
                })
                .collect()
        }
        let (d, s) = (0x80_0000_0000u64, 0x1_0000_0000u64);
        let p = P as u64;
        let mut ops = Vec::new();
        let mut at = d;
        for i in 0..200u64 {
            let pages = 1 + i % 3;
            let o = match i % 5 {
                0 | 1 => op(at, Some(s + (i / 2) * 3 * p), pages, false),
                2 => op(at, Some(s + 7 * p), pages, true),
                3 => op(at, None, pages, false),
                _ => op(at, Some(s + 7 * p), 1, false),
            };
            ops.push(o);
            at += pages * p + if i % 11 == 0 { p } else { 0 };
        }
        let merged = merge_alias(&ops);
        assert!(merged.len() < ops.len());
        assert_eq!(expand(&merged), expand(&ops));
    }

    #[test]
    fn notes_join_changes_and_keep_the_last_view_at_an_address() {
        let mut n = Notes::new();
        n.changed(0x3000, 0x1000);
        n.changed(0x1000, 0x1000);
        n.changed(0x2000, 0x1000);
        n.changed(0x2400, 0x200);
        n.changed(0x9000, 0);
        n.changed(0x8000, 0x1000);
        assert_eq!(n.changed_joined(), vec![(0x1000, 0x4000), (0x8000, 0x9000)]);
        assert_eq!(n.changes(), 5);
        n.view(0x1000, 0x1000, 7, 0);
        n.view(0x2000, 0x1000, 7, 0x1000);
        n.view(0x1000, 0x2000, 8, 0); // mapped again at the same address
        let alive = n.views_alive(|at, len| !(at == 0x2000 && len == 0x1000));
        assert_eq!(alive, vec![(0x1000, 0x2000, 8, 0)]);
    }
}
