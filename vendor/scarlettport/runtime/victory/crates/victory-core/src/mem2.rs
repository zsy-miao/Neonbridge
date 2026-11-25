//! Book-keeping of the second round of memory work (`VICTORY_MEM2_*`, each
//! off by default; `victory-hle` xmem and `victory-gpu` use it).
//!
//! - [`PageTable`]: `VICTORY_MEM2_GPU_ONLY_NOVIEW=1`. A page mapping
//!   (`CopyPageMappingsBatchX`) whose destination lies in one of the title's
//!   GPU-only reservations (no CPU access on the console, 0x801) is not
//!   mapped as a host view: the table notes which physical page the
//!   destination page shows, and readers of title memory translate through
//!   it to a view of the page that never moves.
//! - [`Aliases`]: which physical pages shown in GPU-only reservations are
//!   also shown where the CPU can reach them, for
//!   `VICTORY_MEM2_GPU_ONLY_NOSNAP=1` (title memory stands in for the
//!   snapshot of a GPU-only range shaders write, since no CPU write can
//!   change it during the flight).
//! - [`PreImages`]: for the same switch, the bytes of such a range the
//!   runtime itself is about to overwrite during a flight, taken just
//!   before, so the compare after the flight still sees the bytes from
//!   before it.
//! - [`PageHashes`]: `VICTORY_MEM2_PAGE_REHASH=1`. A texture's hash kept per
//!   64 KiB page, so that a check after a remap or a write hashes the pages
//!   written, not the whole texture.
//!
//! Platform-neutral, so it is tested on any host.

use std::collections::{BTreeMap, BTreeSet};

/// The console's page, the unit of every table here.
pub const PAGE: usize = 64 << 10;

/// Whether the switch `var` is set to `1`.
pub fn switch(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v.trim() == "1")
}

/// The page-aligned addresses of the pages that hold `[at, at + len)`.
fn pages_of(at: usize, len: usize) -> std::ops::Range<usize> {
    if len == 0 {
        return at / PAGE * PAGE..at / PAGE * PAGE;
    }
    at / PAGE * PAGE..at.saturating_add(len)
}

/// A piece of a range cut by a [`PageTable`]: `[at, at + len)`, and the
/// page-aligned address of the table entry that holds it (`None`: the
/// table has nothing there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub at: usize,
    pub len: usize,
    pub page: Option<usize>,
}

/// Something per 64 KiB page of address space, by the page's address.
#[derive(Debug)]
pub struct PageTable<V> {
    pages: BTreeMap<usize, V>,
}

impl<V> Default for PageTable<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> PageTable<V> {
    pub const fn new() -> Self {
        Self { pages: BTreeMap::new() }
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Sets the entry of the page holding `at`; returns what it held.
    pub fn insert(&mut self, at: usize, v: V) -> Option<V> {
        self.pages.insert(at / PAGE * PAGE, v)
    }

    /// The entry of the page holding `at`, with the page's address.
    pub fn get(&self, at: usize) -> Option<(usize, &V)> {
        let page = at / PAGE * PAGE;
        self.pages.get(&page).map(|v| (page, v))
    }

    /// Takes out the entries of every page that shares a byte with
    /// `[at, at + len)`, in address order.
    pub fn remove_range(&mut self, at: usize, len: usize) -> Vec<(usize, V)> {
        let keys: Vec<usize> = self.pages.range(pages_of(at, len)).map(|(&k, _)| k).collect();
        keys.into_iter().filter_map(|k| self.pages.remove(&k).map(|v| (k, v))).collect()
    }

    /// The entries of the pages that share a byte with `[at, at + len)`.
    pub fn iter_in(&self, at: usize, len: usize) -> impl Iterator<Item = (usize, &V)> {
        self.pages.range(pages_of(at, len)).map(|(&k, v)| (k, v))
    }

    /// Whether a page sharing a byte with `[at, at + len)` has an entry.
    pub fn any_in(&self, at: usize, len: usize) -> bool {
        self.iter_in(at, len).next().is_some()
    }

    /// How many pages sharing a byte with `[at, at + len)` have an entry.
    pub fn count_in(&self, at: usize, len: usize) -> usize {
        self.iter_in(at, len).count()
    }

    /// Whether every page that shares a byte with `[at, at + len)` has an
    /// entry (`len > 0`).
    pub fn covers(&self, at: usize, len: usize) -> bool {
        if len == 0 {
            return false;
        }
        let end = at.saturating_add(len);
        let mut next = at / PAGE * PAGE;
        for (page, _) in self.iter_in(at, len) {
            if page != next {
                return false;
            }
            next = page + PAGE;
        }
        next >= end
    }

    /// `[at, at + len)` cut where the table starts or stops holding pages,
    /// and at each page it holds: the pieces without an entry run as long
    /// as they can, each piece with one is one page (or the part of it in
    /// the range). Together they are the range, in order.
    pub fn pieces(&self, at: usize, len: usize) -> Vec<Piece> {
        let end = at.saturating_add(len);
        let mut out = Vec::new();
        let mut cur = at;
        for (page, _) in self.iter_in(at, len) {
            let from = page.max(at);
            if from > cur {
                out.push(Piece { at: cur, len: from - cur, page: None });
            }
            let to = (page + PAGE).min(end);
            out.push(Piece { at: from, len: to - from, page: Some(page) });
            cur = to;
        }
        if cur < end {
            out.push(Piece { at: cur, len: end - cur, page: None });
        }
        out
    }
}

/// A physical page, as the runtime names it: its section and the page's
/// index in the section.
pub type PageKey = (usize, usize);

/// Where physical pages are shown, as far as a GPU-only reservation is
/// concerned: for each page address in one, the page it shows; for each
/// physical page, how many addresses the CPU can reach show it.
#[derive(Debug, Default)]
pub struct Aliases {
    /// Address (page-aligned) → the page shown there, and whether the
    /// address lies in a GPU-only reservation.
    at: BTreeMap<usize, (PageKey, bool)>,
    /// Physical page → addresses the CPU can reach that show it.
    cpu: std::collections::HashMap<PageKey, u32>,
    /// Physical page → GPU-only addresses that show it.
    gpu: std::collections::HashMap<PageKey, BTreeSet<usize>>,
}

impl Aliases {
    pub fn new() -> Self {
        Self::default()
    }

    /// `at` (a page) shows `key` now (after whatever it showed was hidden).
    /// When `at` is an address the CPU can reach, returns the GPU-only
    /// addresses that show the same page: from now on the CPU can write
    /// what they show.
    pub fn show(&mut self, at: usize, key: PageKey, gpu_only: bool) -> Vec<usize> {
        let at = at / PAGE * PAGE;
        self.hide(at);
        self.at.insert(at, (key, gpu_only));
        if gpu_only {
            self.gpu.entry(key).or_default().insert(at);
            return Vec::new();
        }
        *self.cpu.entry(key).or_default() += 1;
        self.gpu.get(&key).map_or_else(Vec::new, |s| s.iter().copied().collect())
    }

    /// `at` (a page) shows nothing now.
    pub fn hide(&mut self, at: usize) {
        let at = at / PAGE * PAGE;
        let Some((key, gpu_only)) = self.at.remove(&at) else {
            return;
        };
        if gpu_only {
            if let std::collections::hash_map::Entry::Occupied(mut o) = self.gpu.entry(key) {
                o.get_mut().remove(&at);
                if o.get().is_empty() {
                    o.remove();
                }
            }
        } else if let std::collections::hash_map::Entry::Occupied(mut o) = self.cpu.entry(key) {
            *o.get_mut() -= 1;
            if *o.get() == 0 {
                o.remove();
            }
        }
    }

    /// Whether no page shown in `[at, at + len)` is also shown where the CPU
    /// can reach it, and the range shows only through GPU-only addresses
    /// (pages showing nothing pass: nothing can be written there).
    pub fn cpu_free(&self, at: usize, len: usize) -> bool {
        self.at
            .range(pages_of(at, len))
            .all(|(_, &(key, gpu_only))| gpu_only && !self.cpu.contains_key(&key))
    }

    /// Pages shown in all.
    pub fn len(&self) -> usize {
        self.at.len()
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }
}

/// What stands for a 64 KiB piece of a range's bytes from before a flight.
#[derive(Debug, PartialEq, Eq)]
pub enum Before<'a> {
    /// Title memory itself: nothing wrote it since the flight began.
    Title,
    /// Its bytes taken just before the runtime wrote it (the piece's part
    /// inside the range, from its first byte there).
    Taken(&'a [u8]),
    /// Written with no bytes taken first (a write told after the fact): the
    /// title's bytes win over the flight's there.
    Tainted,
}

/// Pre-images of the pieces of one range `[lo, hi)` that the runtime wrote
/// during a flight.
#[derive(Debug, Default)]
pub struct PreImages {
    taken: BTreeMap<usize, Vec<u8>>,
    tainted: BTreeSet<usize>,
}

impl PreImages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.taken.is_empty() && self.tainted.is_empty()
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.taken.values().map(Vec::len).sum()
    }

    /// The pieces (64 KiB-aligned addresses) of `[at, at + len)` inside
    /// `[lo, hi)`, each with its part inside `[lo, hi)`.
    fn parts(at: usize, len: usize, lo: usize, hi: usize) -> Vec<(usize, usize, usize)> {
        let (from, to) = (at.max(lo), at.saturating_add(len).min(hi));
        if from >= to {
            return Vec::new();
        }
        (from / PAGE..to.div_ceil(PAGE))
            .map(|p| {
                let piece = p * PAGE;
                (piece, piece.max(lo), (piece + PAGE).min(hi))
            })
            .collect()
    }

    /// The runtime is about to write `[at, at + len)`: the pieces of it in
    /// `[lo, hi)` not taken yet are read now (`read(address, into)`, false if
    /// it cannot: the piece is tainted). Returns the bytes taken.
    pub fn before_write(
        &mut self,
        at: usize,
        len: usize,
        lo: usize,
        hi: usize,
        mut read: impl FnMut(usize, &mut [u8]) -> bool,
    ) -> usize {
        let mut took = 0;
        for (piece, a, b) in Self::parts(at, len, lo, hi) {
            if self.taken.contains_key(&piece) || self.tainted.contains(&piece) {
                continue;
            }
            let mut bytes = vec![0u8; b - a];
            if read(a, &mut bytes) {
                took += bytes.len();
                self.taken.insert(piece, bytes);
            } else {
                self.tainted.insert(piece);
            }
        }
        took
    }

    /// `[at, at + len)` (inside one piece) is declared now with `copy` as
    /// its bytes from before the flight (`copy(address, into)`): a piece
    /// with bytes taken has them replaced there; a piece without gets them
    /// when `differs` (title memory does not hold them), the rest of the
    /// piece read from title memory (`title(address, into)`, zeros if it
    /// cannot), which stands for it; a tainted piece stays so.
    pub fn declare(
        &mut self,
        at: usize,
        len: usize,
        lo: usize,
        hi: usize,
        differs: bool,
        mut copy: impl FnMut(usize, &mut [u8]),
        mut title: impl FnMut(usize, &mut [u8]) -> bool,
    ) {
        for (piece, a, b) in Self::parts(at, len, lo, hi) {
            if self.tainted.contains(&piece) {
                continue;
            }
            let (from, to) = (at.max(a), at.saturating_add(len).min(b));
            if let Some(bytes) = self.taken.get_mut(&piece) {
                copy(from, &mut bytes[from - a..to - a]);
                continue;
            }
            if !differs {
                continue;
            }
            let mut bytes = vec![0u8; b - a];
            if !title(a, &mut bytes) {
                bytes.fill(0);
            }
            copy(from, &mut bytes[from - a..to - a]);
            self.taken.insert(piece, bytes);
        }
    }

    /// `[at, at + len)` was written already (told after the fact): its
    /// pieces in `[lo, hi)` with no bytes taken are tainted.
    pub fn after_write(&mut self, at: usize, len: usize, lo: usize, hi: usize) {
        for (piece, _, _) in Self::parts(at, len, lo, hi) {
            if !self.taken.contains_key(&piece) {
                self.tainted.insert(piece);
            }
        }
    }

    /// What stands for the bytes from before the flight of the piece
    /// holding `at`.
    pub fn before(&self, at: usize) -> Before<'_> {
        let piece = at / PAGE * PAGE;
        if let Some(bytes) = self.taken.get(&piece) {
            Before::Taken(bytes)
        } else if self.tainted.contains(&piece) {
            Before::Tainted
        } else {
            Before::Title
        }
    }
}

/// A texture's content hash by 64 KiB page of title memory.
#[derive(Debug, Clone, Default)]
pub struct PageHashes {
    start: usize,
    size: usize,
    hashes: Vec<u128>,
}

/// What [`PageHashes::recheck`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recheck {
    /// Indices of the pages whose hash changed.
    pub changed: Vec<usize>,
    /// Bytes hashed.
    pub hashed: usize,
}

impl PageHashes {
    /// The pieces of `[start, start + size)` cut at multiples of 64 KiB of
    /// title memory: (offset from `start`, bytes).
    pub fn spans(start: usize, size: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut off = 0;
        while off < size {
            let n = (PAGE - (start + off) % PAGE).min(size - off);
            out.push((off, n));
            off += n;
        }
        out
    }

    /// The hashes of `[start, start + size)`, page by page (`hash(offset,
    /// bytes)`).
    pub fn build(start: usize, size: usize, mut hash: impl FnMut(usize, usize) -> u128) -> Self {
        let hashes = Self::spans(start, size).into_iter().map(|(off, n)| hash(off, n)).collect();
        Self { start, size, hashes }
    }

    /// Whether these are the hashes of `[start, start + size)`.
    pub fn of(&self, start: usize, size: usize) -> bool {
        self.start == start && self.size == size && !self.hashes.is_empty()
    }

    /// The page (index) holding title page number `page` (address / 64 KiB),
    /// if the texture has one there.
    fn index(&self, page: usize) -> Option<usize> {
        let first = self.start / PAGE;
        let i = page.checked_sub(first)?;
        (i < self.hashes.len()).then_some(i)
    }

    /// Hashes the pages of title page numbers `pages` that the texture has
    /// (each once) and keeps the new hashes; says which changed.
    pub fn recheck(&mut self, pages: impl IntoIterator<Item = usize>, mut hash: impl FnMut(usize, usize) -> u128) -> Recheck {
        let spans = Self::spans(self.start, self.size);
        let mut seen = BTreeSet::new();
        let mut out = Recheck { changed: Vec::new(), hashed: 0 };
        for page in pages {
            let Some(i) = self.index(page) else { continue };
            if !seen.insert(i) {
                continue;
            }
            let (off, n) = spans[i];
            let h = hash(off, n);
            out.hashed += n;
            if self.hashes[i] != h {
                self.hashes[i] = h;
                out.changed.push(i);
            }
        }
        out
    }

    /// The page hashes as bytes, for one hash of the whole texture.
    pub fn digest_input(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + 16 * self.hashes.len());
        out.extend_from_slice(&(self.start as u64).to_le_bytes());
        out.extend_from_slice(&(self.size as u64).to_le_bytes());
        for h in &self.hashes {
            out.extend_from_slice(&h.to_le_bytes());
        }
        out
    }

    pub fn pages(&self) -> usize {
        self.hashes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: usize = 0x10_0000_0000;

    #[test]
    fn table_cuts_a_range_into_translated_and_plain_pieces() {
        let mut t = PageTable::new();
        assert!(t.pieces(BASE, 3 * PAGE) == vec![Piece { at: BASE, len: 3 * PAGE, page: None }]);
        t.insert(BASE + PAGE, 7u32);
        t.insert(BASE + 2 * PAGE + 5, 8u32); // any address in the page
        assert_eq!(t.get(BASE + 2 * PAGE + 100), Some((BASE + 2 * PAGE, &8)));
        assert_eq!(t.get(BASE), None);
        // A range from inside page 0 to inside page 3.
        let p = t.pieces(BASE + 10, 3 * PAGE);
        assert_eq!(
            p,
            vec![
                Piece { at: BASE + 10, len: PAGE - 10, page: None },
                Piece { at: BASE + PAGE, len: PAGE, page: Some(BASE + PAGE) },
                Piece { at: BASE + 2 * PAGE, len: PAGE, page: Some(BASE + 2 * PAGE) },
                Piece { at: BASE + 3 * PAGE, len: 10, page: None },
            ]
        );
        assert_eq!(p.iter().map(|p| p.len).sum::<usize>(), 3 * PAGE);
        // Starting inside a translated page: the piece is cut to the range.
        let p = t.pieces(BASE + PAGE + 100, 50);
        assert_eq!(p, vec![Piece { at: BASE + PAGE + 100, len: 50, page: Some(BASE + PAGE) }]);
        assert!(t.any_in(BASE + PAGE - 1, 2));
        assert!(!t.any_in(BASE, PAGE));
        assert_eq!(t.count_in(BASE, 4 * PAGE), 2);
        assert!(t.covers(BASE + PAGE + 1, 2 * PAGE - 1));
        assert!(!t.covers(BASE + PAGE, 3 * PAGE));
        assert!(!t.covers(BASE, 2 * PAGE));
        assert!(t.pieces(BASE, 0).is_empty());
    }

    #[test]
    fn table_removes_every_page_a_range_touches() {
        let mut t = PageTable::new();
        for k in 0..6 {
            t.insert(BASE + k * PAGE, k);
        }
        // One byte into page 4 back to the last byte of page 1: pages 1-4.
        let gone = t.remove_range(BASE + 2 * PAGE - 1, 2 * PAGE + 2);
        assert_eq!(gone.iter().map(|g| g.1).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(t.len(), 2);
        assert!(t.remove_range(BASE + PAGE, 0).is_empty(), "no bytes, no pages");
        assert_eq!(t.remove_range(BASE, 6 * PAGE).len(), 2);
        assert!(t.is_empty());
    }

    #[test]
    fn aliases_tell_ranges_the_cpu_can_write_through_another_address() {
        let mut a = Aliases::new();
        let gpu = BASE;
        let cpu = 0x2000_0000;
        // Page (1, 0) at two GPU-only addresses, page (1, 1) also in a CPU view.
        assert!(a.show(gpu, (1, 0), true).is_empty());
        a.show(gpu + PAGE, (1, 0), true);
        a.show(gpu + 2 * PAGE, (1, 1), true);
        assert!(a.cpu_free(gpu, 3 * PAGE));
        // The CPU view exposes the GPU-only address that shows the same page.
        assert_eq!(a.show(cpu, (1, 1), false), vec![gpu + 2 * PAGE]);
        assert!(a.cpu_free(gpu, 2 * PAGE));
        assert!(!a.cpu_free(gpu, 3 * PAGE));
        assert!(!a.cpu_free(gpu + 2 * PAGE + 7, 1));
        // A range holding a CPU address itself is never CPU-free.
        assert!(!a.cpu_free(cpu, 1));
        // The CPU view goes: free again. Showing again replaces.
        a.hide(cpu);
        assert!(a.cpu_free(gpu, 3 * PAGE));
        a.show(cpu, (1, 1), false);
        a.show(cpu, (2, 0), false);
        assert!(a.cpu_free(gpu, 3 * PAGE), "the CPU address shows another page now");
        a.show(gpu, (2, 0), true);
        assert!(!a.cpu_free(gpu, 1));
        // Unmapped pages pass.
        assert!(a.cpu_free(gpu + 10 * PAGE, 5 * PAGE));
        a.hide(gpu);
        a.hide(gpu); // twice is harmless
        assert_eq!(a.len(), 3);
        // Page (1, 0) is left at one GPU-only address; a CPU view of it names it.
        assert_eq!(a.show(cpu + PAGE, (1, 0), false), vec![gpu + PAGE]);
        a.hide(gpu + PAGE);
        a.hide(cpu + PAGE);
        assert!(a.show(cpu + PAGE, (1, 0), false).is_empty());
    }

    #[test]
    fn pre_images_are_taken_once_before_the_first_write() {
        let (lo, hi) = (BASE + 100, BASE + 3 * PAGE - 100);
        let mut p = PreImages::new();
        assert_eq!(p.before(BASE + PAGE), Before::Title);
        let mut reads = Vec::new();
        // A write over pieces 0 and 1: both taken, cut to [lo, hi).
        let took = p.before_write(BASE, PAGE + 5, lo, hi, |a, into| {
            reads.push((a, into.len()));
            into.fill((a >> 16) as u8);
            true
        });
        assert_eq!(reads, vec![(lo, PAGE - 100), (BASE + PAGE, PAGE)]);
        assert_eq!(took, 2 * PAGE - 100);
        // A second write there takes nothing: the first bytes stand.
        assert_eq!(p.before_write(BASE + 10, 10, lo, hi, |_, _| panic!("read again")), 0);
        match p.before(BASE + PAGE + 1) {
            Before::Taken(b) => assert_eq!(b.len(), PAGE),
            other => panic!("{other:?}"),
        }
        // A write told after the fact taints only pieces with nothing taken.
        p.after_write(BASE, 3 * PAGE, lo, hi);
        assert!(matches!(p.before(BASE + 50), Before::Taken(_)));
        assert_eq!(p.before(BASE + 2 * PAGE), Before::Tainted);
        // A failed read taints; outside the range nothing happens.
        let mut q = PreImages::new();
        q.before_write(BASE + 2 * PAGE, 1, lo, hi, |_, _| false);
        assert_eq!(q.before(BASE + 2 * PAGE), Before::Tainted);
        assert_eq!(q.before_write(BASE + 5 * PAGE, PAGE, lo, hi, |_, _| panic!("outside")), 0);
        q.after_write(BASE + 5 * PAGE, PAGE, lo, hi);
        assert_eq!(q.before(BASE + 5 * PAGE), Before::Title);
        assert_eq!(p.bytes(), 2 * PAGE - 100);
    }

    #[test]
    fn declared_parts_keep_the_copys_bytes_where_title_memory_differs() {
        let (lo, hi) = (BASE, BASE + 2 * PAGE);
        let copy = |a: usize, into: &mut [u8]| into.fill(0xc0 | ((a - BASE) / 1000) as u8 & 0x0f);
        let title = |_: usize, into: &mut [u8]| {
            into.fill(0x11);
            true
        };
        let mut p = PreImages::new();
        // Equal to title memory: nothing kept, title memory stands for it.
        p.declare(BASE + 10, 100, lo, hi, false, copy, title);
        assert_eq!(p.before(BASE + 10), Before::Title);
        assert!(p.is_empty());
        // Different: the piece is kept, the part from the copy, the rest
        // from title memory.
        p.declare(BASE + 2000, 1000, lo, hi, true, copy, title);
        match p.before(BASE) {
            Before::Taken(b) => {
                assert_eq!(b.len(), PAGE);
                assert_eq!(b[0], 0x11);
                assert_eq!(b[1999], 0x11);
                assert_eq!(b[2000], 0xc2);
                assert_eq!(b[2999], 0xc2);
                assert_eq!(b[3000], 0x11);
            }
            other => panic!("{other:?}"),
        }
        // A later part of a kept piece replaces its bytes there, differing
        // or not.
        p.declare(BASE + 2500, 10, lo, hi, false, |_, into| into.fill(0x99), title);
        match p.before(BASE) {
            Before::Taken(b) => assert_eq!((b[2499], b[2500], b[2509], b[2510]), (0xc2, 0x99, 0x99, 0xc2)),
            other => panic!("{other:?}"),
        }
        // A tainted piece stays tainted; unreadable title memory reads as zeros.
        p.after_write(BASE + PAGE, 1, lo, hi);
        p.declare(BASE + PAGE, 10, lo, hi, true, copy, title);
        assert_eq!(p.before(BASE + PAGE), Before::Tainted);
        let mut q = PreImages::new();
        q.declare(BASE + PAGE + 5, 1, lo, hi, true, |_, into| into.fill(7), |_, _| false);
        match q.before(BASE + PAGE) {
            Before::Taken(b) => assert_eq!((b[4], b[5], b[6]), (0, 7, 0)),
            other => panic!("{other:?}"),
        }
    }

    /// A simple stand-in hash over the bytes.
    fn h(bytes: &[u8]) -> u128 {
        bytes.iter().fold(0x9e37_79b9_u128, |a, &b| a.wrapping_mul(0x100_0000_01b3).wrapping_add(b as u128 + 1))
    }

    #[test]
    fn page_hashes_find_exactly_the_pages_that_changed() {
        // A texture from inside one page to inside another: 4 pieces.
        let start = BASE + PAGE / 2;
        let size = 3 * PAGE;
        let mut mem = vec![0u8; size];
        for (i, b) in mem.iter_mut().enumerate() {
            *b = (i * 31 % 251) as u8;
        }
        let spans = PageHashes::spans(start, size);
        assert_eq!(spans, vec![(0, PAGE / 2), (PAGE / 2, PAGE), (3 * PAGE / 2, PAGE), (5 * PAGE / 2, PAGE / 2)]);
        let mut ph = PageHashes::build(start, size, |o, n| h(&mem[o..o + n]));
        assert!(ph.of(start, size));
        assert!(!ph.of(start, size + 1));
        assert_eq!(ph.pages(), 4);
        let first = start / PAGE;
        // Nothing changed: the pages asked about are hashed, nothing found.
        let r = ph.recheck([first + 1, first + 1, first + 9], |o, n| h(&mem[o..o + n]));
        assert_eq!(r, Recheck { changed: vec![], hashed: PAGE });
        // A byte of the third piece changes; the guard names pieces 2 and 3.
        mem[2 * PAGE] ^= 0xff;
        let r = ph.recheck([first + 2, first + 3], |o, n| h(&mem[o..o + n]));
        assert_eq!(r.changed, vec![2]);
        assert_eq!(r.hashed, PAGE + PAGE / 2);
        // The kept hashes are those of the memory now: a whole rebuild agrees.
        let again = PageHashes::build(start, size, |o, n| h(&mem[o..o + n]));
        assert_eq!(ph.digest_input(), again.digest_input());
        // And differs from before the change.
        mem[2 * PAGE] ^= 0xff;
        let before = PageHashes::build(start, size, |o, n| h(&mem[o..o + n]));
        assert_ne!(before.digest_input(), again.digest_input());
        // Pages below the texture are not its own.
        assert!(ph.recheck([first - 1], |_, _| panic!("not the texture's")).changed.is_empty());
    }

    #[test]
    fn page_hash_spans_cover_aligned_and_tiny_textures() {
        assert_eq!(PageHashes::spans(BASE, 2 * PAGE), vec![(0, PAGE), (PAGE, PAGE)]);
        assert_eq!(PageHashes::spans(BASE + 1, 10), vec![(0, 10)]);
        assert!(PageHashes::spans(BASE, 0).is_empty());
        assert_eq!(PageHashes::spans(BASE + PAGE - 1, 2), vec![(0, 1), (1, 1)]);
    }
}
