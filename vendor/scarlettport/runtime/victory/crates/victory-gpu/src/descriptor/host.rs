//! Host descriptor heaps behind the title's shader-visible heaps (decision
//! 0007, step 3). Translated shaders index the heaps directly at the
//! title's descriptor positions, so each host heap keeps the title heap's
//! order: slot `i` is made from what the title wrote at slot `i`, when it
//! writes it (view creation and descriptor copies) and again when a draw
//! reads it through a table and finds the slot changed. A view heap the
//! host caps below the title's slots keeps that order only for an identity
//! window of them; past it each title slot that holds a view is made in a
//! pool slot, which translated shaders look up in a heap map ([`MapLayout`]).

use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::slot_map::SlotMap;
use super::{VIEW_TAG, View, view_at};
use crate::host::host;
use victory_shader::emit::words::{ImageType, Shape, WORDS, console_words};

pub(crate) struct HostHeap {
    pub(crate) heap: ID3D12DescriptorHeap,
    cpu: usize,
    step: u32,
    /// Slots mirroring the title's at their own index (in a view heap with
    /// a heap map, the identity window); view heaps have `dynamic` more.
    count: u32,
    /// Dynamic slots from `dynamic_first`: [`DYNAMIC`] in view heaps, none
    /// in sampler heaps. (Upstream falls back to 2^16 on a capped host heap;
    /// ours keeps 2^18 there: 2^16 and 2^17 both ran out in this title.)
    dynamic: u32,
    /// The first dynamic slot: `count`, or in a view heap with a heap map
    /// the one after the map's slot ([`MapLayout`]).
    dynamic_first: u32,
    /// The title's own slot count (`count` may be fewer).
    title_count: u32,
    sampler: bool,
    /// The record (view index and generation) each host slot of the
    /// title's slots was last made from, and the heap map's bookkeeping.
    made: Mutex<Made>,
    /// The heap map of a view heap capped below the title's slots
    /// ([`MapLayout`]); `None` for the others.
    map: Option<HeapMap>,
    /// The title heap's first descriptor, as the first slot mirrored past
    /// the window gives it (0 until then): for [`HostHeap::reclaim`].
    title_start: AtomicU64,
    /// When [`HostHeap::reclaim`] last ran (`crate::host::now_ms`;
    /// `u64::MAX`: never).
    reclaimed: AtomicU64,
    /// The host resource each slot views, held while the slot does: shaders
    /// index heaps directly, so a slot the title has not rewritten since it
    /// released the resource may still be read, and a freed host resource
    /// read there is a GPU page fault (the console reads stale memory).
    kept: Mutex<Vec<Option<ID3D12Resource>>>,
    /// Where the search for free dynamic slots starts, and which are taken
    /// (by lists not yet reset).
    next: Mutex<(u32, Vec<bool>)>,
    /// How many dynamic slots are free (changed under `next`'s lock): with
    /// fewer than a table needs, the search is not made. Once the world
    /// had taken them all, every later table of the frame searched every
    /// slot before it was refused.
    free: std::sync::atomic::AtomicU32,
    /// The shortest table that found no run of free slots since slots were
    /// last given back (changed under `next`'s lock; `u32::MAX`: none).
    /// Until something is given back the free runs only shrink, so a table
    /// of that length or longer is refused without the search. Enough
    /// slots were free, but in pieces, and every such table walked the
    /// whole ring before it was refused: 0.33 ms each with 131,072 slots,
    /// 1.0 s of the recording threads in a frame that refused 3,143 (run
    /// 45), under the lock all of them need. (A free run that lies across
    /// the place the search starts from is seen by neither.)
    too_long: std::sync::atomic::AtomicU32,
    /// View heaps: the bytes per element of the buffer view at each title
    /// slot, for shaders that pick a buffer by an index they compute
    /// (`victory_shader::emit::SLOT_STRIDES`): an upload buffer of
    /// `SLOT_STRIDES_COUNT` dwords, its GPU address and its mapping.
    strides: Option<(ID3D12Resource, u64, usize)>,
    /// View heaps: the console descriptor dwords of the view at each title
    /// slot ([`victory_shader::emit::words`]), for shaders that read a
    /// descriptor's dwords as numbers (`victory_shader::emit::SLOT_WORDS`):
    /// an upload buffer of `SLOT_STRIDES_COUNT` slots of eight dwords, its
    /// GPU address and its mapping.
    words: Option<(ID3D12Resource, u64, usize)>,
    /// Title slots holding buffer views. Such a view gets its host slot
    /// (and its memory a host copy) only when something reads it: a draw
    /// through a table, or [`HostHeap::buffer_slots`] for shaders that pick
    /// buffers from the whole heap.
    buffers: Mutex<std::collections::BTreeSet<u32>>,
    /// Of those and of the slots that left the set: the ones written since
    /// [`HostHeap::buffer_changes`] last took them.
    changed: Mutex<std::collections::HashSet<u32>>,
    /// Title slots holding a view of a texture the title may fill itself (a
    /// placed texture, a texture object), with the frame from which they
    /// are due: such a texture is uploaded when a draw reads it through a
    /// table; one a shader picks by a computed index is brought up to date
    /// from here ([`HostHeap::texture_slots_due`]).
    textures: Mutex<TextureQueue>,
    /// Per title slot, the frame (plus one; 0: never) in which a draw last
    /// read it through a table, or made a table it lies in (with
    /// [`TABLED_READ`] set if the draw read it). The draw that
    /// reads such a slot brings its texture up to date itself and the slot
    /// then leaves `textures`; a write of a slot that was a table's within
    /// the last [`TABLED_FRAMES`] frames is due only [`TABLED_GRACE`]
    /// frames later, by when its draw has read it. A title that builds a
    /// table per draw writes thousands of texture views a frame into slots
    /// only tables read (the title: some 2,500 a frame in its
    /// opening cutscene, a few hundred textures over and over), and every
    /// one of them stood in the queue of textures picked by index, ahead
    /// of or behind the ones shaders do pick by index.
    tabled: Vec<std::sync::atomic::AtomicU32>,
    /// The title memory each placed texture view of this heap describes,
    /// by slot and by address ([`Spans`]): which slots a newer view of
    /// other memory there makes stale, and which to look at again when the
    /// runtime maps, unmaps or writes memory ([`remapped`]).
    placed: Mutex<Spans>,
}

/// Host-slot state behind one lock: every write of a host slot for a title
/// slot takes it, so a pool slot is taken, made and published (or given
/// back) at once.
struct Made {
    /// The record (view index and generation) each host slot of the title's
    /// (`count`, and in a heap with a heap map its pool's) was last made
    /// from; 0 for none.
    keys: Vec<u64>,
    /// Which pool slot each title slot past the identity window holds (a
    /// view heap with a heap map).
    map: Option<SlotMap>,
}

impl Made {
    /// The host slot title slot `title` is made in, if it has one (none is
    /// taken here): its own below the window, its pool slot past it.
    fn host(&self, title: u32) -> Option<u32> {
        match &self.map {
            Some(map) => map.host(title),
            None => ((title as usize) < self.keys.len()).then_some(title),
        }
    }
}

/// The heap map of a view heap with a [`MapLayout`]: an upload buffer of one
/// entry per title slot from the identity window on ([`slot_map::entry`]:
/// the bitwise not of its host slot, 0 for none), persistently mapped, and
/// read by translated shaders through a raw view at the layout's map slot
/// (`victory_shader::emit::set_view_heap_map`).
///
/// [`slot_map::entry`]: super::slot_map::entry
struct HeapMap {
    buffer: ID3D12Resource,
    /// Where the entries are mapped.
    at: usize,
}

/// The title's view slots in a view heap whose host heap is capped below
/// them, with a heap map: title slots `0..window` at their own index, as
/// tables made in place need; `pool` slots for the title slots past the
/// window that hold views (looked up in the map by translated shaders);
/// the map's raw view at `slot`; then the fork's [`DYNAMIC`] dynamic slots
/// and its null guard slot last, where they were without a map.
///
/// On NVIDIA a shader-visible view heap holds at most 1,000,000 views
/// ([`PORTABLE_VIEWS`]); the title's has 1,245,184 slots, some
/// 650,000-700,000 of them holding views, about 335,000 of those past the
/// 737,855 title slots a capped heap kept in place (their bindless reads
/// gave the null slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MapLayout {
    window: u32,
    pool: u32,
    slot: u32,
}

impl MapLayout {
    /// The layout for `title_count` title slots, an identity window of up
    /// to `window` slots and the map at `slot`.
    fn new(title_count: u32, window: u32, slot: u32) -> Self {
        let window = window.min(slot).min(title_count);
        MapLayout {
            window,
            pool: (slot - window).min(title_count - window),
            slot,
        }
    }

    /// The first dynamic slot.
    fn dynamic_first(self) -> u32 {
        self.slot + 1
    }

    /// The null guard slot, the heap's last: the limit translated shaders
    /// clamp view indices to.
    fn guard(self) -> u32 {
        self.dynamic_first() + DYNAMIC
    }

    /// The heap's slots.
    fn size(self) -> u32 {
        self.guard() + 1
    }
}

/// Bytes per title view descriptor.
const TITLE_SLOT_BYTES: u64 = 32;

/// The identity window of a heap map without `VICTORY_HEAP_WINDOW`: the
/// title's tables and its first two 64Ki runs of views lie below it, its
/// first empty run (110,940..327,680) spans it.
const WINDOW: u32 = 1 << 17;

/// The heap map's slot in a capped view heap: right before the dynamic
/// slots, so that those and the guard slot stay where they were
/// (`PORTABLE_VIEWS - DYNAMIC - 1` on, the guard at `PORTABLE_VIEWS - 1`).
/// No view index reaches it: one below the window is its own, one past it
/// is a pool slot or the guard, and tables made in place lie below the
/// window.
const MAP_SLOT: u32 = PORTABLE_VIEWS - DYNAMIC - 2;

/// The identity window: [`WINDOW`], or `VICTORY_HEAP_WINDOW` (a smaller one
/// maps more slots, a larger one fewer; at most [`MAP_SLOT`]).
fn window() -> u32 {
    std::env::var("VICTORY_HEAP_WINDOW")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(WINDOW)
        .min(MAP_SLOT)
}

/// Whether a capped view heap gets a heap map; `VICTORY_HEAP_MAP=0` keeps
/// the fork's cap (views past it read the null slot).
fn heap_map_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_MAP").is_none_or(|v| v != "0"))
}

/// The heap map translated shaders read: the identity window and the
/// map's slot ([`probe_view_limit`] sets it before the first translation).
fn heap_map() -> Option<(u32, u32)> {
    #[cfg(test)]
    if let Some(map) = TEST_MAP.get() {
        return Some(map);
    }
    victory_shader::emit::view_heap_map()
}

/// VIcky (diagnostics): the heap map's pool (title slots mapped, pool
/// slots free and waiting, refusals), each time the title slots it holds
/// pass another 64Ki.
#[cfg(feature = "diagnostics")]
fn note_pool(made: &Made) {
    use std::sync::atomic::Ordering::Relaxed;
    static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let Some(stats) = made.map.as_ref().map(SlotMap::stats) else {
        return;
    };
    let step = stats.mapped >> 16;
    if step > LOGGED.load(Relaxed) {
        LOGGED.store(step, Relaxed);
        tracing::info!(?stats, "heap map pool");
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: the heap map (window, slot) of the view heaps this thread
    /// makes, as [`probe_view_limit`] sets one on a capped host, without
    /// reaching other tests' heaps.
    pub(crate) static TEST_MAP: std::cell::Cell<Option<(u32, u32)>> = const { std::cell::Cell::new(None) };
}

/// Title memory by the placed texture views that describe it: per span of
/// memory (its start, its end) the resource description of the texture
/// there and the title slots holding a view of it. A span is the memory of
/// the mips a view shows ([`crate::resource::placed_bytes_from`]). Spans of
/// two textures never overlap: a console title need not clear a descriptor
/// when it streams a texture out, so a view written over memory that older
/// views describe otherwise (another start, or another length and another
/// description: another texture) is all that says the older ones are stale.
/// Views of one span with different descriptions (the formats of one
/// family, say) are one texture's, and so are views of one description at
/// one start that show more and fewer of its mips.
/// No host heap is needed, so it is tested without one.
#[derive(Default)]
pub(crate) struct Spans {
    spans: std::collections::BTreeMap<(u64, u64), ([u32; 14], std::collections::BTreeSet<u32>)>,
    by_slot: std::collections::HashMap<u32, (u64, u64)>,
    /// The bytes a placed texture takes, by resource description and first
    /// mip: worked out once for each (a title writes the views of a few
    /// thousand descriptions, thousands of times a frame).
    sizes: std::collections::HashMap<([u32; 14], u32), Option<u64>>,
}

impl Spans {
    /// The bytes of title memory the mips from `first` on of a placed
    /// texture with the resource description `desc` take.
    fn bytes(&mut self, desc: &[u32; 14], first: u32) -> Option<u64> {
        *self
            .sizes
            .entry((*desc, first))
            .or_insert_with(|| crate::resource::placed_bytes_from(desc, first))
    }

    /// The spans that overlap `[start, end)`.
    fn overlapping(&self, start: u64, end: u64) -> Vec<(u64, u64)> {
        // The spans of one start are one texture's, and no other texture's
        // reaches over it: only those of the start before may reach here.
        let before = self.spans.range(..(start, 0)).next_back().map(|(&(at, _), _)| at);
        let before = before.into_iter().flat_map(|at| self.spans.range((at, 0)..=(at, u64::MAX)));
        before
            .map(|(&span, _)| span)
            .filter(|span| span.1 > start)
            .chain(self.spans.range((start, 0)..(end, 0)).map(|(&span, _)| span))
            .collect()
    }

    /// The spans that say otherwise than that a texture with the resource
    /// description `desc` lies in `[start, end)`.
    fn against(&self, start: u64, end: u64, desc: &[u32; 14]) -> Vec<(u64, u64)> {
        let mut spans = self.overlapping(start, end);
        spans.retain(|span| {
            let same = span.0 == start && (span.1 == end || self.spans.get(span).is_some_and(|s| s.0 == *desc));
            !same
        });
        spans
    }

    /// Slot `index` holds a view of a texture with the resource description
    /// `desc` in `[start, end)`. `newer`: the title wrote it now, so the
    /// slots whose views describe that memory otherwise are stale: they are
    /// returned, and forgotten. Otherwise (a draw made the slot from what
    /// it held) a view that overlaps other spans is left out.
    fn note(&mut self, index: u32, start: u64, end: u64, desc: &[u32; 14], newer: bool) -> Vec<u32> {
        if self.by_slot.get(&index) == Some(&(start, end)) {
            return Vec::new();
        }
        self.forget(index);
        if end <= start {
            return Vec::new();
        }
        let mut stale = Vec::new();
        for span in self.against(start, end, desc) {
            if !newer {
                return stale;
            }
            if let Some((_, slots)) = self.spans.remove(&span) {
                for slot in slots {
                    self.by_slot.remove(&slot);
                    stale.push(slot);
                }
            }
        }
        self.spans.entry((start, end)).or_insert_with(|| (*desc, Default::default())).1.insert(index);
        self.by_slot.insert(index, (start, end));
        stale
    }

    /// Slot `index` holds no placed texture view any more.
    fn forget(&mut self, index: u32) {
        let Some(at) = self.by_slot.remove(&index) else {
            return;
        };
        if let Some(span) = self.spans.get_mut(&at) {
            span.1.remove(&index);
            if span.1.is_empty() {
                self.spans.remove(&at);
            }
        }
    }

    /// What the spans say of a view of a texture with the resource
    /// description `desc` in `[start, end)` in slot `index`, and the first
    /// span that speaks against it.
    fn claim(&self, index: u32, start: u64, end: u64, desc: &[u32; 14]) -> (Claim, Option<(u64, u64)>) {
        if self.by_slot.get(&index) == Some(&(start, end)) {
            return (Claim::Own, None);
        }
        let spans = self.against(start, end, desc);
        match spans.first() {
            None => (Claim::Free, None),
            Some(&span) if spans.iter().all(|s| s.0 == start) => (Claim::SameStart, Some(span)),
            Some(&span) => (Claim::Other, Some(span)),
        }
    }

    /// The slots whose texture's memory overlaps `[start, end)`.
    fn slots_in(&self, start: u64, end: u64) -> Vec<u32> {
        self.overlapping(start, end)
            .iter()
            .filter_map(|at| self.spans.get(at))
            .flat_map(|span| span.1.iter().copied())
            .collect()
    }
}

/// Whose the memory of a texture view is, by [`Spans`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    /// The slot's own.
    Own,
    /// No other texture's.
    Free,
    /// Another texture's that starts at the same address.
    SameStart,
    /// Another texture's.
    Other,
}

/// What the last whole look at the heap's placed texture views found
/// ([`HostHeap::texture_sweep`]): views; of them shown by their host slot;
/// in the queue; read through tables lately; without memory; then the ones
/// with memory that nothing brings up to date, by [`Claim`] (own, free,
/// same start, other), and of all those the ones whose host slot shows
/// another view. And since the start: the ones put back into the queue.
pub(crate) static SWEPT: [std::sync::atomic::AtomicU64; 11] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 11];

/// A look at the heap under way: the next slot, what it counted, and when
/// (milliseconds) it went on last.
static SWEEP: Mutex<(u32, [u64; 10], u64)> = Mutex::new((0, [0; 10], 0));

/// Title memory the runtime mapped, unmapped or is about to write since a
/// texture pass last took the ranges ([`remapped`]).
static REMAPPED: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// The lowest start and the highest end of any span of [`Spans`] so far
/// (the start is `u64::MAX` until there is one): what the runtime does to
/// memory outside is none of the texture pass's business.
static SPANNED: [std::sync::atomic::AtomicU64; 2] = [
    std::sync::atomic::AtomicU64::new(u64::MAX),
    std::sync::atomic::AtomicU64::new(0),
];

/// Since the start: slots whose view a newer view of other memory there
/// made stale; ranges of memory noted as mapped, unmapped or written;
/// ranges dropped because no pass took them; slots made due by such
/// ranges; and made slots whose memory was found unmapped (made null).
pub(crate) static STALE: [std::sync::atomic::AtomicU64; 5] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 5];

/// Whether a view written over memory that older views describe otherwise
/// makes those stale; `VICTORY_STALE_TEXTURES=0` leaves them as they are.
fn stale_out() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_STALE_TEXTURES").is_none_or(|v| v != "0"))
}

/// The runtime mapped or unmapped title memory `[at, at + len)`, or is
/// about to write it through the kernel (`crate::watch`): the placed
/// textures there may have arrived, gone, or changed with no descriptor
/// written. The range is kept for the next texture pass
/// ([`HostHeap::texture_slots_remapped`]). Called with the runtime's own
/// locks held: this locks one mutex of its own and calls nothing.
pub(crate) fn remapped(at: usize, len: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    /// Ranges kept until a pass takes them.
    /// (A world's first minutes note tens of thousands between two passes:
    /// 131,072 were dropped in one run at the 32,768 there were.)
    const MOST: usize = 1 << 18;
    let (start, end) = (at as u64, at.saturating_add(len) as u64);
    if end <= SPANNED[0].load(Relaxed) || start >= SPANNED[1].load(Relaxed) {
        return;
    }
    let mut ranges = REMAPPED.lock().unwrap_or_else(|e| e.into_inner());
    STALE[1].fetch_add(1, Relaxed);
    // Pages are mapped one after the other.
    if let Some(last) = ranges.last_mut()
        && last.1 == start
    {
        last.1 = end;
        return;
    }
    if ranges.len() >= MOST {
        // No pass runs (no shader that picks textures by index is bound):
        // the slots there are looked at when they are due, as before.
        STALE[2].fetch_add(ranges.len() as u64, Relaxed);
        ranges.clear();
    }
    ranges.push((start, end));
}

/// Frames for which a slot a draw read through a table counts as a table's
/// slot ([`HostHeap::tabled`]).
const TABLED_FRAMES: u64 = 240;

/// Frames a write of a table's slot waits in the queue for the draw that
/// reads it, before the slot is taken for one shaders pick by index.
/// Frames as `crate::frame::current` counts them: in its opening cutscene
/// The title begins two or three of them per frame it presents
/// (435 begun in the 353 s of run 52's, for some 140 presented).
const TABLED_GRACE: u64 = 8;

/// In [`HostHeap::tabled`]: a draw read the slot in that frame.
const TABLED_READ: u32 = 1 << 31;

/// Since the start: writes of texture views into slots that count as a
/// table's (due later), and slots that left the queue because a draw read
/// them through a table.
pub(crate) static TABLED_WRITES: [std::sync::atomic::AtomicU64; 2] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Whether slots draws read through tables leave the queue of textures
/// picked by index; `VICTORY_TABLED_TEXTURES=1` keeps them in it as
/// before.
fn tabled_out() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TABLED_TEXTURES").is_none_or(|v| v != "1"))
}

/// What the queue of textures picked by index knows of a title slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Waiting {
    /// The frame from which the slot is due.
    due: u64,
    /// The frame it was written in.
    written: u64,
    /// Which write of the queue put it here: the pass hands it back with
    /// what it did, so a slot written again meanwhile stays as written.
    write: u64,
    /// Looks in a row that found nothing to upload (its memory not mapped,
    /// or no more of it than at the last upload).
    looks: u8,
    /// Bytes of its memory that were mapped when it was last uploaded
    /// without all of it there; 0: not uploaded yet.
    mapped: u64,
    /// For a slot uploaded whole and looked at again
    /// ([`TextureQueue::recheck`]): where its memory is (address, bytes),
    /// the fingerprint it had at the upload, how many looks found it
    /// unchanged, and the frame of the upload. A fingerprint of 0: none.
    checked: (u64, u64, u64, u8, u64),
}

/// The queue of title slots whose texture view waits to be brought up to
/// date for shaders that pick textures by index: which are due, and in
/// which order (no host heap needed, so it is tested without one).
#[derive(Default)]
pub(crate) struct TextureQueue {
    slots: std::collections::BTreeMap<u32, Waiting>,
    writes: u64,
}

impl TextureQueue {
    /// A texture view was written into slot `index` in `frame`, due from
    /// frame `due`. `seen`: it was not written, a draw or the pass made
    /// the host slot from a view the queue may hold already, which then
    /// stays as it is.
    fn written(&mut self, index: u32, frame: u64, due: u64, seen: bool) {
        if seen && self.slots.contains_key(&index) {
            return;
        }
        self.writes += 1;
        let write = self.writes;
        self.slots.insert(index, Waiting { due, written: frame, write, looks: 0, mapped: 0, checked: (0, 0, 0, 0, 0) });
    }

    fn remove(&mut self, index: u32) {
        self.slots.remove(&index);
    }

    /// Up to `max` slots due at `frame`, each with its write number, in
    /// the order to serve them:
    /// - first the ones looked at before (their memory was not mapped, or
    ///   only part of it): the look is a query, and memory that has
    ///   arrived since is what the title streamed in last;
    /// - then the others, the newest write first and the oldest next, in
    ///   turns: slot order served the low slots of a backlog of
    ///   25,000-40,000 while the textures of the shot on screen waited
    ///   (run 52: 98% of the drawn pixels had no colour texture in the
    ///   first 3D frames).
    fn due(&self, frame: u64, max: usize) -> Vec<(u32, u64)> {
        let mut again: Vec<(u64, u32, u64)> = Vec::new();
        let mut fresh: Vec<(u64, u32, u64)> = Vec::new();
        for (&index, w) in self.slots.iter().filter(|(_, w)| w.due <= frame) {
            let class = if w.looks > 0 || w.mapped > 0 { &mut again } else { &mut fresh };
            class.push((w.written, index, w.write));
        }
        // By the number of the write, counted over the run.
        let newest_first = |a: &(u64, u32, u64), b: &(u64, u32, u64)| b.2.cmp(&a.2);
        again.sort_unstable_by(newest_first);
        fresh.sort_unstable_by(newest_first);
        let mut order: Vec<(u32, u64)> = again.iter().map(|&(_, index, write)| (index, write)).collect();
        let (mut new, mut old) = (0, fresh.len());
        while new < old && order.len() < max {
            order.push((fresh[new].1, fresh[new].2));
            new += 1;
            if new < old && order.len() < max {
                old -= 1;
                order.push((fresh[old].1, fresh[old].2));
            }
        }
        order.truncate(max);
        order
    }

    /// The entry of `index` if it is still the write the pass was handed.
    fn same(&mut self, index: u32, write: u64) -> Option<&mut Waiting> {
        self.slots.get_mut(&index).filter(|w| w.write == write)
    }

    /// Slot `index` is up to date: out of the queue until written again.
    fn done(&mut self, index: u32, write: u64) {
        if self.same(index, write).is_some() {
            self.slots.remove(&index);
        }
    }

    /// Nothing to upload at slot `index` now: looked at again after 1, 2,
    /// 4, ... frames, at most `most`.
    fn later(&mut self, index: u32, write: u64, frame: u64, most: u64) {
        if let Some(w) = self.same(index, write) {
            w.due = frame + (1u64 << w.looks.min(16)).min(most.max(1));
            w.looks = w.looks.saturating_add(1);
        }
    }

    /// Slot `index` was uploaded with `mapped` bytes of its memory there,
    /// not all of it: looked at again from frame `due`.
    fn partly(&mut self, index: u32, write: u64, due: u64, mapped: u64) {
        if let Some(w) = self.same(index, write) {
            (w.due, w.looks, w.mapped) = (due, 0, mapped.max(1));
        }
    }

    /// Slot `index` was uploaded whole from memory `at` (address, bytes)
    /// with fingerprint `print`: looked at again from frame `due`, for a
    /// title that fills a texture's memory after it wrote the view (the
    /// `looks`-th such look; `uploaded` is the upload's frame).
    #[allow(clippy::too_many_arguments)]
    fn recheck(&mut self, index: u32, write: u64, due: u64, at: (u64, u64), print: u64, looks: u8, uploaded: u64) {
        if let Some(w) = self.same(index, write) {
            (w.due, w.looks, w.mapped) = (due, 1, 0);
            w.checked = (at.0, at.1, print, looks, uploaded);
        }
    }

    /// What [`TextureQueue::recheck`] noted of slot `index`, if it is
    /// waiting for such a look.
    fn checked(&self, index: u32) -> Option<(u64, u64, u64, u8, u64)> {
        self.slots.get(&index).map(|w| w.checked).filter(|c| c.2 != 0)
    }

    /// The memory slot `index`'s texture lies in was mapped, unmapped or
    /// written in `frame`: the slot is looked at from frame `due` as one
    /// written now, whatever was known of it (a pass at work on it does
    /// not take it out).
    fn changed(&mut self, index: u32, frame: u64, due: u64) {
        self.writes += 1;
        let write = self.writes;
        let due = self.slots.get(&index).map_or(due, |w| w.due.min(due));
        self.slots.insert(index, Waiting { due, written: frame, write, looks: 0, mapped: 0, checked: (0, 0, 0, 0, 0) });
    }

    /// Bytes of slot `index`'s memory that were mapped at its last upload
    /// (0: not uploaded, or all of it was).
    fn mapped(&self, index: u32) -> u64 {
        self.slots.get(&index).map_or(0, |w| w.mapped)
    }

    /// For the frame line: slots in the queue; of them due at `frame`;
    /// waiting for their memory (looked at, never uploaded); uploaded in
    /// part; and the frames the oldest due one has waited since its write.
    fn census(&self, frame: u64) -> [u64; 5] {
        let mut c = [self.slots.len() as u64, 0, 0, 0, 0];
        for w in self.slots.values() {
            if w.mapped > 0 {
                c[3] += 1;
            } else if w.looks > 0 {
                c[2] += 1;
            }
            if w.due <= frame {
                c[1] += 1;
                c[4] = c[4].max(frame.saturating_sub(w.written));
            }
        }
        c
    }
}

/// Whether `view` is of a texture the title may fill itself: a texture it
/// placed in memory, or a texture object.
pub(crate) fn is_texture_view(view: &View) -> bool {
    match view {
        View::Placed(false, resource, _) => resource[0] != 1,
        View::Shader(r, _) => crate::resource::buffer_bytes(*r as crate::object::This).is_none(),
        _ => false,
    }
}

/// Buffer views left null because the host would refuse them
/// ([`buffer_view_ok`]), since the start.
pub(crate) static REFUSED_VIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether the host device is gone, and why (the first time it is asked
/// after that): for passes that make many views, to name the one that did
/// it.
pub(crate) fn device_removed() -> Option<String> {
    let host = host()?;
    // SAFETY: a plain query.
    let reason = unsafe { host.device.GetDeviceRemovedReason() };
    reason.err().map(|e| e.to_string())
}

/// Writes of title view-heap slots with a placed texture view and with a
/// view of a texture object, since the start (diagnostic counts).
pub(crate) static PLACED_TEXTURE_SLOTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static OBJECT_TEXTURE_SLOTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bytes per element of a buffer view, as the console's descriptor holds
/// them for indexed reads (structured: the stride; raw: 1, the index is the
/// byte; typed: the format's); `None` if `view` is not a buffer view.
fn view_stride(view: &View) -> Option<u32> {
    const BUFFER: u32 = 1;
    let (words, unordered): (&[u32; 10], bool) = match view {
        View::Shader(_, Some(d)) if d[1] == BUFFER => (d, false),
        View::Unordered(_, _, Some(d)) if d[1] == BUFFER => (d, true),
        View::Placed(unordered, resource, d) if resource[0] == BUFFER => {
            (d[..10].try_into().ok()?, *unordered)
        }
        _ => return None,
    };
    // SAFETY: buffer views' union members.
    let (stride, raw, format) = unsafe {
        if unordered {
            let d = uav_desc(words);
            let b = d.Anonymous.Buffer;
            (b.StructureByteStride, b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0, d.Format)
        } else {
            let d = srv_desc(words);
            let b = d.Anonymous.Buffer;
            (b.StructureByteStride, b.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0, d.Format)
        }
    };
    Some(if stride > 0 {
        stride
    } else if raw {
        1
    } else {
        crate::resource::element(format.0 as u32).map_or(0, |(bytes, _)| bytes as u32)
    })
}

/// The console descriptor a translated shader would read for the title's
/// `view`, for the descriptor-words table ([`HostHeap::words`]): a buffer
/// view's title address, stride field and records; an image view's first
/// level's size, layers or depth and levels, from the title's resource
/// description, and its memory's address. Samplers and nothing: none.
fn view_shape(view: Option<&View>) -> Shape {
    let Some(view) = view else {
        return Shape::None;
    };
    let (unordered, words) = match view {
        &View::Constant(address, size) => {
            return Shape::Buffer {
                address,
                stride: 0,
                records: size,
                format: 0,
            };
        }
        View::Sampler(_) | View::ConsoleSampler(_) => return Shape::None,
        View::Placed(unordered, _, desc) => (*unordered, desc[..10].try_into().ok()),
        View::Shader(_, d) => (false, *d),
        View::Unordered(_, _, d) => (true, *d),
    };
    if let Some((address, bytes)) = buffer_range(view) {
        // The console's stride field: a raw view's is 0 (records in bytes),
        // others' the element size (records in elements).
        let stride = view_stride(view).unwrap_or(0);
        let raw = words.is_none_or(|w: [u32; 10]| raw_words(unordered, &w));
        let (stride, records) = if raw || stride <= 1 {
            (0, bytes)
        } else {
            (stride, bytes / u64::from(stride))
        };
        // A typed view of three 32-bit channels: the format, for the
        // translation's float3 addressing (`f3_words_format`).
        let format = match words {
            Some(w) if !raw && stride == 12 => {
                f3_words_format(if unordered { uav_desc(&w).Format } else { srv_desc(&w).Format })
            }
            _ => 0,
        };
        return Shape::Buffer {
            address,
            stride,
            records: u32::try_from(records).unwrap_or(u32::MAX),
            format,
        };
    }
    // The texture's description, in the host's 14-dword layout, and its
    // memory.
    let (resource, address): ([u32; 14], u64) = match view {
        View::Placed(false, resource, desc) => (*resource, u64::from(desc[10]) | u64::from(desc[11]) << 32),
        View::Shader(r, _) | View::Unordered(r, _, _) => {
            match crate::resource::texture_memory(*r as crate::object::This) {
                // SAFETY: the description is 56 bytes in the host's layout.
                Some((p, d)) => (unsafe { std::ptr::from_ref(&d).cast::<[u32; 14]>().read_unaligned() }, p as u64),
                None => return Shape::None,
            }
        }
        _ => return Shape::None,
    };
    texture_shape(words, &resource, unordered, address)
}

/// Whether buffer view description `words` is raw.
fn raw_words(unordered: bool, words: &[u32; 10]) -> bool {
    if unordered {
        // SAFETY: a buffer view's union member.
        let b = unsafe { uav_desc(words).Anonymous.Buffer };
        b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0
    } else {
        // SAFETY: a buffer view's union member.
        let b = unsafe { srv_desc(words).Anonymous.Buffer };
        b.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0
    }
}

/// [`view_shape`] of a texture view (description `words`; none: the whole
/// resource) of a texture described by `resource` (`D3D12_RESOURCE_DESC`
/// as dwords: dimension, alignment, width, height, depth or layers and
/// levels, format, samples).
fn texture_shape(words: Option<[u32; 10]>, resource: &[u32; 14], unordered: bool, address: u64) -> Shape {
    const TEXTURE1D: u32 = 2;
    const TEXTURE3D: u32 = 4;
    let (w, h) = (resource[4], resource[6].max(1));
    let layers = (resource[7] & 0xFFFF).max(1);
    let mips = (resource[7] >> 16).max(1);
    let samples = resource[9].max(1);
    let level = |x: u32, m: u32| (x >> m).max(1);
    let count = |n: u32, first: u32| {
        if n == u32::MAX || n == 0 {
            mips.saturating_sub(first).max(1)
        } else {
            n
        }
    };
    let whole = |n: u32, first: u32| {
        if n == u32::MAX || n == 0 {
            layers.saturating_sub(first).max(1)
        } else {
            n
        }
    };
    let shape = |ty, m: u32, depth, levels| Shape::Image {
        ty,
        width: level(w, m),
        height: level(h, m),
        depth,
        levels,
        address,
    };
    let Some(words) = words else {
        // The resource's own view: its whole shape.
        let levels = if unordered { 1 } else { mips };
        return match resource[0] {
            TEXTURE1D if layers > 1 => shape(ImageType::D1Array, 0, layers, levels),
            TEXTURE1D => shape(ImageType::D1, 0, 1, levels),
            TEXTURE3D => shape(ImageType::D3, 0, layers, levels),
            _ if samples > 1 && layers > 1 => shape(ImageType::D2MsaaArray, 0, layers, samples),
            _ if samples > 1 => shape(ImageType::D2Msaa, 0, 1, samples),
            _ if layers > 1 => shape(ImageType::D2Array, 0, layers, levels),
            _ => shape(ImageType::D2, 0, 1, levels),
        };
    };
    if unordered {
        let u = uav_desc(&words);
        // SAFETY: the member the view dimension names.
        unsafe {
            match u.ViewDimension {
                D3D12_UAV_DIMENSION_TEXTURE1D => shape(ImageType::D1, u.Anonymous.Texture1D.MipSlice, 1, 1),
                D3D12_UAV_DIMENSION_TEXTURE1DARRAY => {
                    let a = u.Anonymous.Texture1DArray;
                    let n = whole(a.ArraySize, a.FirstArraySlice);
                    shape(ImageType::D1Array, a.MipSlice, n, 1)
                }
                D3D12_UAV_DIMENSION_TEXTURE2DARRAY => {
                    let a = u.Anonymous.Texture2DArray;
                    let n = whole(a.ArraySize, a.FirstArraySlice);
                    shape(ImageType::D2Array, a.MipSlice, n, 1)
                }
                D3D12_UAV_DIMENSION_TEXTURE3D => {
                    let a = u.Anonymous.Texture3D;
                    let depth = if a.WSize == u32::MAX || a.WSize == 0 {
                        level(layers, a.MipSlice).saturating_sub(a.FirstWSlice).max(1)
                    } else {
                        a.WSize
                    };
                    shape(ImageType::D3, a.MipSlice, depth, 1)
                }
                _ => shape(ImageType::D2, u.Anonymous.Texture2D.MipSlice, 1, 1),
            }
        }
    } else {
        let v = srv_desc(&words);
        // SAFETY: the member the view dimension names.
        unsafe {
            match v.ViewDimension {
                D3D12_SRV_DIMENSION_TEXTURE1D => {
                    let a = v.Anonymous.Texture1D;
                    let m = a.MostDetailedMip;
                    shape(ImageType::D1, m, 1, count(a.MipLevels, m))
                }
                D3D12_SRV_DIMENSION_TEXTURE1DARRAY => {
                    let a = v.Anonymous.Texture1DArray;
                    let m = a.MostDetailedMip;
                    let n = whole(a.ArraySize, a.FirstArraySlice);
                    shape(ImageType::D1Array, m, n, count(a.MipLevels, m))
                }
                D3D12_SRV_DIMENSION_TEXTURE2DARRAY => {
                    let a = v.Anonymous.Texture2DArray;
                    let m = a.MostDetailedMip;
                    let n = whole(a.ArraySize, a.FirstArraySlice);
                    shape(ImageType::D2Array, m, n, count(a.MipLevels, m))
                }
                D3D12_SRV_DIMENSION_TEXTURE2DMS => shape(ImageType::D2Msaa, 0, 1, samples),
                D3D12_SRV_DIMENSION_TEXTURE2DMSARRAY => {
                    let a = v.Anonymous.Texture2DMSArray;
                    let n = whole(a.ArraySize, a.FirstArraySlice);
                    shape(ImageType::D2MsaaArray, 0, n, samples)
                }
                D3D12_SRV_DIMENSION_TEXTURE3D => {
                    let a = v.Anonymous.Texture3D;
                    let m = a.MostDetailedMip;
                    shape(ImageType::D3, m, level(layers, m), count(a.MipLevels, m))
                }
                D3D12_SRV_DIMENSION_TEXTURECUBE => {
                    let a = v.Anonymous.TextureCube;
                    let m = a.MostDetailedMip;
                    shape(ImageType::Cube, m, 6, count(a.MipLevels, m))
                }
                D3D12_SRV_DIMENSION_TEXTURECUBEARRAY => {
                    let a = v.Anonymous.TextureCubeArray;
                    let m = a.MostDetailedMip;
                    let faces = if a.NumCubes == u32::MAX { layers } else { 6 * a.NumCubes };
                    shape(ImageType::Cube, m, faces, count(a.MipLevels, m))
                }
                _ => {
                    let a = v.Anonymous.Texture2D;
                    let m = a.MostDetailedMip;
                    shape(ImageType::D2, m, 1, count(a.MipLevels, m))
                }
            }
        }
    }
}

#[cfg(test)]
mod words_tests {
    use super::*;

    /// A `D3D12_RESOURCE_DESC` as dwords: a 2D texture of `w` x `h`,
    /// `layers` layers and `mips` levels.
    fn texture(w: u32, h: u32, layers: u32, mips: u32) -> [u32; 14] {
        let mut r = [0; 14];
        r[0] = 3;
        r[4] = w;
        r[6] = h;
        r[7] = layers | mips << 16;
        r[9] = 1;
        r
    }

    #[test]
    fn a_texture_views_words_hold_its_first_levels_size() {
        // Whole resource: 64 x 32, 7 levels.
        let s = texture_shape(None, &texture(64, 32, 1, 7), false, 0x1_0000);
        assert_eq!(
            s,
            Shape::Image {
                ty: ImageType::D2,
                width: 64,
                height: 32,
                depth: 1,
                levels: 7,
                address: 0x1_0000,
            }
        );
        // A 2D view from level 2, all levels after: 16 x 8, 5 levels.
        let mut d = [0u32; 10];
        d[1] = D3D12_SRV_DIMENSION_TEXTURE2D.0 as u32;
        d[4] = 2;
        d[5] = u32::MAX;
        let s = texture_shape(Some(d), &texture(64, 32, 1, 7), false, 0);
        assert!(matches!(s, Shape::Image { width: 16, height: 8, levels: 5, ty: ImageType::D2, .. }), "{s:?}");
        // A cube view: six faces.
        d[1] = D3D12_SRV_DIMENSION_TEXTURECUBE.0 as u32;
        d[4] = 0;
        d[5] = 1;
        let s = texture_shape(Some(d), &texture(16, 16, 6, 1), false, 0);
        assert!(matches!(s, Shape::Image { depth: 6, levels: 1, ty: ImageType::Cube, .. }), "{s:?}");
    }

    #[test]
    fn a_constant_views_words_hold_its_address_and_size() {
        let s = view_shape(Some(&View::Constant(0x2_0000_0100, 256)));
        assert_eq!(console_words(&s)[..3], [0x0000_0100, 2, 256]);
        assert_eq!(view_shape(None), Shape::None);
    }

    /// Float3 views' words carry the 32_32_32 format while the switch is on
    /// (off by default), other formats none; with it off every format gets none.
    #[test]
    fn float3_views_words_carry_their_format() {
        if f3_view_fix() {
            assert_eq!(f3_words_format(DXGI_FORMAT_R32G32B32_UINT), 72);
            assert_eq!(f3_words_format(DXGI_FORMAT_R32G32B32_SINT), 73);
            assert_eq!(f3_words_format(DXGI_FORMAT_R32G32B32_FLOAT), 74);
        } else {
            assert_eq!(f3_words_format(DXGI_FORMAT_R32G32B32_FLOAT), 0);
        }
        assert_eq!(f3_words_format(DXGI_FORMAT_R32G32B32A32_FLOAT), 0);
        assert_eq!(f3_words_format(DXGI_FORMAT_R32_FLOAT), 0);
    }
}

/// Host heaps' GPU ranges (start, bytes, bytes per slot, sampler), for
/// device-removed reports: a shader indexing a heap past its end faults.
static GPU_RANGES: Mutex<Vec<(u64, u64, u64, bool)>> = Mutex::new(Vec::new());

/// For device-removed reports: where host GPU address `va` lies against
/// the host descriptor heaps.
pub(crate) fn near_heaps(va: u64) -> String {
    let ranges = GPU_RANGES.lock().unwrap_or_else(|e| e.into_inner());
    let mut text = String::new();
    for &(start, bytes, step, sampler) in ranges.iter() {
        let kind = if sampler { "sampler" } else { "view" };
        let place = if va < start {
            format!("{:#x} below", start - va)
        } else if va < start + bytes {
            format!("inside, slot {}", (va - start) / step)
        } else {
            format!("{:#x} past its end (slot {})", va - start - bytes, (va - start) / step)
        };
        text.push_str(&format!("; {kind} heap {start:#x}+{bytes:#x}: {place}"));
    }
    text
}

/// The host's limit for shader-visible view heaps on resource binding
/// tiers 1 and 2; tier 3 hardware may take more.
const PORTABLE_VIEWS: u32 = 1_000_000;

/// The views of the title's heap, for the limit shaders translated
/// before that heap is made have ([`probe_view_limit`]).
/// shortcut: this title's; a larger heap would lose its last views.
const TITLE_VIEWS: u32 = 1_245_184;

/// Asks the host, once, whether it caps view heaps at [`PORTABLE_VIEWS`],
/// and if so tells the translator now (`set_view_heap_limit`) instead of
/// when the title's heap is made: a shader keeps the limit it was
/// translated with, and the title makes 25 pipelines (their
/// shaders translated at once) before it makes its view heap. Those would
/// read a view index past the heap unclamped, the read that hangs NVIDIA.
/// Called before a shader is translated.
/// shortcut: the limit is the capped heap's last slot whatever size the
/// title's heap turns out to have; a heap that fits is not capped and
/// keeps no null slot there (an index past it is as undefined as before).
///
/// A capped host also gets a heap map (unless `VICTORY_HEAP_MAP=0`): the
/// translator looks bindless views past the identity window up in it
/// (`set_view_heap_map`), and from then on every view heap of more title
/// slots than the window is laid out for it ([`MapLayout`], whatever its
/// size: the map's slot is fixed in the translations). Also called before
/// such a heap is made, so that a heap made before the first translation
/// agrees with the shaders.
pub(crate) fn probe_view_limit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(host) = host() else { return };
        // SAFETY: plain heap creation; the heap is released at once.
        let made: windows::core::Result<ID3D12DescriptorHeap> = unsafe {
            host.device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: PORTABLE_VIEWS + 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
        };
        if made.is_err() {
            tracing::info!("the host caps view heaps at {PORTABLE_VIEWS}: view indices are clamped from the first shader on");
            victory_shader::emit::set_view_heap_limit(PORTABLE_VIEWS - 1);
            if heap_map_on() {
                let window = window();
                victory_shader::emit::set_view_heap_map(Some((window, MAP_SLOT)));
                tracing::info!(
                    window,
                    map_slot = MAP_SLOT,
                    "heap map: bindless views past the identity window are looked up (VICTORY_HEAP_MAP=0: off)"
                );
            }
        } else {
            // A read past the end of an uncapped heap hangs AMD's GPU too
            // (RX 7800 XT: a page fault 480 MiB past the heap's start).
            victory_shader::emit::set_view_heap_limit(TITLE_VIEWS + DYNAMIC - 1);
        }
    });
}

/// Slots past the title's in a view heap, for tables a draw needs made
/// afresh (constant buffers copied per draw). A list holds the slots it
/// took until it is reset (its submissions have run) or released.
/// 2^16 ran out in the world ("no free dynamic descriptor slots", run 11;
/// upstream PR #336 takes 2^18). On a host heap capped at 1,000,000 views
/// (NVIDIA) these come out of the title's slots, whose highest seen is
/// about 720,000: 2^17 left 868,000 of them, and ran out in the opening
/// cutscene (a frame asks for up to 16,000 tables while the frame before
/// still holds its own; a draw refused its table read slots never made
/// for it, and the card stopped within frames). 2^18 leaves 737,000; a
/// table past them is made afresh in these slots like any other.
pub(crate) const DYNAMIC: u32 = 1 << 18;

// The heap map leaves the fork's dynamic slots and guard slot in place.
const _: () = assert!(MAP_SLOT + 1 + DYNAMIC == PORTABLE_VIEWS - 1);

/// The size of a view heap smaller than the limit shaders clamp view
/// indices to: up to and with that slot, which stays null (it lies past
/// the dynamic slots). Without it an index between the heap's end and the
/// clamp read past the heap, the read that hangs NVIDIA. Adapted from
/// Zettrox/ztrx#20 (rivaldodev). `VICTORY_HEAP_PAD_NULL=0`: the heap is
/// made to the size asked, as before.
fn padded_size(sampler: bool, asked: u32) -> Option<u32> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if sampler || !*ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_PAD_NULL").is_none_or(|v| v != "0")) {
        return None;
    }
    #[cfg(not(test))]
    probe_view_limit();
    padded_to(asked, victory_shader::emit::view_heap_limit())
}

fn padded_to(asked: u32, limit: u32) -> Option<u32> {
    (limit != 0 && asked <= limit).then(|| limit + 1)
}

impl HostHeap {
    pub(crate) fn new(sampler: bool, count: u32) -> Option<Self> {
        let host = host()?;
        let kind = if sampler {
            D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER
        } else {
            D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
        };
        let make = |count: u32| -> windows::core::Result<ID3D12DescriptorHeap> {
            // SAFETY: plain heap creation.
            unsafe {
                host.device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: kind,
                        NumDescriptors: count,
                        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                        NodeMask: 0,
                    })
            }
        };
        let extra = if sampler { 0 } else { DYNAMIC };
        let title_count = count;
        // Whether the translator looks bindless views up in a heap map, as
        // it does from the first translation on (asked now if none ran).
        #[cfg(not(test))]
        if !sampler && heap_map_on() {
            probe_view_limit();
        }
        let layout = (!sampler)
            .then(heap_map)
            .flatten()
            .filter(|&(window, _)| count > window)
            .map(|(window, slot)| MapLayout::new(count, window, slot));
        let made = match layout {
            Some(l) => {
                tracing::warn!(
                    count,
                    window = l.window,
                    pool = l.pool,
                    map_slot = l.slot,
                    "host view heap with a heap map: title slots past the window that hold views get pool slots"
                );
                make(l.size()).map(|heap| (heap, l.window, l.size()))
            }
            None => match padded_size(sampler, count + extra) {
                Some(size) => make(size).map(|heap| (heap, count, size)),
                None => make(count + extra).map(|heap| (heap, count, count + extra)),
            },
        };
        let (heap, count, size) = match made {
            Ok(made) => made,
            Err(e) if layout.is_some() => {
                tracing::warn!(error = %e, count, "no host view heap with a heap map");
                return None;
            }
            Err(e) if !sampler && count + extra > PORTABLE_VIEWS => {
                // shortcut: descriptors past the host's limit are left out; a title that reaches
                // them needs remapping. The heap's last slot stays null, and shaders read it for
                // any view index from there on (`set_view_heap_limit`): a read past the heap's
                // end hung NVIDIA's GPU (the title's heap is a little over 1 000 000 views).
                tracing::warn!(error = %e, count, "host view heap capped at {PORTABLE_VIEWS}; views past it read a null slot");
                victory_shader::emit::set_view_heap_limit(PORTABLE_VIEWS - 1);
                (make(PORTABLE_VIEWS).ok()?, PORTABLE_VIEWS - extra - 1, PORTABLE_VIEWS)
            }
            Err(e) => {
                tracing::warn!(error = %e, count, sampler, "no host descriptor heap");
                return None;
            }
        };
        // SAFETY: a live heap.
        let (cpu, step) = unsafe {
            (
                heap.GetCPUDescriptorHandleForHeapStart().ptr,
                host.device.GetDescriptorHandleIncrementSize(kind),
            )
        };
        tracing::debug!(count, sampler, "host descriptor heap");
        if !sampler && count + extra != TITLE_VIEWS + DYNAMIC && size == count + extra {
            tracing::warn!(count, "a view heap of another size than the limit shaders have");
        }
        // The last slot stays null: what shaders read for an index past the heap
        // (victory-amd). In a heap without a cap that is the last dynamic slot; a
        // capped heap, with a heap map or without, keeps its guard slot past them.
        let guard_in_dynamic = layout.is_none() && size == count + extra;
        let mut taken = vec![false; DYNAMIC as usize];
        taken[DYNAMIC as usize - 1] = guard_in_dynamic;
        // SAFETY: a live shader-visible heap.
        let gpu = unsafe { heap.GetGPUDescriptorHandleForHeapStart().ptr };
        GPU_RANGES.lock().unwrap_or_else(|e| e.into_inner()).push((
            gpu,
            u64::from(size) * u64::from(step),
            u64::from(step),
            sampler,
        ));
        // The heap map: an entry per title slot past the window, zeroed (no
        // slot: the guard).
        let map = layout.and_then(|l| {
            let bytes = u64::from((title_count - l.window).max(1)) * 4;
            let Some((buffer, at)) = host.upload_buffer(bytes) else {
                tracing::warn!(bytes, "no heap map: title slots past the window read the null slot");
                return None;
            };
            // SAFETY: the buffer's whole mapping.
            unsafe { std::ptr::write_bytes(at, 0, bytes as usize) };
            crate::host::name(&buffer, || "heap map".to_owned());
            Some(HeapMap { buffer, at: at as usize })
        });
        let strides = (!sampler)
            .then(|| {
                let bytes = u64::from(victory_shader::emit::SLOT_STRIDES_DWORDS) * 4;
                let (buffer, data) = host.upload_buffer(bytes)?;
                // SAFETY: the buffer's whole mapping; a live buffer.
                let gpu = unsafe {
                    std::ptr::write_bytes(data, 0, bytes as usize);
                    buffer.GetGPUVirtualAddress()
                };
                crate::host::name(&buffer, || "element strides by view heap slot".to_owned());
                Some((buffer, gpu, data as usize))
            })
            .flatten();
        // Title-keyed state covers the title's slots past the window too.
        let (pool, dynamic_first, titled) = match layout {
            Some(l) => (l.pool, l.dynamic_first(), title_count),
            None => (0, count, count),
        };
        let words = (!sampler)
            .then(|| {
                let bytes = u64::from(victory_shader::emit::SLOT_STRIDES_COUNT) * u64::from(WORDS) * 4;
                let (buffer, data) = host.upload_buffer(bytes)?;
                // SAFETY: the buffer's whole mapping; a live buffer.
                let gpu = unsafe {
                    std::ptr::write_bytes(data, 0, bytes as usize);
                    buffer.GetGPUVirtualAddress()
                };
                crate::host::name(&buffer, || "console descriptor dwords by view heap slot".to_owned());
                Some((buffer, gpu, data as usize))
            })
            .flatten();
        let h = HostHeap {
            heap,
            cpu,
            step,
            count,
            dynamic: extra,
            dynamic_first,
            title_count,
            sampler,
            made: Mutex::new(Made {
                keys: vec![0; (count + pool) as usize],
                map: map
                    .is_some()
                    .then(|| SlotMap::new(count, title_count, count, pool)),
            }),
            map,
            title_start: AtomicU64::new(0),
            reclaimed: AtomicU64::new(u64::MAX),
            kept: Mutex::new(vec![None; (dynamic_first + extra) as usize]),
            next: Mutex::new((0, taken)),
            free: std::sync::atomic::AtomicU32::new(DYNAMIC - u32::from(guard_in_dynamic)),
            too_long: std::sync::atomic::AtomicU32::new(u32::MAX),
            strides,
            words,
            buffers: Mutex::default(),
            changed: Mutex::default(),
            textures: Mutex::default(),
            tabled: (0..if sampler { 0 } else { titled })
                .map(|_| std::sync::atomic::AtomicU32::new(0))
                .collect(),
            placed: Mutex::default(),
        };
        // A slot never written is undefined: shaders reading one (a slot
        // the title wrote in a way we do not know, or a table entry no
        // used-resource entry names) read a wild address. Every slot starts
        // null instead.
        let started = std::time::Instant::now();
        if let Err(e) = h.fill_null(kind, size) {
            tracing::warn!(error = %e, "host descriptor heap not made null");
        }
        tracing::debug!(ms = started.elapsed().as_millis(), "host descriptor heap made null");
        if let Some(l) = layout {
            // The map's raw view (`raw_at` holds the buffer), or one that
            // reads zeros: every lookup then gives the guard slot.
            match &h.map {
                Some(m) => h.raw_at(l.slot, &m.buffer, 0, u64::from(title_count - l.window) * 4),
                None => h.null_raw_at(l.slot),
            }
        }
        Some(h)
    }

    /// Makes the first `total` slots null: one null slot in a CPU-side heap,
    /// doubled by copies to a block, the block copied over the heap.
    fn fill_null(&self, kind: D3D12_DESCRIPTOR_HEAP_TYPE, total: u32) -> windows::core::Result<()> {
        const BLOCK: u32 = 1 << 16;
        let Some(host) = host() else { return Ok(()) };
        // SAFETY: plain heap creation and copies inside live heaps.
        unsafe {
            let block: ID3D12DescriptorHeap =
                host.device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: kind,
                    NumDescriptors: BLOCK,
                    ..Default::default()
                })?;
            let start = block.GetCPUDescriptorHandleForHeapStart();
            let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: start.ptr + (i * self.step) as usize,
            };
            if self.sampler {
                null_sampler(start);
            } else {
                null_view(start);
            }
            let mut filled = 1;
            while filled < BLOCK {
                let n = filled.min(BLOCK - filled);
                host.device.CopyDescriptorsSimple(n, at(filled), start, kind);
                filled += n;
            }
            let mut done = 0;
            while done < total {
                let n = BLOCK.min(total - done);
                host.device.CopyDescriptorsSimple(n, self.cpu(done), start, kind);
                done += n;
            }
        }
        Ok(())
    }

    /// A plain point sampler at slot `index`, for sampler slots without a
    /// view.
    fn null_sampler(&self, index: u32) {
        null_sampler(self.cpu(index));
    }
}

/// A plain point sampler at `at`.
fn null_sampler(at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let desc = D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        ..Default::default()
    };
    // SAFETY: a slot of a sampler heap of ours.
    unsafe { host.device.CreateSampler(&desc, at) };
}

impl HostHeap {
    /// `n` consecutive free dynamic slots (view heaps only), taken until
    /// [`HostHeap::release`]: the first's index.
    pub(crate) fn dynamic(&self, n: u32) -> Option<u32> {
        if self.sampler || n == 0 || n > self.dynamic {
            return None;
        }
        #[cfg(feature = "diagnostics")]
        let started = std::time::Instant::now();
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::DynamicAsked, 1);
        let mut guard = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let (next, taken) = &mut *guard;
        let enough = self.free.load(std::sync::atomic::Ordering::Relaxed) >= n
            && n < self.too_long.load(std::sync::atomic::Ordering::Relaxed);
        let n = n as usize;
        // Search from `next` on, then from the start.
        let starts = (*next as usize..self.dynamic as usize).chain(0..*next as usize);
        let mut run = 0;
        let mut found = None;
        let mut last = usize::MAX;
        for at in starts.take(if enough { DYNAMIC as usize } else { 0 }) {
            // A run cannot wrap past the end.
            if at != last.wrapping_add(1) {
                run = 0;
            }
            last = at;
            run = if taken[at] { 0 } else { run + 1 };
            if run == n {
                found = Some(at + 1 - n);
                break;
            }
        }
        let Some(first) = found else {
            if enough {
                self.too_long.fetch_min(n as u32, std::sync::atomic::Ordering::Relaxed);
            }
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!(n, "no free dynamic descriptor slots: a table is not made"));
            #[cfg(feature = "diagnostics")]
            {
                use crate::copystat::{C, add};
                add(C::DynamicRefused, 1);
                add(C::DynamicNs, started.elapsed().as_nanos() as u64);
            }
            return None;
        };
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::DynamicNs, started.elapsed().as_nanos() as u64);
        taken[first..first + n].fill(true);
        self.free.fetch_sub(n as u32, std::sync::atomic::Ordering::Relaxed);
        *next = ((first + n) % DYNAMIC as usize) as u32;
        Some(self.dynamic_first + first as u32)
    }

    /// The title slots this heap mirrors at their own index (tables read in
    /// place reach only these: one that reaches past them is made afresh);
    /// fewer than the title's when the host caps its size, and with a heap
    /// map the identity window.
    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    /// The host slot title slot `title` is made in (`made` locked): its own
    /// index below `count`; with a heap map past it, its pool slot, taken
    /// now if it has none (then `true`: [`HostHeap::publish`] writes its
    /// map entry once the slot is made). `None` past the title's slots, or
    /// with the pool used up even after [`HostHeap::reclaim`] (the title
    /// slot then reads the guard slot).
    fn host_slot(&self, made: &mut Made, title: u32) -> Option<(u32, bool)> {
        if title < self.count {
            return Some((title, false));
        }
        if title >= self.title_count {
            return None;
        }
        {
            let map = made.map.as_mut()?;
            if let Some(host) = map.host(title) {
                return Some((host, false));
            }
            if let Some(host) = map.assign(title, crate::host::submissions().1) {
                return Some((host, true));
            }
        }
        if self.reclaim(made) > 0
            && let Some(host) = made.map.as_mut()?.assign(title, crate::host::submissions().1)
        {
            return Some((host, true));
        }
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::warn!(title, "heap map pool used up: title slots past the window read the null slot");
        });
        None
    }

    /// Writes title slot `title`'s heap map entry (`made` locked), after
    /// its pool slot was made.
    fn publish(&self, made: &Made, title: u32) {
        let (Some(map), Some(m)) = (&made.map, &self.map) else {
            return;
        };
        let Some(i) = title.checked_sub(map.window()).filter(|&i| i < map.len()) else {
            return;
        };
        // The descriptor first, then the entry naming it.
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        // SAFETY: entry `i` of the mapped heap map.
        unsafe { (m.at as *mut u32).add(i as usize).write_volatile(map.entry(title)) };
    }

    /// Gives back title slot `title`'s pool slot (`made` locked), if it has
    /// one: its map entry is zeroed (the guard slot), and the slot is
    /// handed out again once the submissions that may have read the old
    /// entry are done. Its view stays, held, until it is made again.
    fn unmap(&self, made: &mut Made, title: u32) {
        let (Some(map), Some(m)) = (made.map.as_mut(), &self.map) else {
            return;
        };
        let Some(host) = map.host(title).filter(|_| title >= map.window()) else {
            return;
        };
        let i = title - map.window();
        // SAFETY: entry `i` of the mapped heap map (`title` is mapped).
        unsafe { (m.at as *mut u32).add(i as usize).write_volatile(0) };
        // The entry is out before the submissions running are counted.
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        map.free(title, crate::host::submissions().0);
        made.keys[host as usize] = 0;
    }

    /// Title slot `title` holds no view (`made` locked): its host slot reads
    /// nothing and leaves the sets of the slots shaders pick by index (as
    /// the fork did), and a pool slot is given back. `for_draw`: the slot
    /// leaves the sets even if its host slot was never made.
    fn empty_slot(&self, made: &mut Made, title: u32, for_draw: bool) {
        match made.host(title) {
            Some(host) if made.keys[host as usize] != 0 => {
                made.keys[host as usize] = 0;
                self.make_null(host);
                let _ = self.note_buffer(title, None, false);
            }
            // Left for later and stale since: out of the sets and the stride table.
            _ if for_draw => {
                let _ = self.note_buffer(title, None, false);
            }
            _ => {}
        }
        self.unmap(made, title);
    }

    /// With the heap map's pool used up (`made` locked): gives back the pool
    /// slots of title slots that hold no live view any more (a view of a
    /// released resource, or no view), which nothing else finds: the fork
    /// learns of a slot's view only from the title's descriptor writes and
    /// copies, and a title that releases a resource need not write its
    /// slots again. At most once every two seconds; the slots given back
    /// are handed out once the submissions running now are done. Returns
    /// how many.
    fn reclaim(&self, made: &mut Made) -> usize {
        use std::sync::atomic::Ordering::Relaxed;
        const EVERY_MS: u64 = 2000;
        let now = crate::host::now_ms();
        let last = self.reclaimed.load(Relaxed);
        if last != u64::MAX && now < last.saturating_add(EVERY_MS) {
            return 0;
        }
        self.reclaimed.store(now, Relaxed);
        let start = self.title_start.load(Relaxed);
        let Some(map) = made.map.as_ref().filter(|_| start != 0) else {
            return 0;
        };
        let started = std::time::Instant::now();
        let dead: Vec<u32> = map
            .mapped()
            .map(|(title, _)| title)
            .filter(|&title| {
                let handle = start + u64::from(title) * TITLE_SLOT_BYTES;
                // SAFETY: `title < title_count`, a slot of the live title heap.
                let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
                tag != VIEW_TAG || view_at(i, generation).is_none()
            })
            .collect();
        for &title in &dead {
            self.empty_slot(made, title, true);
        }
        // The first passes at warn level, the rest at debug.
        static PASSES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let (given_back, ms) = (dead.len(), started.elapsed().as_millis());
        if PASSES.fetch_add(1, Relaxed) < 16 {
            tracing::warn!(given_back, ms, "heap map pool used up: pool slots of title slots without a live view given back");
        } else {
            tracing::debug!(given_back, ms, "heap map pool used up: pool slots of title slots without a live view given back");
        }
        dead.len()
    }

    /// The GPU address of the table of element strides by title slot (view
    /// heaps).
    pub(crate) fn strides_gpu(&self) -> Option<u64> {
        self.strides.as_ref().map(|s| s.1)
    }

    /// The table's buffer, for a list to hold.
    pub(crate) fn strides_buffer(&self) -> Option<&ID3D12Resource> {
        self.strides.as_ref().map(|s| &s.0)
    }

    /// The GPU address and buffer of the table of console descriptor
    /// dwords by title slot (view heaps).
    pub(crate) fn words_table(&self) -> Option<(u64, &ID3D12Resource)> {
        self.words.as_ref().map(|s| (s.1, &s.0))
    }

    /// Up to `max` title slots whose texture view is due at `frame`, each
    /// with its write number, in the order to serve them
    /// ([`TextureQueue::due`]), and the queue's counts
    /// ([`TextureQueue::census`]).
    pub(crate) fn texture_slots_due(&self, frame: u64, max: usize) -> (Vec<(u32, u64)>, [u64; 5]) {
        let textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        (if max == 0 { Vec::new() } else { textures.due(frame, max) }, textures.census(frame))
    }

    /// The texture view of title slot `index` (as handed out with `write`)
    /// was brought up to date: due again from frame `again` (uploaded with
    /// that many bytes of its memory there, not all), or not until the
    /// slot is written again.
    pub(crate) fn texture_slot_done(&self, index: u32, write: u64, again: Option<(u64, u64)>) {
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        match again {
            Some((frame, mapped)) => textures.partly(index, write, frame, mapped),
            None => textures.done(index, write),
        }
    }

    /// Title slot `index` was uploaded whole: see [`TextureQueue::recheck`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn texture_slot_recheck(
        &self,
        index: u32,
        write: u64,
        due: u64,
        at: (u64, u64),
        print: u64,
        looks: u8,
        uploaded: u64,
    ) {
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        textures.recheck(index, write, due, at, print, looks, uploaded);
    }

    /// See [`TextureQueue::checked`].
    pub(crate) fn texture_slot_checked(&self, index: u32) -> Option<(u64, u64, u64, u8, u64)> {
        self.textures.lock().unwrap_or_else(|e| e.into_inner()).checked(index)
    }

    /// Nothing to upload at title slot `index` now (its memory is not
    /// mapped, or no more of it than before): looked at again after 1, 2,
    /// 4, ... frames, at most `most`.
    pub(crate) fn texture_slot_later(&self, index: u32, write: u64, frame: u64, most: u64) {
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        textures.later(index, write, frame, most);
    }

    /// Bytes of title slot `index`'s memory that were mapped when it was
    /// last uploaded in part (0: not uploaded, or whole).
    pub(crate) fn texture_slot_mapped(&self, index: u32) -> u64 {
        self.textures.lock().unwrap_or_else(|e| e.into_inner()).mapped(index)
    }

    /// Takes the ranges of title memory the runtime mapped, unmapped or
    /// wrote since the last call ([`remapped`]) and makes the slots whose
    /// placed texture lies there due from the next frame (by when a write
    /// under way has landed): memory that arrived is uploaded, memory that
    /// went makes the slot null ([`HostHeap::texture_slot_gone`]), and a
    /// texture streamed into the place of another of the same description
    /// is uploaded although no descriptor says so. Slots tables read are
    /// left to their draws. Returns how many slots it made due.
    /// shortcut: the ranges are kept in one place for all heaps; the heap
    /// whose pass runs first takes them (a title has one view heap).
    pub(crate) fn texture_slots_remapped(&self, frame: u64) -> usize {
        use std::sync::atomic::Ordering::Relaxed;
        let ranges = std::mem::take(&mut *REMAPPED.lock().unwrap_or_else(|e| e.into_inner()));
        if ranges.is_empty() {
            return 0;
        }
        let mut slots: Vec<u32> = {
            let placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            ranges.iter().flat_map(|&(start, end)| placed.slots_in(start, end)).collect()
        };
        slots.sort_unstable();
        slots.dedup();
        // A table's slot waits for its draw, as a write of it does: the
        // draw takes it out again. Left out altogether, a slot that shaders
        // also pick by index kept what it showed before.
        let lately: Vec<bool> = slots.iter().map(|&slot| self.tabled_lately(slot, frame)).collect();
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        for (&slot, lately) in slots.iter().zip(lately) {
            textures.changed(slot, frame, frame + if lately { TABLED_GRACE } else { 1 });
        }
        STALE[3].fetch_add(slots.len() as u64, Relaxed);
        slots.len()
    }

    /// Goes on with a look at all of the heap's placed texture views, a
    /// part at a time: a view whose memory is there, whose host slot shows
    /// nothing (or another view) and which is not in the queue will stay
    /// that way until the title writes it again, and a shader that picks it
    /// reads black. One whose memory no newer view describes otherwise goes
    /// back into the queue; the others are counted ([`SWEPT`]).
    /// `VICTORY_TEXTURE_SWEEP=0` turns it off, `count` only counts.
    pub(crate) fn texture_sweep(&self, heap: &super::Heap, frame: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        /// Slots looked at at a time, and the least time between.
        const SLOTS: u32 = 1 << 15;
        const EVERY_MS: u64 = 100;
        static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
        let mode = *MODE.get_or_init(|| match std::env::var("VICTORY_TEXTURE_SWEEP").as_deref() {
            Ok("0") => 0,
            Ok("count") => 1,
            _ => 2,
        });
        if mode == 0 || self.sampler {
            return;
        }
        let now = crate::host::now_ms();
        let Ok(mut sweep) = SWEEP.try_lock() else {
            return;
        };
        if now.saturating_sub(sweep.2) < EVERY_MS {
            return;
        }
        sweep.2 = now;
        let (from, mut counts) = (sweep.0.min(self.count), sweep.1);
        let to = from.saturating_add(SLOTS).min(self.count);
        self.sweep_part(from..to, |index| heap.handle(index), frame, mode == 2, &mut counts);
        sweep.0 = to;
        sweep.1 = counts;
        if to >= self.count {
            for (to, counted) in SWEPT.iter().zip(counts) {
                to.store(counted, Relaxed);
            }
            (sweep.0, sweep.1) = (0, [0; 10]);
        }
    }

    /// [`HostHeap::texture_sweep`] of some slots, `handle` saying where the
    /// title's descriptors lie.
    fn sweep_part(
        &self,
        slots: std::ops::Range<u32>,
        handle: impl Fn(u32) -> Option<u64>,
        frame: u64,
        put_back: bool,
        counts: &mut [u64; 10],
    ) {
        use std::sync::atomic::Ordering::Relaxed;
        /// Put back per part, and only while the queue is shorter than
        /// this: a view the title forgot to clear may be among them, and
        /// the queue serves the newest first.
        const MOST: u32 = 128;
        const SHORT: usize = 4_000;
        let mut back = 0;
        // The slots whose host slot does not show what the title's says.
        let mut unmade = Vec::new();
        {
            let made = self.made.lock().unwrap_or_else(|e| e.into_inner());
            // The memory is the title's, and a heap may be larger than what
            // of it is there: asked once per page.
            let mut page = (u64::MAX, false);
            for index in slots {
                let Some(handle) = handle(index) else {
                    break;
                };
                if page.0 != handle >> 12 {
                    page = (handle >> 12, crate::texture::committed(handle as *const u8, 12));
                }
                if !page.1 {
                    continue;
                }
                // SAFETY: a descriptor slot of the title's heap, there.
                let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
                if tag != VIEW_TAG || generation >> super::KIND_SHIFT == super::CONSTANT {
                    continue;
                }
                let key = u64::from(generation) << 32 | u64::from(i);
                // (the record of the slot's host slot: its own, or its pool slot past a
                // heap map's window)
                let shown = made.host(index).and_then(|h| made.keys.get(h as usize)).copied().unwrap_or(0);
                if shown == key {
                    counts[0] += 1;
                    counts[1] += 1;
                } else {
                    unmade.push((index, i, generation, shown != 0));
                }
            }
        }
        for (index, i, generation, other) in unmade {
            let Some(View::Placed(false, resource, desc)) = view_at(i, generation) else {
                continue;
            };
            if resource[0] == 1 {
                continue;
            }
            counts[0] += 1;
            let queued = {
                let textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
                textures.slots.contains_key(&index)
            };
            if queued {
                counts[2] += 1;
                continue;
            }
            if self.tabled_lately(index, frame) {
                counts[3] += 1;
                continue;
            }
            let start = u64::from(desc[10]) | u64::from(desc[11]) << 32;
            if !crate::texture::committed(start as *const u8, 1) {
                counts[4] += 1;
                continue;
            }
            let (claim, against) = {
                let mut placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
                let Some(bytes) = placed.bytes(&resource, crate::resource::first_mip(&desc)) else {
                    continue;
                };
                placed.claim(index, start, start.saturating_add(bytes), &resource)
            };
            counts[5 + claim as usize] += 1;
            counts[9] += u64::from(other);
            static SAID: [std::sync::atomic::AtomicU32; 4] = [const { std::sync::atomic::AtomicU32::new(0) }; 4];
            if SAID[claim as usize].fetch_add(1, Relaxed) < 12 {
                let (width, height, format) = (resource[4], resource[6], resource[8]);
                tracing::info!(
                    "a texture view nothing brings up to date: slot {index}, {claim:?}, {width}x{height} f{format} \
                     mips {} at {start:#x}, shows {}; against {against:x?}; its view {:x?}",
                    resource[7] >> 16,
                    if other { "another view" } else { "nothing" },
                    &desc[..10]
                );
            }
            if put_back && back < MOST && matches!(claim, Claim::Own | Claim::Free) {
                let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
                if textures.slots.len() < SHORT {
                    textures.written(index, frame, frame + 1, false);
                    back += 1;
                    SWEPT[10].fetch_add(1, Relaxed);
                }
            }
        }
    }

    /// The memory of the texture title slot `index` views is not mapped
    /// (any more): a host slot made from it becomes null again, so that
    /// the host copy can be let go (`crate::resource::let_go_idle`; a slot
    /// keeps what it views). True if the slot was made.
    /// (With a heap map, a title slot past the window keeps its pool slot:
    /// it still holds the view, made again once its memory is mapped.)
    pub(crate) fn texture_slot_gone(&self, index: u32) -> bool {
        let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
        if made
            .host(index)
            .and_then(|host| made.keys.get(host as usize))
            .is_none_or(|&key| key == 0)
        {
            return false;
        }
        self.unmake(&mut made, index);
        STALE[4].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        true
    }

    /// Makes title slot `index`'s host slot null again, as it is before its
    /// view is made: a null view of the kind it held.
    fn unmake(&self, made: &mut Made, index: u32) {
        let Some(host) = made.host(index) else {
            return;
        };
        let Some(key) = made.keys.get_mut(host as usize).map(std::mem::take) else {
            return;
        };
        if key == 0 {
            return;
        }
        match view_at(key as u32, (key >> 32) as u32) {
            Some(view) => null_for(&view, self.cpu(host)),
            None => null_view(self.cpu(host)),
        }
        self.keep(host, None);
    }

    /// Title slot `index` is a table's ([`HostHeap::tabled`]). `read`: a
    /// draw read it through the table and brought what it views up to
    /// date, so the slot leaves the queue of textures picked by index; the
    /// caller makes the host slot in place ([`HostHeap::mirror`]), as the
    /// pass would have, for a shader that picks it by index all the same.
    pub(crate) fn texture_slot_tabled(&self, index: u32, read: bool) {
        self.tabled_in(index, read, crate::frame::current());
    }

    /// [`HostHeap::texture_slot_tabled`] in frame `frame`.
    fn tabled_in(&self, index: u32, read: bool, frame: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        let Some(stamp) = self.tabled.get(index as usize) else {
            return;
        };
        let now = (frame as u32).wrapping_add(1) & !TABLED_READ;
        let old = stamp.load(Relaxed);
        if !read {
            if old & !TABLED_READ != now {
                stamp.store(now, Relaxed);
            }
            return;
        }
        // Once per slot and frame: tables are read by every draw.
        if old != now | TABLED_READ {
            stamp.store(now | TABLED_READ, Relaxed);
            if tabled_out() {
                let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
                if textures.slots.remove(&index).is_some() {
                    TABLED_WRITES[1].fetch_add(1, Relaxed);
                }
            }
        }
    }

    /// Whether title slot `index` was a table's within the last
    /// [`TABLED_FRAMES`] frames before `frame` (never, if such slots are
    /// to be queued as any other).
    fn tabled_lately(&self, index: u32, frame: u64) -> bool {
        let stamp = self.tabled_stamp(index) & !TABLED_READ;
        let now = (frame as u32).wrapping_add(1) & !TABLED_READ;
        tabled_out() && stamp != 0 && u64::from(now.wrapping_sub(stamp)) <= TABLED_FRAMES
    }

    /// What `tabled` holds for title slot `index`; 0: never a table's.
    fn tabled_stamp(&self, index: u32) -> u32 {
        self.tabled
            .get(index as usize)
            .map_or(0, |s| s.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The queue's part of a write of title slot `index` in `frame`:
    /// `texture`, whether it now holds a view of a texture the title may
    /// fill; `seen` as for [`HostHeap::note_buffer`].
    fn queue_texture(&self, index: u32, texture: bool, seen: bool, frame: u64) {
        let stamp = if tabled_out() { self.tabled_stamp(index) } else { 0 };
        let now = (frame as u32).wrapping_add(1) & !TABLED_READ;
        // A table's slot lately: the draw that reads it uploads its
        // texture, so its write waits for that draw.
        let lately = stamp != 0 && u64::from(now.wrapping_sub(stamp & !TABLED_READ)) <= TABLED_FRAMES;
        // Made now by a draw of this frame that read it through a table:
        // up to date.
        let read_now = seen && stamp == now | TABLED_READ;
        let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
        if texture && !read_now {
            let due = if lately && !seen {
                TABLED_WRITES[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                frame + TABLED_GRACE
            } else {
                0
            };
            textures.written(index, frame, due, seen);
        } else {
            textures.remove(index);
        }
    }

    /// The title slots that hold buffer views.
    pub(crate) fn buffer_slots(&self) -> Vec<u32> {
        let buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        buffers.iter().copied().collect()
    }

    /// The title slots whose buffer view was written, or went, since the
    /// last call.
    pub(crate) fn buffer_changes(&self) -> Vec<u32> {
        let mut changed = self.changed.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *changed).into_iter().collect()
    }

    /// Records what title slot `index` now holds for shaders that pick
    /// buffers from the heap: its element stride in the table, and whether
    /// it is a buffer view at all.
    /// `seen`: the slot was not written now, a draw or a pass made its
    /// host slot from what it held.
    /// Returns the title slots whose placed texture view the write made
    /// stale ([`Spans`]): out of the queue of textures already, their host
    /// slots are the caller's to make null (it holds `made`).
    fn note_buffer(&self, index: u32, view: Option<&View>, seen: bool) -> Vec<u32> {
        if self.sampler {
            return Vec::new();
        }
        // Measured for the texture side of the same gap (textures a shader
        // picks by a computed index are neither made, if placed, nor
        // uploaded): slots written with a placed texture view, and with a
        // view of a texture object.
        let texture = match view {
            Some(View::Placed(false, resource, _)) if resource[0] != 1 => {
                PLACED_TEXTURE_SLOTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                true
            }
            Some(View::Shader(r, _)) if crate::resource::buffer_bytes(*r as crate::object::This).is_none() => {
                OBJECT_TEXTURE_SLOTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                true
            }
            _ => false,
        };
        self.queue_texture(index, texture, seen, crate::frame::current());
        // The memory a placed texture view describes. Not for a slot
        // tables read (thousands of writes a frame, each read by its
        // draw): no pass looks at it, and the same texture's view in a
        // slot shaders pick by index says what it says.
        let placed_view = match view {
            Some(View::Placed(false, resource, desc))
                if resource[0] != 1 && !self.tabled_lately(index, crate::frame::current()) =>
            {
                let first = crate::resource::first_mip(desc);
                Some((resource, u64::from(desc[10]) | u64::from(desc[11]) << 32, first))
            }
            _ => None,
        };
        let stale = {
            use std::sync::atomic::Ordering::Relaxed;
            let mut placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            let span = placed_view
                .and_then(|(resource, start, first)| Some((resource, start, start.saturating_add(placed.bytes(resource, first)?))));
            match span {
                Some((resource, start, end)) => {
                    SPANNED[0].fetch_min(start, Relaxed);
                    SPANNED[1].fetch_max(end, Relaxed);
                    placed.note(index, start, end, resource, !seen && stale_out())
                }
                None => {
                    placed.forget(index);
                    Vec::new()
                }
            }
        };
        if !stale.is_empty() {
            STALE[0].fetch_add(stale.len() as u64, std::sync::atomic::Ordering::Relaxed);
            let mut textures = self.textures.lock().unwrap_or_else(|e| e.into_inner());
            for &slot in &stale {
                textures.remove(slot);
            }
        }
        if let Some((_, _, data)) = &self.words
            && index < victory_shader::emit::SLOT_STRIDES_COUNT
        {
            let at = (index * WORDS) as usize;
            for (k, w) in console_words(&view_shape(view)).into_iter().enumerate() {
                // SAFETY: inside the mapping; shaders read it while we write
                // (whole dwords).
                unsafe { (*data as *mut u32).add(at + k).write_volatile(w) };
            }
        }
        let stride = view.and_then(view_stride);
        if let Some((_, _, data)) = &self.strides
            && index < victory_shader::emit::SLOT_STRIDES_COUNT
        {
            // SAFETY: inside the mapping; shaders read it while we write (a
            // dword, either value is one the slot had).
            unsafe { (*data as *mut u32).add(index as usize).write_volatile(stride.unwrap_or(0)) };
            // The title address of the buffer view, after the strides
            // (`victory_shader::emit::SLOT_ADDRESSES`): low dword, then high.
            let address = view
                .and_then(buffer_range)
                .map_or(0, |range| range.0);
            // SAFETY: as above, two dwords inside the mapping.
            unsafe {
                let at = (*data as *mut u32).add((victory_shader::emit::SLOT_ADDRESSES + 2 * index) as usize);
                at.write_volatile(address as u32);
                at.add(1).write_volatile((address >> 32) as u32);
            }
        }
        let mut buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        let was = if stride.is_some() {
            buffers.insert(index);
            true
        } else {
            buffers.remove(&index)
        };
        if was {
            self.changed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(index);
        }
        stale
    }

    /// Gives back `n` dynamic slots from `first` (an index [`dynamic`]
    /// returned).
    ///
    /// [`dynamic`]: HostHeap::dynamic
    pub(crate) fn release(&self, first: u32, n: u32) {
        let Some(at) = first.checked_sub(self.dynamic_first) else {
            return;
        };
        let mut guard = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let end = (at + n).min(self.dynamic) as usize;
        if let Some(slots) = guard.1.get_mut(at as usize..end) {
            let given = slots.iter().filter(|taken| **taken).count() as u32;
            self.free.fetch_add(given, std::sync::atomic::Ordering::Relaxed);
            self.too_long.store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
            slots.fill(false);
        }
    }

    /// Makes slot `index` (a dynamic one) from `view`; a null view if the
    /// host cannot.
    pub(crate) fn make_at(&self, index: u32, view: &View) {
        let kept = make(view, self.cpu(index));
        if kept.is_none() {
            null_for(view, self.cpu(index));
        }
        self.keep(index, kept.flatten());
    }

    /// Makes slot `index` read nothing: a null view, or a plain sampler.
    pub(crate) fn make_null(&self, index: u32) {
        if self.sampler {
            self.null_sampler(index);
        } else {
            null_view(self.cpu(index));
        }
        self.keep(index, None);
    }

    /// Slot `index` now views `resource` (or nothing of the host's).
    fn keep(&self, index: u32, resource: Option<ID3D12Resource>) {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = kept.get_mut(index as usize) {
            // A running submission may still read the old resource through
            // this slot.
            if let Some(old) = std::mem::replace(slot, resource) {
                crate::host::retire(old);
            }
        }
    }

    /// Makes slot `index` (a dynamic one) a raw buffer view of `len` bytes
    /// of `buffer` from byte `offset` (a multiple of 16): a root SRV with a
    /// size, whose reads past it return zeros instead of reaching whatever
    /// the host placed after the buffer.
    pub(crate) fn raw_at(&self, index: u32, buffer: &ID3D12Resource, offset: u64, len: u64) {
        let Some(host) = host() else { return };
        let elements = (len / 4).clamp(1, u64::from(u32::MAX)) as u32;
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_SRV {
                    FirstElement: offset / 4,
                    NumElements: elements,
                    StructureByteStride: 0,
                    Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                },
            },
        };
        // SAFETY: a slot of our heap and a live buffer covering the range.
        unsafe {
            host.device
                .CreateShaderResourceView(buffer, Some(&desc), self.cpu(index))
        };
        self.keep(index, Some(buffer.clone()));
    }

    /// Makes slot `index` (a dynamic one) a raw buffer view that reads
    /// zeros.
    pub(crate) fn null_raw_at(&self, index: u32) {
        null_buffer_view(self.cpu(index));
        self.keep(index, None);
    }

    /// Makes slot `index` (a dynamic one) a raw unordered access view of
    /// `len` bytes of `buffer` from byte `offset` (a multiple of 16): a
    /// root UAV with a size, whose accesses past it do nothing instead of
    /// reaching whatever the host placed after the buffer.
    pub(crate) fn raw_uav_at(&self, index: u32, buffer: &ID3D12Resource, offset: u64, len: u64) {
        let Some(host) = host() else { return };
        let elements = (len / 4).clamp(1, u64::from(u32::MAX)) as u32;
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: offset / 4,
                    NumElements: elements,
                    StructureByteStride: 0,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                },
            },
        };
        // SAFETY: a slot of our heap and a live buffer covering the range.
        unsafe {
            host.device.CreateUnorderedAccessView(
                buffer,
                None::<&ID3D12Resource>,
                Some(&desc),
                self.cpu(index),
            )
        };
        self.keep(index, Some(buffer.clone()));
    }

    /// Makes slot `index` (a dynamic one) a raw unordered access view of
    /// nothing: reads give zeros, writes are dropped.
    pub(crate) fn null_raw_uav_at(&self, index: u32) {
        self.keep(index, None);
        let Some(host) = host() else { return };
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    NumElements: 1,
                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                    ..Default::default()
                },
            },
        };
        // SAFETY: a slot of our heap; a null resource is allowed with a desc.
        unsafe {
            host.device.CreateUnorderedAccessView(
                None::<&ID3D12Resource>,
                None::<&ID3D12Resource>,
                Some(&desc),
                self.cpu(index),
            )
        };
    }

    /// The shader-visible handle of slot `index`.
    pub(crate) fn gpu_handle(&self, index: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        // SAFETY: a live shader-visible heap.
        let start = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart().ptr };
        D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: start + u64::from(index) * u64::from(self.step),
        }
    }

    /// Makes slot `index` (a dynamic one) a constant buffer view of host
    /// memory (the list's own, which it holds).
    pub(crate) fn constant_at(&self, index: u32, gpu: u64, size: u32) {
        self.keep(index, None);
        let Some(host) = host() else { return };
        let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
            BufferLocation: gpu,
            SizeInBytes: constant_size(size),
        };
        // SAFETY: a slot of our heap; the memory is the caller's.
        unsafe {
            host.device
                .CreateConstantBufferView(Some(&desc), self.cpu(index))
        };
    }

    fn cpu(&self, index: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: self.cpu + index as usize * self.step as usize,
        }
    }

    /// Makes title slot `index`'s host slot (its own, or with a heap map
    /// past the identity window its pool slot) from the record the title
    /// has at `handle` if it changed since, and returns the view recorded
    /// there. Buffer views are made only for draws (`for_draw`), which also
    /// copy their data: a host copy of a buffer is made on first use.
    pub(crate) fn mirror(&self, index: u32, handle: u64, for_draw: bool) -> Option<View> {
        let mirrored = if self.map.is_some() { self.title_count } else { self.count };
        if index >= mirrored {
            // Diagnostic (NVIDIA heap cap): the highest title slot past the
            // host heap seen so far.
            static HIGHEST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            // (logged at the first one and then at each doubling of how far past: a title
            // that fills its whole heap at start walks hundreds of thousands of these, and a
            // line for each took minutes before its second frame)
            let past = index - self.count + 1;
            if HIGHEST.fetch_max(index, std::sync::atomic::Ordering::Relaxed) < index && past.is_power_of_two() {
                tracing::warn!(
                    index,
                    count = self.count,
                    for_draw,
                    "title slot past the host heap"
                );
            }
            return None;
        }
        if self.map.is_some() && index >= self.count {
            // Where the title's descriptors start, for `reclaim`.
            let start = handle.wrapping_sub(u64::from(index) * TITLE_SLOT_BYTES);
            let _ = self.title_start.compare_exchange(
                0,
                start,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        // SAFETY: the handle is a descriptor slot in title memory.
        let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
        // Generations start at 1, so a made slot's key is never 0.
        let key = u64::from(generation) << 32 | u64::from(i);
        let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
        let view = (tag == VIEW_TAG).then(|| view_at(i, generation)).flatten();
        let Some(view) = view else {
            // Not a view of ours (never written, or of a resource the
            // title released): nothing to read.
            self.empty_slot(&mut made, index, for_draw);
            return None;
        };
        let Some((host, taken)) = self.host_slot(&mut made, index) else {
            // Past the window with the pool used up: the slot reads the
            // guard. What the title wrote is noted all the same (the sets
            // of slots shaders pick by index retry it).
            if !for_draw {
                for stale in self.note_buffer(index, Some(&view), false) {
                    if stale != index {
                        self.unmake(&mut made, stale);
                    }
                }
            }
            return Some(view);
        };
        if taken || made.keys[host as usize] != key {
            // Views of other textures in the memory this one describes
            // are stale: their host slots read nothing from here on, and
            // let go of the host copies they held.
            for stale in self.note_buffer(index, Some(&view), for_draw) {
                if stale != index {
                    self.unmake(&mut made, stale);
                }
            }
            // A view of a texture object that has no host copy yet waits
            // for it, as buffer views wait for a draw: the slot is made
            // when the texture is first used (`super::make_waiting`), or
            // here when a draw reads the slot through a table.
            let waiting = if for_draw { None } else { view.waiting_texture() };
            if let Some(texture) = waiting {
                super::wait_for(texture, handle);
            }
            let later = !for_draw && (view.is_buffer() || waiting.is_some());
            // Another view of the placed texture the slot shows already (a
            // title that streams a texture in writes its view again with
            // more mips each time some arrive): the slot goes on showing
            // the old view until a draw or the texture pass makes the new
            // one and uploads what it shows. Made null here, the texture
            // read as nothing for as long as it waited in the queue, every
            // time it got sharper.
            let old = made.keys[host as usize];
            if later
                && keep_views()
                && view_at(old as u32, (old >> 32) as u32)
                    .is_some_and(|old| same_placed_texture(&old, &view))
            {
                KEPT_VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Some(view);
            }
            made.keys[host as usize] = if later { 0 } else { key };
            let kept = if later {
                None
            } else {
                make(&view, self.cpu(host))
            };
            if kept.is_none() && !self.sampler {
                null_for(&view, self.cpu(host));
            }
            self.keep(host, kept.flatten());
            if taken {
                self.publish(&made, index);
                #[cfg(feature = "diagnostics")]
                note_pool(&made);
            }
        }
        Some(view)
    }

    /// Whether title slot `index`'s host slot shows `buffer`; `None` if it
    /// shows no resource (diagnostics: a buffer view's slot must show the
    /// host copy that is brought up to date for it).
    #[cfg(feature = "diagnostics")]
    pub(crate) fn shows(&self, index: u32, buffer: &ID3D12Resource) -> Option<bool> {
        let host = self.made.lock().unwrap_or_else(|e| e.into_inner()).host(index)?;
        let kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        let shown = kept.get(host as usize)?.as_ref()?;
        Some(windows::core::Interface::as_raw(shown) == windows::core::Interface::as_raw(buffer))
    }

    /// The view host slot `index` was made from, in a few words, with the
    /// title and host addresses of a buffer view (diagnostics). "null" for
    /// a slot never made or left null.
    #[cfg(feature = "diagnostics")]
    pub(crate) fn describe_slot(&self, index: u32) -> Option<String> {
        let key = *self.made.lock().unwrap_or_else(|e| e.into_inner()).keys.get(index as usize)?;
        if key == 0 {
            return Some("null".into());
        }
        let view = view_at(key as u32, (key >> 32) as u32)?;
        let mut text: String = format!("{view:?}").chars().take(120).collect();
        if let Some((address, len)) = buffer_range(&view) {
            let host = crate::shadow::find(address, len.max(1)).map(|s| s.gpu(address));
            text.push_str(&format!(" title {address:#x}+{len:#x} host {host:#x?}"));
        }
        Some(text)
    }
}

/// Logs `what` the first time it happens.
macro_rules! once {
    ($($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!($($arg)*));
    }};
}

/// A view description from its words, with the format as the host has it.
fn srv_desc(words: &[u32; 10]) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    let mut words = *words;
    words[0] = crate::resource::pc_format(words[0]);
    // SAFETY: 40 bytes in the host's layout.
    unsafe {
        std::ptr::from_ref(&words)
            .cast::<D3D12_SHADER_RESOURCE_VIEW_DESC>()
            .read_unaligned()
    }
}

fn uav_desc(words: &[u32; 10]) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    let mut words = *words;
    words[0] = crate::resource::pc_format(words[0]);
    // SAFETY: 40 bytes in the host's layout.
    unsafe {
        std::ptr::from_ref(&words)
            .cast::<D3D12_UNORDERED_ACCESS_VIEW_DESC>()
            .read_unaligned()
    }
}

/// A buffer view of title memory at `address` on its host copy: the first
/// element moves by the copy's offset.
/// Bytes per element of a buffer view.
fn element_size(desc: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> u64 {
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    if buffer.StructureByteStride > 0 {
        u64::from(buffer.StructureByteStride)
    } else if buffer.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0 {
        4
    } else {
        crate::resource::element(desc.Format.0 as u32).map_or(0, |(bytes, _)| bytes)
    }
}

/// The title memory a buffer shader or unordered access view reads: its
/// address and length.
pub(crate) fn buffer_range(view: &View) -> Option<(u64, u64)> {
    let (base, words): (u64, &[u32; 10]) = match view {
        View::Shader(r, Some(desc)) => (
            crate::resource::buffer_bytes(*r as crate::object::This)?.0 as u64,
            desc,
        ),
        View::Placed(false, resource, desc) if resource[0] == 1 => (
            u64::from(desc[10]) | u64::from(desc[11]) << 32,
            desc[..10].try_into().ok()?,
        ),
        View::Unordered(r, _, Some(desc)) => {
            let base = crate::resource::buffer_bytes(*r as crate::object::This)?.0 as u64;
            return unordered_range(base, desc);
        }
        View::Placed(true, resource, desc) if resource[0] == 1 => {
            let base = u64::from(desc[10]) | u64::from(desc[11]) << 32;
            return unordered_range(base, desc[..10].try_into().ok()?);
        }
        _ => return None,
    };
    let desc = srv_desc(words);
    let element = element_size(&desc);
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    Some((
        base + buffer.FirstElement * element,
        u64::from(buffer.NumElements) * element,
    ))
}

/// The title memory the host slot of buffer view `view` is made on
/// (address, bytes): the whole buffer the view is of, which is what
/// [`make`] finds the host copy by. [`buffer_range`] is the part of it the
/// view reads.
pub(crate) fn buffer_extent(view: &View) -> Option<(u64, u64)> {
    match view {
        View::Shader(r, Some(_)) | View::Unordered(r, _, Some(_)) => {
            let (data, len) = crate::resource::buffer_bytes(*r as crate::object::This)?;
            Some((data as u64, len))
        }
        View::Placed(_, resource, desc) if resource[0] == 1 => Some((
            u64::from(desc[10]) | u64::from(desc[11]) << 32,
            u64::from(resource[4]) | u64::from(resource[5]) << 32,
        )),
        _ => None,
    }
}

/// Whether a buffer view is brought up to date in the host copy its slot
/// shows ([`buffer_copy`]). On by default; `VICTORY_VIEW_EXTENT=0` looks the
/// copy up by the view's range alone, as before.
fn view_extent() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_VIEW_EXTENT").is_none_or(|v| v != "0"))
}

/// The host copy buffer view `view` reads, and the title range (address,
/// bytes) it reads there; `None` for other views and empty ranges.
///
/// The copy is the one the view's slot shows: [`make`] finds it by the
/// whole buffer the view is of ([`buffer_extent`]). Looked up by the range
/// the view reads, as it was, the copy is another one wherever the two
/// lookups differ: for memory outside buffer objects, a view of a part of a
/// buffer that crosses a window line (the buffer's window is two cells, the
/// part's one), and for buffer objects, a part that also lies in a smaller
/// buffer object. The slot then showed a copy nothing brought up to date,
/// and what shaders wrote through it was looked for in the other copy.
pub(crate) fn buffer_copy(view: &View) -> Option<(std::sync::Arc<crate::shadow::Shadow>, u64, u64)> {
    let (address, len) = buffer_range(view)?;
    if len == 0 {
        return None;
    }
    if view_extent()
        && let Some((base, width)) = buffer_extent(view)
        // (The console is lax: a view may begin past its buffer's end.)
        && address >= base
        && address - base < width.max(1)
        && let Some(shadow) = crate::shadow::find(base, width)
    {
        return Some((shadow, address, len));
    }
    crate::shadow::find(address, len).map(|shadow| (shadow, address, len))
}

/// [`buffer_range`] of an unordered access view of memory at `base`.
fn unordered_range(base: u64, words: &[u32; 10]) -> Option<(u64, u64)> {
    let desc = uav_desc(words);
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    let element = if buffer.StructureByteStride > 0 {
        u64::from(buffer.StructureByteStride)
    } else if buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
        4
    } else {
        crate::resource::element(desc.Format.0 as u32)?.0
    };
    Some((
        base + buffer.FirstElement * element,
        u64::from(buffer.NumElements) * element,
    ))
}

/// The host resource a slot made by [`make`] views, if any.
type Kept = Option<ID3D12Resource>;

/// A structured buffer view (`first`, `count` elements of `stride` bytes,
/// `offset` bytes into its host copy) as a raw view of the same bytes:
/// its first dword and its dwords. Translated shaders read untyped buffers
/// as raw bytes at `element * stride + offset` (the stride from the
/// used-resource entry, or from the table by heap slot for a buffer picked
/// by index), and a raw read through a structured view gives zeros on
/// NVIDIA (reported upstream with PR #193; structured reads through raw
/// views were right there). So the host's view is raw whenever it can be:
/// the bytes must start on 16 (the host's raw view alignment) and be whole
/// dwords. A view that cannot stays structured.
fn raw_window(offset: u64, first: u64, count: u32, stride: u32) -> Option<(u64, u32)> {
    if stride == 0 {
        return None;
    }
    let start = first.checked_mul(u64::from(stride))?.checked_add(offset)?;
    let bytes = u64::from(count) * u64::from(stride);
    (start % 16 == 0 && bytes % 4 == 0)
        .then(|| u32::try_from(bytes / 4).ok().map(|dwords| (start / 4, dwords)))
        .flatten()
}

/// Whether the host accepts a buffer view with these fields of a buffer of
/// `width` bytes. It removes the device for one it does not (view creation
/// returns nothing), and the console is laxer: the title's heap holds
/// views past their memory's end, of no elements, of typeless formats
/// without the raw flag, with strides the host has no structured buffers
/// for. Since every buffer view of the heap is made for shaders that pick
/// them by index, one such view ended the run (2026-11-24, run 10).
fn buffer_view_ok(format: DXGI_FORMAT, first: u64, count: u32, stride: u32, raw: bool, width: u64) -> bool {
    use crate::resource::family;
    let f = format.0 as u32;
    let element = if raw {
        // raw: R32 typeless, counted in dwords, starting on 16 bytes
        if format != DXGI_FORMAT_R32_TYPELESS || stride != 0 || !first.is_multiple_of(4) {
            return false;
        }
        4
    } else if stride > 0 {
        // structured: no format, a stride the host takes
        if format != DXGI_FORMAT_UNKNOWN || stride > 2048 || !stride.is_multiple_of(4) {
            return false;
        }
        u64::from(stride)
    } else {
        // typed: a format with a size that is not typeless
        let typeless = family(f) == f && family(f + 1) == f;
        match crate::resource::element(f) {
            Some((bytes, 1)) if f != 0 && !typeless && bytes > 0 => bytes,
            _ => return false,
        }
    };
    count > 0
        && first
            .checked_add(u64::from(count))
            .and_then(|end| end.checked_mul(element))
            .is_some_and(|end| end <= width)
}

/// The bytes of a constant buffer view of `size` bytes: whole 256s, and at
/// most the 64 KiB a shader can index. The host removes the device for a
/// larger one, and the title's heap holds old views of any size in slots it
/// does not use.
fn constant_size(size: u32) -> u32 {
    size.min(64 << 10).next_multiple_of(256)
}

/// How many of `count` elements of `element` bytes from `first` on lie in
/// `width` bytes: the console lets a view reach past its buffer's end, the
/// host refuses one. A view with no element inside keeps its count (and is
/// refused).
fn elements_inside(first: u64, count: u32, element: u64, width: u64) -> u32 {
    let room = (width / element.max(1)).saturating_sub(first);
    match u32::try_from(room) {
        Ok(0) | Err(_) => count,
        Ok(room) => count.min(room),
    }
}

/// Diagnostics: a buffer view left null for shaders that pick views by
/// index, and why. Told at each doubling of the distinct views of a kind.
fn left_null(why: &'static str, unordered: bool, address: u64, len: u64, element: u64, first: u64) {
    if !cfg!(feature = "diagnostics") {
        return;
    }
    type Seen = std::collections::BTreeMap<(&'static str, bool, u64), std::collections::HashSet<(u64, u64)>>;
    static SEEN: Mutex<Seen> = Mutex::new(Seen::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let views = seen.entry((why, unordered, element)).or_default();
    if views.insert((address, len)) && views.len().is_power_of_two() {
        tracing::warn!(
            views = views.len(),
            unordered,
            element,
            first,
            len,
            address = format_args!("{address:#x}"),
            "buffer views left null: {why}"
        );
    }
}

/// `VICTORY_F3_VIEW_FIX`: typed buffer views of three 32-bit channels
/// (`R32G32B32_FLOAT/UINT/SINT`, 12-byte elements) made as one-channel
/// views of the same dwords (`R32_*`, three per element). The host's
/// typed view starts on a whole element of its buffer; a title view of
/// float3 data in a pooled buffer often does not (the start is a
/// multiple of 4, not of 12), and was left null: 2,276 distinct views in
/// one run (blendshape and bone tables among them, read as zeros). The
/// one-channel view starts on any dword.
///
/// Every such view takes this shape while on, on an element or not, so
/// that what the translated program reads is decided by the format alone:
/// element `i`, channel `k` is dword `3 * i + k` of the view. That is the
/// translator's side of it (victory-shader `lower.rs` `f3_format`): the
/// view's descriptor words carry the 32_32_32 format ([`view_shape`]),
/// and translated typed loads and stores read elements `3i + k` where they
/// find it. Both follow the one switch, `victory_shader::emit::words::
/// f3_view_fix` (off by default; `VICTORY_F3_VIEW_FIX=1`).
fn f3_view_fix() -> bool {
    victory_shader::emit::words::f3_view_fix()
}

/// The GFX10 buffer format a typed view of `format` gets in its descriptor
/// words: 32_32_32 for the three-channel formats [`f3_view_fix`] makes one
/// channel wide (while on), else 0 (no format; the translation reads one
/// element per index).
fn f3_words_format(format: DXGI_FORMAT) -> u32 {
    let (first, _) = victory_shader::emit::words::FORMAT_32_32_32;
    match format {
        _ if !f3_view_fix() => 0,
        DXGI_FORMAT_R32G32B32_UINT => first,
        DXGI_FORMAT_R32G32B32_SINT => first + 1,
        DXGI_FORMAT_R32G32B32_FLOAT => first + 2,
        _ => 0,
    }
}

/// Views made by [`f3_view_fix`]'s rule and, of those, the ones that did
/// not start on a 12-byte element (left null without it).
pub(crate) static F3_VIEWS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// The one-channel shape of a typed view of three 32-bit channels: its
/// format, first dword and dwords, `offset` bytes into the host copy and
/// `first`, `count` elements on. None for other formats and for a start
/// that is not on a dword.
fn f3_as_dwords(format: DXGI_FORMAT, offset: u64, first: u64, count: u32) -> Option<(DXGI_FORMAT, u64, u32)> {
    let one = match format {
        DXGI_FORMAT_R32G32B32_FLOAT => DXGI_FORMAT_R32_FLOAT,
        DXGI_FORMAT_R32G32B32_UINT => DXGI_FORMAT_R32_UINT,
        DXGI_FORMAT_R32G32B32_SINT => DXGI_FORMAT_R32_SINT,
        _ => return None,
    };
    let start = first.checked_mul(12)?.checked_add(offset)?;
    if !start.is_multiple_of(4) {
        return None;
    }
    Some((one, start / 4, count.checked_mul(3)?))
}

/// Counts a view made by [`f3_view_fix`]'s rule; told at each doubling of
/// the distinct views that were off an element.
fn f3_made(off_element: bool, address: u64, len: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    F3_VIEWS[0].fetch_add(1, Relaxed);
    if !off_element {
        return;
    }
    F3_VIEWS[1].fetch_add(1, Relaxed);
    static SEEN: Mutex<Option<std::collections::HashSet<(u64, u64)>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(Default::default);
    if seen.insert((address, len)) && seen.len().is_power_of_two() {
        tracing::info!(
            distinct = seen.len(),
            made = F3_VIEWS[0].load(Relaxed),
            off_element = F3_VIEWS[1].load(Relaxed),
            "float3 buffer views made as one-channel views (VICTORY_F3_VIEW_FIX; were left null)"
        );
    }
}

fn buffer_srv(
    address: u64,
    len: u64,
    mut desc: D3D12_SHADER_RESOURCE_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) -> Option<Kept> {
    let shadow = crate::shadow::find(address, len)?;
    let offset = shadow.offset(address);
    // A structured view as a raw view of the same bytes (see `raw_window`).
    // SAFETY: a buffer view's union member.
    let structured = unsafe { desc.Anonymous.Buffer };
    let window = (desc.Format == DXGI_FORMAT_UNKNOWN)
        .then(|| raw_window(offset, structured.FirstElement, structured.NumElements, structured.StructureByteStride))
        .flatten();
    if let Some((first, count)) = window {
        desc.Format = DXGI_FORMAT_R32_TYPELESS;
        desc.Anonymous.Buffer = D3D12_BUFFER_SRV {
            FirstElement: first,
            NumElements: count,
            StructureByteStride: 0,
            Flags: D3D12_BUFFER_SRV_FLAG_RAW,
        };
    }
    let element = element_size(&desc);
    let view_format = desc.Format;
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { &mut desc.Anonymous.Buffer };
    let mut unit = if window.is_some() { 4 } else { element };
    let f3 = (window.is_none() && f3_view_fix())
        .then(|| f3_as_dwords(view_format, offset, buffer.FirstElement, buffer.NumElements))
        .flatten();
    if let Some((_, first, count)) = f3 {
        buffer.FirstElement = first;
        buffer.NumElements = count;
        unit = 4;
        f3_made(offset % 12 != 0, address, len);
    } else if window.is_none() {
        if element == 0 || offset % element != 0 {
            left_null("not on an element of their host copy", false, address, len, element, buffer.FirstElement);
            return None;
        }
        buffer.FirstElement += offset / element;
    }
    // SAFETY: a live buffer.
    let width = unsafe { shadow.buffer.GetDesc() }.Width;
    buffer.NumElements = elements_inside(buffer.FirstElement, buffer.NumElements, unit, width);
    let (first, count, stride) = (buffer.FirstElement, buffer.NumElements, buffer.StructureByteStride);
    let raw = buffer.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0;
    if let Some((format, ..)) = f3 {
        desc.Format = format;
    }
    if !buffer_view_ok(desc.Format, first, count, stride, raw, width) {
        REFUSED_VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let why = if raw { "raw, refused by the host" } else { "refused by the host" };
        left_null(why, false, address, len, element, first);
        once!(
            format = desc.Format.0,
            first,
            count,
            stride,
            raw,
            width,
            "buffer view the host would refuse left null"
        );
        return None;
    }
    // The console has no component mapping; one the host does not know is the default's.
    if desc.Shader4ComponentMapping != D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING {
        desc.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    }
    let host = host()?;
    // SAFETY: a live buffer and a slot of our heap.
    unsafe {
        host.device
            .CreateShaderResourceView(&shadow.buffer, Some(&desc), at)
    };
    Some(Some(shadow.buffer.clone()))
}

/// An unordered access view of title buffer memory at `address` (`len`
/// bytes) on its host copy, as [`buffer_srv`]; false if the copy cannot be
/// written by shaders (upload memory).
/// shortcut: what shaders write stays in the host copy; title memory (and
/// so the CPU, and copies run on the CPU) does not see it.
fn buffer_uav(
    address: u64,
    len: u64,
    mut desc: D3D12_UNORDERED_ACCESS_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) -> Option<Kept> {
    let shadow = crate::shadow::find(address, len)?;
    // SAFETY: a live buffer.
    let flags = unsafe { shadow.buffer.GetDesc() }.Flags;
    if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
        once!("unordered access to a buffer whose host copy shaders cannot write");
        return None;
    }
    let view_format = desc.Format;
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { &mut desc.Anonymous.Buffer };
    let element = if buffer.StructureByteStride > 0 {
        u64::from(buffer.StructureByteStride)
    } else if buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
        4
    } else {
        crate::resource::element(desc.Format.0 as u32).map_or(0, |(bytes, _)| bytes)
    };
    let offset = shadow.offset(address);
    // A structured view as a raw view of the same bytes (see `raw_window`).
    let window = (desc.Format == DXGI_FORMAT_UNKNOWN)
        .then(|| raw_window(offset, buffer.FirstElement, buffer.NumElements, buffer.StructureByteStride))
        .flatten();
    let f3 = (window.is_none() && buffer.StructureByteStride == 0 && f3_view_fix())
        .then(|| f3_as_dwords(view_format, offset, buffer.FirstElement, buffer.NumElements))
        .flatten();
    let mut unit = if window.is_some() { 4 } else { element };
    if let Some((first, count)) = window {
        buffer.FirstElement = first;
        buffer.NumElements = count;
        buffer.StructureByteStride = 0;
        buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
    } else if let Some((_, first, count)) = f3 {
        buffer.FirstElement = first;
        buffer.NumElements = count;
        unit = 4;
        f3_made(offset % 12 != 0, address, len);
    } else {
        if element == 0 || offset % element != 0 {
            left_null("not on an element of their host copy", true, address, len, element, buffer.FirstElement);
            return None;
        }
        buffer.FirstElement += offset / element;
    }
    buffer.CounterOffsetInBytes = 0;
    // SAFETY: a live buffer.
    let width = unsafe { shadow.buffer.GetDesc() }.Width;
    buffer.NumElements = elements_inside(buffer.FirstElement, buffer.NumElements, unit, width);
    let (first, count, stride) = (buffer.FirstElement, buffer.NumElements, buffer.StructureByteStride);
    let raw = buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0;
    if let Some((format, ..)) = f3 {
        desc.Format = format;
    }
    if window.is_some() {
        // (Left out at first: every structured unordered access view was then
        // refused by `buffer_view_ok` and left null, runs 15 and 16.)
        desc.Format = DXGI_FORMAT_R32_TYPELESS;
    }
    if !buffer_view_ok(desc.Format, first, count, stride, raw, width) {
        REFUSED_VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let why = if raw { "raw, refused by the host" } else { "refused by the host" };
        left_null(why, true, address, len, element, first);
        once!(
            format = desc.Format.0,
            first,
            count,
            stride,
            raw,
            width,
            "unordered buffer view the host would refuse left null"
        );
        return None;
    }
    let host = host()?;
    // SAFETY: a live buffer and a slot of our heap.
    unsafe {
        host.device
            .CreateUnorderedAccessView(&shadow.buffer, None::<&ID3D12Resource>, Some(&desc), at)
    };
    Some(Some(shadow.buffer.clone()))
}

/// Whether the host can make a shader (or unordered access) view of
/// `texture` in `format` (none: the texture's own). The console allows any
/// view; the host refuses (and removes the device for) views without the
/// resource's permission, of typed depth, and in another format than a
/// typed resource's. A view of a typed texture in another format of its
/// family takes the texture's format.
/// shortcut: such a view (sRGB of UNORM, say) reads without its
/// conversion; cast with castable formats if colors come out wrong.
fn fits(texture: &ID3D12Resource, mut format: Option<&mut DXGI_FORMAT>, unordered: bool) -> bool {
    use crate::resource::{DEPTH_FORMATS, family};
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    let allowed = if unordered {
        d.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0
    } else {
        d.Flags.0 & D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE.0 == 0
    };
    let own = d.Format.0 as u32;
    if !allowed || DEPTH_FORMATS.iter().any(|&(typed, _)| typed == own) {
        if unordered {
            crate::resource::DEPTH_UNORDERED[3].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        once!(
            unordered,
            format = own,
            flags = d.Flags.0,
            "view the host does not allow"
        );
        return false;
    }
    // Every family has more than one format, its typeless one first.
    let typeless = family(own) == own && family(own + 1) == own;
    if let Some(format) = format.as_deref_mut()
        && unordered
        && let Some(&(_, linear)) = crate::resource::SRGB
            .iter()
            .find(|&&(s, _)| s == format.0 as u32)
    {
        format.0 = linear as i32;
    }
    let view = format.as_ref().map_or(0, |f| f.0 as u32);
    if view == 0 {
        return !typeless;
    }
    // A view of a target whose format the host refuses, in the title's
    // format: in the stand-in the host copy has.
    if view != own && crate::resource::target_format(view) == own {
        if let Some(format) = format {
            *format = d.Format;
        }
        return true;
    }
    if family(view) != family(own) {
        once!(view, own, "view format of another family");
        return false;
    }
    if let Some(format) = format
        && !typeless
    {
        *format = d.Format;
    }
    true
}

/// The title's unordered access view of a depth texture, as a view of the
/// texture's colour twin `twin` ([`crate::resource::Twin`]): the same mip
/// and slices, in the colour format of the twin's family for a depth or
/// typeless format (R16_UNORM for 16-bit depth, R32_FLOAT for 32-bit; the
/// bits are the same). Without a description: mip 0 of every slice.
/// The twin of a stencil plane (R8) stands for the views of that plane
/// only, which become R8_UINT views of its one plane; `None` for a view of
/// the depth plane, which the twin does not stand for. The depth plane has
/// a twin of its own, viewed as a depth texture's without stencil: a view
/// in the format of the depth plane of a depth-stencil format (21, 46)
/// takes the twin's colour format too.
fn twin_uav(
    twin: &ID3D12Resource,
    desc: Option<&D3D12_UNORDERED_ACCESS_VIEW_DESC>,
) -> Option<D3D12_UNORDERED_ACCESS_VIEW_DESC> {
    // SAFETY: a live resource.
    let d = unsafe { twin.GetDesc() };
    let mut desc = desc.cloned().unwrap_or_else(|| {
        let mut all = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        if d.DepthOrArraySize > 1 {
            all.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
            all.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV {
                ArraySize: u32::from(d.DepthOrArraySize),
                ..Default::default()
            };
        }
        all
    });
    // A stencil plane's twin (R8 typeless, 60): the title's view is of the
    // stencil plane if it names plane 1 or a stencil view format (22, 47).
    if d.Format.0 == 60 {
        let mut plane = 0;
        match desc.ViewDimension {
            D3D12_UAV_DIMENSION_TEXTURE2D => {
                // SAFETY: the member the dimension names.
                let mut view = unsafe { desc.Anonymous.Texture2D };
                plane = std::mem::take(&mut view.PlaneSlice);
                desc.Anonymous.Texture2D = view;
            }
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => {
                // SAFETY: the member the dimension names.
                let mut view = unsafe { desc.Anonymous.Texture2DArray };
                plane = std::mem::take(&mut view.PlaneSlice);
                desc.Anonymous.Texture2DArray = view;
            }
            _ => {}
        }
        if plane != 1 && !matches!(desc.Format.0, 22 | 47) {
            return None;
        }
        desc.Format.0 = 62;
        return Some(desc);
    }
    // None, typeless and depth formats (R16: 0, 53, 55; R32: 0, 39, 40),
    // and those of a depth-stencil format's depth plane (19 to 21, 44 to
    // 46), for the twin of such a plane.
    desc.Format.0 = match (desc.Format.0, d.Format.0) {
        (0 | 53 | 55 | 19..=21 | 44..=46, 53) => 56,
        (0 | 39 | 40 | 19..=21 | 44..=46, 39) => 41,
        (other, _) => other,
    };
    Some(desc)
}

/// Makes the host descriptor for `view` at `at`: the host resource it
/// views, or `None` if it cannot.
fn make(view: &View, at: D3D12_CPU_DESCRIPTOR_HANDLE) -> Option<Kept> {
    let host = host()?;
    let device = &host.device;
    match view {
        &View::Constant(address, size) => {
            let shadow = crate::shadow::find(address, u64::from(size))?;
            let gpu = shadow.gpu(address);
            if gpu % 256 != 0 {
                once!(
                    address = format_args!("{address:#x}"),
                    "constant buffer not 256-byte aligned on the host"
                );
                return None;
            }
            let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
                BufferLocation: gpu,
                SizeInBytes: constant_size(size),
            };
            // SAFETY: a slot of our heap; the range lies in the host copy.
            unsafe { device.CreateConstantBufferView(Some(&desc), at) };
            Some(Some(shadow.buffer.clone()))
        }
        View::Shader(resource, desc) => {
            let resource = *resource as crate::object::This;
            if let Some(texture) = crate::resource::host_resource(resource) {
                // The title's views of textures with a stencil plane, as it
                // describes them (format, dimension, component mapping, a
                // pad, then first level, levels, plane): each distinct one
                // once. They say how its shaders read the stencil plane
                // (the lighting takes each pixel's material class and its
                // "inside a room" bit from it).
                #[cfg(feature = "diagnostics")]
                {
                    // SAFETY: a live resource.
                    let own = unsafe { texture.GetDesc() }.Format.0;
                    if matches!(own, 19..=22 | 44..=47) {
                        static SEEN: Mutex<Vec<(i32, Option<[u32; 10]>)>> = Mutex::new(Vec::new());
                        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
                        if seen.len() < 64 && !seen.contains(&(own, *desc)) {
                            seen.push((own, *desc));
                            tracing::info!(
                                ?resource,
                                texture_format = own,
                                view = ?desc,
                                "shader view of a depth-stencil texture"
                            );
                        }
                    }
                }
                let mut desc = desc.as_ref().map(srv_desc);
                if !fits(texture, desc.as_mut().map(|d| &mut d.Format), false) {
                    return None;
                }
                // SAFETY: a live texture and a slot of our heap.
                unsafe {
                    device.CreateShaderResourceView(
                        texture,
                        desc.as_ref().map(std::ptr::from_ref),
                        at,
                    )
                };
                Some(Some(texture.clone()))
            } else if let (Some((data, len)), Some(desc)) =
                (crate::resource::buffer_bytes(resource), desc)
            {
                buffer_srv(data as u64, len, srv_desc(desc), at)
            } else {
                None
            }
        }
        View::Unordered(resource, _counter, desc) => {
            let resource = *resource as crate::object::This;
            let Some(texture) = crate::resource::host_resource(resource) else {
                return match (crate::resource::buffer_bytes(resource), desc) {
                    (Some((data, len)), Some(desc)) => {
                        buffer_uav(data as u64, len, uav_desc(desc), at)
                    }
                    _ => None,
                };
            };
            // The first such views of depth textures are logged as the
            // title describes them (format, dimension, then mip, slices and
            // plane): they say which planes shaders write, and as what.
            if crate::resource::depth_and_unordered(resource) {
                static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 512 {
                    tracing::info!(
                        ?resource,
                        view = ?desc,
                        "unordered access view of a depth texture"
                    );
                }
            }
            let mut desc = desc.as_ref().map(uav_desc);
            // A depth texture's unordered access is its colour twin's: the
            // host gives a depth texture none.
            let twin = crate::resource::twin_resource(resource)
                .and_then(|twin| Some((twin, twin_uav(twin, desc.as_ref())?)))
                // A view of the depth plane of a texture with a stencil
                // plane: of that plane's own twin.
                .or_else(|| {
                    let twin = crate::resource::twin_plane_resource(resource)?;
                    Some((twin, twin_uav(twin, desc.as_ref())?))
                });
            let texture = match twin {
                Some((twin, view)) => {
                    desc = Some(view);
                    twin
                }
                None => texture,
            };
            if !fits(texture, desc.as_mut().map(|d| &mut d.Format), true) {
                return None;
            }
            // SAFETY: a live texture and a slot of our heap.
            unsafe {
                device.CreateUnorderedAccessView(
                    texture,
                    None::<&ID3D12Resource>,
                    desc.as_ref().map(std::ptr::from_ref),
                    at,
                )
            };
            Some(Some(texture.clone()))
        }
        View::Sampler(words) => {
            // SAFETY: 13 words in the host's layout.
            let desc = unsafe {
                std::ptr::from_ref(words)
                    .cast::<D3D12_SAMPLER_DESC>()
                    .read_unaligned()
            };
            // SAFETY: a slot of our sampler heap.
            unsafe { device.CreateSampler(&desc, at) };
            Some(None)
        }
        View::ConsoleSampler(words) => {
            let desc = crate::sampler::console(words);
            // SAFETY: a slot of our sampler heap.
            unsafe { device.CreateSampler(&desc, at) };
            Some(None)
        }
        View::Placed(unordered, resource, desc) => {
            if let Some(view) = placed_texture(view) {
                return make(&view, at);
            }
            const BUFFER: u32 = 1;
            if resource[0] != BUFFER {
                // shortcut: unordered access to a texture the title placed
                // itself is not mirrored; give it a resource as shader
                // views of such textures get when a dispatch writes one.
                once!(dimension = resource[0], "placed unordered texture view not mirrored");
                return None;
            }
            // The resource description's width, after the alignment.
            let len = u64::from(resource[4]) | u64::from(resource[5]) << 32;
            let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
            let words: &[u32; 10] = desc[..10].try_into().unwrap_or(&[0; 10]);
            if *unordered {
                buffer_uav(address, len, uav_desc(words), at)
            } else {
                buffer_srv(address, len, srv_desc(words), at)
            }
        }
    }
}

/// Writes of a slot with another view of the placed texture it showed,
/// since the start: the slot kept showing the old view.
pub(crate) static KEPT_VIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether a slot written with another view of the placed texture it
/// shows keeps showing the old one until the new is made;
/// `VICTORY_KEEP_VIEWS=0` makes it null at once, as before.
fn keep_views() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_KEEP_VIEWS").is_none_or(|v| v != "0"))
}

/// Whether both are shader views of one texture the title placed in
/// memory: the same address and the same resource description, whatever
/// part of it each shows.
fn same_placed_texture(a: &View, b: &View) -> bool {
    match (a, b) {
        (View::Placed(false, resource, desc), View::Placed(false, other, other_desc)) => {
            resource[0] != 1 && resource == other && desc[10..12] == other_desc[10..12]
        }
        _ => false,
    }
}

/// Whether the title's unordered access views of textures it placed in
/// memory itself are mirrored (`VICTORY_WRITE_VIEWS=1`).
///
/// Why they were left null: a placed texture had only shader views (its
/// host copy is filled from title memory, which nothing the host writes
/// reaches), and the unordered ones were left out as a shortcut. But the
/// title packs light (the diffuse GI, the sky and probe caches) as R9G9B9E5
/// into textures it places in memory and writes them with compute shaders
/// through a placed R32_UINT unordered view of the same memory, because
/// R9G9B9E5 cannot be an unordered access format; those writes went to a
/// null view and were dropped, so the lighting read zeros.
pub(crate) fn write_views() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WRITE_VIEWS").is_some_and(|v| v == "1"))
}

/// The resource description (the host's 14 words) of the texture a placed
/// unordered view writes: the title's, with unordered access allowed and no
/// depth, and R9G9B9E5 (67) as R32_UINT (41): the same 4-byte texels, which
/// the host can write. The texture the title reads through its own R9G9B9E5
/// view keeps its description: `resource::alias_of` finds this one by
/// address and size and copies its texels there before a draw reads them.
fn written_description(resource: &[u32; 14]) -> [u32; 14] {
    const FORMAT: usize = 8;
    const FLAGS: usize = 12;
    const UNORDERED: u32 = 0x4;
    const DEPTH: u32 = 0x2;
    let mut r = *resource;
    r[FLAGS] = (r[FLAGS] | UNORDERED) & !DEPTH;
    if r[FORMAT] == 67 {
        r[FORMAT] = 41;
    }
    r
}

/// A shader view of a texture the title placed in memory itself, as a view
/// of the resource made for that memory (on first use); with
/// [`write_views`], an unordered view of one likewise, of the resource
/// shaders write ([`written_description`]).
pub(crate) fn placed_texture(view: &View) -> Option<View> {
    const BUFFER: u32 = 1;
    let View::Placed(unordered, resource, desc) = view else {
        return None;
    };
    if resource[0] == BUFFER || (*unordered && !write_views()) {
        return None;
    }
    let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
    if !*unordered {
        let texture = crate::resource::texture_at(address, resource)?;
        return Some(View::Shader(texture as usize, desc[..10].try_into().ok()));
    }
    let texture = crate::resource::texture_at(address, &written_description(resource))?;
    let mut words: [u32; 10] = desc[..10].try_into().ok()?;
    // The view names the format it writes: R9G9B9E5 is R32_UINT here too.
    if words[0] == 67 {
        words[0] = 41;
    }
    once!(
        format = resource[8],
        address,
        "a placed unordered texture view is mirrored (VICTORY_WRITE_VIEWS)"
    );
    Some(View::Unordered(texture as usize, 0, Some(words)))
}

/// With `VICTORY_NULL_MAGENTA=1`, a 2D colour texture view that shows
/// nothing yet shows magenta: on the screen, a texture that is not there is
/// told from one whose contents are black. False if it is to be null.
#[cfg(feature = "diagnostics")]
fn magenta_for(desc: &[u32; 16], at: D3D12_CPU_DESCRIPTOR_HANDLE) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("VICTORY_NULL_MAGENTA").is_some_and(|v| v == "1")) {
        return false;
    }
    // Block-compressed colour (BC1 to BC3, BC7) and 8-bit RGBA and BGRA.
    let colour = matches!(desc[0], 27..=29 | 70..=78 | 87..=91 | 97..=99);
    if desc[1] != D3D12_SRV_DIMENSION_TEXTURE2D.0 as u32 || !colour {
        return false;
    }
    let (Some(host), Some(texture)) = (host(), magenta()) else {
        return false;
    };
    // SAFETY: a slot of our heap and a live texture.
    unsafe { host.device.CreateShaderResourceView(texture, None, at) };
    true
}

/// One magenta texel, written by the processor.
#[cfg(feature = "diagnostics")]
fn magenta() -> Option<&'static ID3D12Resource> {
    static TEXTURE: std::sync::OnceLock<Option<ID3D12Resource>> = std::sync::OnceLock::new();
    TEXTURE
        .get_or_init(|| {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_CUSTOM,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
                MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
                ..Default::default()
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: 1,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: valid descriptions and an out pointer; the texel is
            // written while nothing reads the texture.
            unsafe {
                host()?
                    .device
                    .CreateCommittedResource(
                        &heap,
                        D3D12_HEAP_FLAG_NONE,
                        &desc,
                        D3D12_RESOURCE_STATE_COMMON,
                        None,
                        &mut texture,
                    )
                    .ok()?;
                let texture = texture?;
                texture.Map(0, None, None).ok()?;
                let texel = [255u8, 0, 255, 255];
                texture.WriteToSubresource(0, None, texel.as_ptr().cast(), 4, 4).ok()?;
                texture.Unmap(0, None);
                Some(texture)
            }
        })
        .as_ref()
}

/// A null view of the kind `view` is, for a slot whose view the host
/// cannot make: unordered access (to a buffer or a 2D texture) or a shader
/// view. Writes to it are dropped, reads give zeros.
fn null_for(view: &View, at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let is_buffer = |r: &usize| crate::resource::buffer_bytes(*r as crate::object::This).is_some();
    // Unordered views: whether a buffer, and the title's view dimension.
    let (buffer, dimension) = match view {
        View::Unordered(r, _, desc) => (
            is_buffer(r) || desc.is_some_and(|d| d[1] == 1),
            desc.map(|d| d[1]),
        ),
        View::Placed(true, resource, desc) => (resource[0] == 1, Some(desc[1])),
        // Shader views of buffers read nothing through a null buffer view:
        // a null 2D view where the shader expects a buffer is a mismatch
        // the hardware does not survive in general (GPU-based validation
        // reports it).
        View::Shader(r, desc) if is_buffer(r) || desc.is_some_and(|d| d[1] == 1) => {
            return null_buffer_view(at);
        }
        View::Placed(false, resource, _) if resource[0] == 1 => return null_buffer_view(at),
        // Textures: a null view of the title's dimension (1D, arrays, 3D,
        // cubes), not always 2D.
        View::Shader(_, Some(desc)) => return null_like(desc, at),
        View::Placed(false, _, desc) => {
            #[cfg(feature = "diagnostics")]
            if magenta_for(desc, at) {
                return;
            }
            return null_like(desc[..10].try_into().unwrap_or(&[0; 10]), at);
        }
        View::Constant(..) => {
            let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC::default();
            // SAFETY: a slot of our heap; a zero location makes a null view.
            return unsafe { host.device.CreateConstantBufferView(Some(&desc), at) };
        }
        _ => return null_view(at),
    };
    let desc = if buffer {
        D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            ..Default::default()
        }
    } else {
        null_uav_desc(dimension)
    };
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device.CreateUnorderedAccessView(
            None::<&ID3D12Resource>,
            None::<&ID3D12Resource>,
            Some(&desc),
            at,
        )
    };
}

/// A null texture UAV of the title's view dimension (1D, 1D or 2D arrays,
/// 3D), 2D if it names none: a 2D null view where the shader writes a 3D
/// texture is a mismatch GPU-based validation reports (the world's wind
/// field, CS_CopyWindFieldBeforeUpdateWithSmooth, 2026-11-24).
fn null_uav_desc(dimension: Option<u32>) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    let mut desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: DXGI_FORMAT_R32_UINT,
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    match dimension.map(|d| D3D12_UAV_DIMENSION(d as i32)) {
        Some(D3D12_UAV_DIMENSION_TEXTURE1D) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE1D;
        }
        Some(D3D12_UAV_DIMENSION_TEXTURE1DARRAY) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE1DARRAY;
            desc.Anonymous.Texture1DArray = D3D12_TEX1D_ARRAY_UAV {
                ArraySize: 1,
                ..Default::default()
            };
        }
        Some(D3D12_UAV_DIMENSION_TEXTURE2DARRAY) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
            desc.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV {
                ArraySize: 1,
                ..Default::default()
            };
        }
        Some(D3D12_UAV_DIMENSION_TEXTURE3D) => {
            desc.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE3D;
            desc.Anonymous.Texture3D = D3D12_TEX3D_UAV {
                WSize: u32::MAX,
                ..Default::default()
            };
        }
        _ => {}
    }
    desc
}

/// A buffer view that reads zeros (raw, as translated shaders mostly read
/// buffers).
fn null_buffer_view(at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R32_TYPELESS,
        ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Buffer: D3D12_BUFFER_SRV {
                NumElements: 1,
                Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                ..Default::default()
            },
        },
    };
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device
            .CreateShaderResourceView(None::<&ID3D12Resource>, Some(&desc), at)
    };
}

/// A texture view that reads zeros with the dimension (and mip and array
/// counts) of the title's description `words`; 2D if it names none.
fn null_like(words: &[u32; 10], at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    const TEXTURES: std::ops::RangeInclusive<u32> = 2..=10;
    if !TEXTURES.contains(&words[1]) {
        return null_view(at);
    }
    let Some(host) = host() else { return };
    // One mip, one slice or cube, plane 0: fields a null view still checks.
    let mut w = [0u32; 10];
    w[1] = words[1];
    match words[1] {
        6 => {}
        7 => w[5] = 1,
        3 | 5 | 10 => (w[5], w[7]) = (1, 1),
        _ => w[5] = 1,
    }
    let mut desc = srv_desc(&w);
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device
            .CreateShaderResourceView(None::<&ID3D12Resource>, Some(&desc), at)
    };
}

/// A view that reads zeros, for slots whose view the host cannot make.
/// shortcut: slots with no view at all (kind unknown) get this null 2D
/// view; null buffer views there did not stop the world's low-address
/// page faults (2026-11-24, run w10), so they are not the cause.
fn null_view(at: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let Some(host) = host() else { return };
    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MipLevels: 1,
                ..Default::default()
            },
        },
    };
    // SAFETY: a slot of our heap; a null resource is allowed with a desc.
    unsafe {
        host.device
            .CreateShaderResourceView(None::<&ID3D12Resource>, Some(&desc), at)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A float3 view off a 12-byte element becomes a one-channel view of
    /// the same dwords: element i, channel k at dword 3 * i + k.
    #[test]
    fn float3_views_as_dwords() {
        // 8 bytes into the copy, element 5 on, 10 elements: bytes 68..188.
        let (f, first, count) = f3_as_dwords(DXGI_FORMAT_R32G32B32_FLOAT, 8, 5, 10).unwrap();
        assert_eq!(f, DXGI_FORMAT_R32_FLOAT);
        assert_eq!((first, count), (17, 30));
        assert_eq!(first * 4, 8 + 5 * 12);
        assert_eq!((first + u64::from(count)) * 4, 8 + 15 * 12);
        // Element i, channel k of the title's view is the same byte.
        for i in 0..10u64 {
            for k in 0..3u64 {
                assert_eq!((first + 3 * i + k) * 4, 8 + (5 + i) * 12 + 4 * k);
            }
        }
        // On an element: the same rule (one shape for the translator).
        assert_eq!(
            f3_as_dwords(DXGI_FORMAT_R32G32B32_UINT, 24, 1, 6),
            Some((DXGI_FORMAT_R32_UINT, 9, 18))
        );
        assert_eq!(
            f3_as_dwords(DXGI_FORMAT_R32G32B32_SINT, 4, 0, 1),
            Some((DXGI_FORMAT_R32_SINT, 1, 3))
        );
        // Not on a dword, other formats, overflow: not this shape.
        assert_eq!(f3_as_dwords(DXGI_FORMAT_R32G32B32_FLOAT, 2, 0, 1), None);
        assert_eq!(f3_as_dwords(DXGI_FORMAT_R32G32B32A32_FLOAT, 4, 0, 1), None);
        assert_eq!(f3_as_dwords(DXGI_FORMAT_R32G32B32_TYPELESS, 4, 0, 1), None);
        assert_eq!(f3_as_dwords(DXGI_FORMAT_R32G32B32_FLOAT, 4, 0, u32::MAX), None);
        assert_eq!(f3_as_dwords(DXGI_FORMAT_R32G32B32_FLOAT, 4, u64::MAX, 1), None);
    }

    /// The one-channel shape is one the host takes, and is cut at the
    /// copy's end in dwords.
    #[test]
    fn float3_dword_views_are_valid() {
        let (f, first, count) = f3_as_dwords(DXGI_FORMAT_R32G32B32_FLOAT, 4, 0, 46).unwrap();
        assert!(buffer_view_ok(f, first, count, 0, false, 4 + 46 * 12));
        assert!(!buffer_view_ok(f, first, count, 0, false, 46 * 12));
        let count = elements_inside(first, count, 4, 46 * 12);
        assert_eq!(count, 137);
        assert!(buffer_view_ok(f, first, count, 0, false, 46 * 12));
    }

    /// A null texture UAV keeps the title's dimension (3D for the world's
    /// wind field), and is 2D when the title names none.
    #[test]
    fn null_uavs_keep_the_titles_dimension() {
        let d = null_uav_desc(Some(8));
        assert_eq!(d.ViewDimension, D3D12_UAV_DIMENSION_TEXTURE3D);
        // SAFETY: the member the dimension names.
        assert_eq!(unsafe { d.Anonymous.Texture3D.WSize }, u32::MAX);
        let d = null_uav_desc(Some(5));
        assert_eq!(d.ViewDimension, D3D12_UAV_DIMENSION_TEXTURE2DARRAY);
        // SAFETY: the member the dimension names.
        assert_eq!(unsafe { d.Anonymous.Texture2DArray.ArraySize }, 1);
        let dimension = |d| null_uav_desc(d).ViewDimension;
        assert_eq!(dimension(Some(2)), D3D12_UAV_DIMENSION_TEXTURE1D);
        assert_eq!(dimension(Some(4)), D3D12_UAV_DIMENSION_TEXTURE2D);
        assert_eq!(dimension(None), D3D12_UAV_DIMENSION_TEXTURE2D);
        // Multisampled UAVs need a cap; they stay 2D.
        assert_eq!(dimension(Some(6)), D3D12_UAV_DIMENSION_TEXTURE2D);
    }

    /// Views of one placed texture are told from views of others by the
    /// address and the resource description, not by what they show.
    #[test]
    fn views_of_one_placed_texture_are_told_by_address_and_description() {
        let texture = [3, 0, 0, 0, 512, 0, 512, 0x8_0001, 98, 1, 0, 0x105, 0, 0];
        let view = |resource: [u32; 14], first_mip: u32, address: u32| {
            let mut desc = [0u32; 16];
            (desc[0], desc[1], desc[4], desc[5], desc[10], desc[11]) = (98, 4, first_mip, 8 - first_mip, address, 0xb);
            View::Placed(false, resource, desc)
        };
        let low = view(texture, 3, 0x1000_0000);
        // more mips of the same texture
        assert!(same_placed_texture(&low, &view(texture, 0, 0x1000_0000)));
        // another address, another description, a buffer, an unordered view
        assert!(!same_placed_texture(&low, &view(texture, 3, 0x2000_0000)));
        let mut wider = texture;
        wider[4] = 1024;
        assert!(!same_placed_texture(&low, &view(wider, 3, 0x1000_0000)));
        let mut buffer = texture;
        buffer[0] = 1;
        assert!(!same_placed_texture(&view(buffer, 3, 0x1000_0000), &view(buffer, 3, 0x1000_0000)));
        let View::Placed(_, resource, desc) = low.clone() else { unreachable!() };
        assert!(!same_placed_texture(&low, &View::Placed(true, resource, desc)));
        assert!(!same_placed_texture(&low, &View::Shader(0, None)));
    }

    /// The texture that stands for missing ones can be made and viewed.
    #[cfg(feature = "diagnostics")]
    #[test]
    fn a_magenta_texture_is_made() {
        let Some(host) = host() else {
            return; // no host GPU
        };
        let Some(heap) = HostHeap::new(false, 4) else {
            return;
        };
        let texture = magenta().expect("the texture");
        // SAFETY: a slot of our heap and a live texture.
        unsafe { host.device.CreateShaderResourceView(texture, None, heap.cpu(0)) };
        // SAFETY: a plain device query.
        assert!(unsafe { host.device.GetDeviceRemovedReason() }.is_ok());
    }

    /// A null 3D UAV made on the real device leaves it alive.
    #[test]
    fn null_3d_uav_is_accepted_by_the_device() {
        let Some(host) = host() else {
            return; // no host GPU
        };
        let Some(heap) = HostHeap::new(false, 4) else {
            return;
        };
        let view = View::Unordered(0, 0, Some([28, 8, 0, 0, 0, 0, 0, 0, 0, 0]));
        null_for(&view, heap.cpu(0));
        // SAFETY: a plain device query.
        assert!(unsafe { host.device.GetDeviceRemovedReason() }.is_ok());
    }

    /// Dynamic slots stay taken until given back, and runs do not wrap.
    #[test]
    fn a_structured_view_made_raw_passes_the_host_check() {
        // 100 elements of 20 bytes from element 8, in a copy that starts 32
        // bytes before the view's memory: bytes 192..2192.
        let (first, count) = raw_window(32, 8, 100, 20).unwrap();
        assert_eq!((first, count), (48, 500));
        assert!(buffer_view_ok(DXGI_FORMAT_R32_TYPELESS, first, count, 0, true, 4096));
        // the format a raw view must name; the structured one's is refused
        assert!(!buffer_view_ok(DXGI_FORMAT_UNKNOWN, first, count, 0, true, 4096));
        // past the buffer, not on 16 bytes, no elements
        assert!(!buffer_view_ok(DXGI_FORMAT_R32_TYPELESS, first, count, 0, true, 2000));
        assert!(raw_window(4, 0, 100, 20).is_none());
        assert!(!buffer_view_ok(DXGI_FORMAT_R32_TYPELESS, 0, 0, 0, true, 4096));
        // a structured view the host takes as it is
        assert!(buffer_view_ok(DXGI_FORMAT_UNKNOWN, 1, 10, 12, false, 4096));
    }

    /// The queue of textures picked by index serves the slots looked at
    /// before first, then the newest write and the oldest in turns; a
    /// slot whose memory is not there is looked at again after
    /// 1, 2, 4 frames up to the limit; and what the pass reports about a
    /// slot written again meanwhile is dropped.
    #[test]
    fn constant_views_are_whole_and_at_most_64_kib() {
        assert_eq!(constant_size(1), 256);
        assert_eq!(constant_size(256), 256);
        assert_eq!(constant_size(65_536), 65_536);
        assert_eq!(constant_size(969_472), 65_536);
        assert_eq!(constant_size(u32::MAX), 65_536);
    }

    #[test]
    fn views_past_the_end_are_cut_to_the_elements_inside() {
        assert_eq!(elements_inside(0, 10, 16, 160), 10);
        assert_eq!(elements_inside(4, 10, 16, 160), 6);
        assert_eq!(elements_inside(4, 10, 16, 167), 6);
        // Nothing inside: as it was, for the host check to refuse.
        assert_eq!(elements_inside(10, 10, 16, 160), 10);
        assert_eq!(elements_inside(0, 7, 4, 1 << 40), 7);
    }

    #[test]
    fn the_texture_queue_serves_the_newest_writes_first() {
        let mut q = TextureQueue::default();
        // Slots 10..18 written in frames 1..9, slot 5 last.
        for i in 0..8u32 {
            q.written(10 + i, u64::from(i) + 1, 0, false);
        }
        q.written(5, 9, 0, false);
        let order: Vec<u32> = q.due(9, 100).into_iter().map(|d| d.0).collect();
        // newest, oldest, newest, oldest, and so on
        assert_eq!(order, [5, 10, 17, 11, 16, 12, 15, 13, 14]);
        assert_eq!(q.due(9, 2).len(), 2);
        assert_eq!(q.census(9), [9, 9, 0, 0, 8]);
        // Slot 17's memory is not mapped: again next frame, then in 2, 4, 4.
        let write = |q: &TextureQueue, index: u32| q.due(u64::MAX, 100).iter().find(|d| d.0 == index).unwrap().1;
        let w17 = write(&q, 17);
        let mut frame = 9;
        for wait in [1, 2, 4, 4] {
            q.later(17, w17, frame, 4);
            assert!(!q.due(frame + wait - 1, 100).iter().any(|d| d.0 == 17));
            frame += wait;
            // Due again, and ahead of every slot never looked at.
            assert_eq!(q.due(frame, 100)[0].0, 17);
        }
        assert_eq!(q.census(9)[2], 1);
        q.done(17, w17);
        assert!(!q.due(u64::MAX, 100).iter().any(|d| d.0 == 17));
        // Uploaded with part of its memory there: kept, with what was there.
        let w16 = write(&q, 16);
        q.partly(16, w16, 12, 4096);
        assert_eq!(q.mapped(16), 4096);
        assert_eq!(q.due(12, 100)[0].0, 16);
        // The title writes slot 16 again while the pass works on it: what
        // the pass then says about the old write changes nothing.
        q.written(16, 13, 0, false);
        q.done(16, w16);
        q.later(16, w16, 13, 4);
        assert_eq!(q.mapped(16), 0);
        assert_eq!(q.due(13, 100)[0].0, 16);
        // A draw that makes the host slot (`seen`) keeps what the queue knows,
        let w15 = write(&q, 15);
        q.partly(15, w15, 20, 8192);
        q.written(15, 14, 0, true);
        assert_eq!(q.mapped(15), 8192);
        // and queues a slot the queue did not hold.
        q.written(99, 14, 0, true);
        assert!(q.due(14, 100).iter().any(|d| d.0 == 99));
        // A write due later (a table's slot) is not handed out before.
        q.written(98, 14, 16, false);
        assert!(!q.due(15, 100).iter().any(|d| d.0 == 98));
        assert!(q.due(16, 100).iter().any(|d| d.0 == 98));
    }

    /// A view written over memory that older views describe otherwise
    /// makes those stale; views of the same span are one texture's.
    #[test]
    fn a_newer_view_of_other_memory_makes_the_older_stale() {
        let mut spans = Spans::default();
        // Three textures side by side, the first in two slots.
        assert!(spans.note(1, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert!(spans.note(2, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert!(spans.note(3, 0x3000, 0x4000, &shape(0x3000, 0x4000), true).is_empty());
        assert!(spans.note(4, 0x8000, 0x9000, &shape(0x8000, 0x9000), true).is_empty());
        // The same slot written with the same span again: nothing changes.
        assert!(spans.note(2, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert_eq!(spans.slots_in(0x2000, 0x3800), [1, 2, 3]);
        assert_eq!(spans.slots_in(0x4000, 0x8000), [] as [u32; 0]);
        // A draw makes a slot whose view overlaps them: left out, nothing stale.
        assert!(spans.note(9, 0x2000, 0x3800, &shape(0x2000, 0x3800), false).is_empty());
        assert_eq!(spans.slots_in(0x2000, 0x3800), [1, 2, 3]);
        // The title writes a texture over the end of the first and all of
        // the second: their three slots are stale.
        let mut stale = spans.note(5, 0x2000, 0x4000, &shape(0x2000, 0x4000), true);
        stale.sort_unstable();
        assert_eq!(stale, [1, 2, 3]);
        assert_eq!(spans.slots_in(0, u64::MAX), [5, 4]);
        // Another start in the same memory: stale in turn. Another length
        // from the same start too.
        assert_eq!(spans.note(6, 0x2800, 0x3000, &shape(0x2800, 0x3000), true), [5]);
        assert_eq!(spans.note(7, 0x2800, 0x2c00, &shape(0x2800, 0x2c00), true), [6]);
        // A slot that moves to another texture leaves its span behind.
        assert!(spans.note(7, 0x8000, 0x9000, &shape(0x8000, 0x9000), true).is_empty());
        assert_eq!(spans.slots_in(0x2800, 0x2c00), [] as [u32; 0]);
        assert_eq!(spans.slots_in(0x8000, 0x8001), [4, 7]);
        spans.forget(4);
        spans.forget(7);
        assert!(spans.spans.is_empty() && spans.by_slot.is_empty());
    }

    /// Views of one texture that show more and fewer of its mips are all
    /// its own, and the next texture may begin where the mips that are
    /// there end.
    #[test]
    fn a_texture_of_which_the_smaller_mips_are_there_ends_behind_them() {
        // As the title has them: 512x512 BC7 of which 64x64 is
        // there, and a 1024x512 BC1 texture 8 KiB on.
        let texture = [3, 0, 0, 0, 512, 0, 512, 0x8_0001, 99, 1, 0, 0x105, 0, 0];
        let next = [3, 0, 0, 0, 1024, 0, 512, 0x8_0001, 71, 1, 0, 0x105, 0, 0];
        let mut spans = Spans::default();
        let at = 0xb_5c04_0000u64;
        let end = |spans: &mut Spans, desc: &[u32; 14], at: u64, first| at + spans.bytes(desc, first).unwrap();
        let a = end(&mut spans, &texture, at, 3);
        assert_eq!(a, at + 0x2000);
        assert!(spans.note(1, at, a, &texture, true).is_empty());
        let b = end(&mut spans, &next, a, 3);
        assert!(b <= a + 0x4000);
        assert!(spans.note(2, a, b, &next, true).is_empty());
        assert_eq!(spans.slots_in(at, b), [1, 2]);
        // The smaller mips alone in another slot: the same texture's.
        let small = end(&mut spans, &texture, at, 5);
        assert!(small < a);
        assert!(spans.note(3, at, small, &texture, true).is_empty());
        assert_eq!(spans.slots_in(at, at + 1), [3, 1]);
        // More of it streamed in, over the next texture: that one is stale.
        let more = end(&mut spans, &texture, at, 1);
        assert!(more > b);
        assert_eq!(spans.note(1, at, more, &texture, true), [2]);
        assert_eq!(spans.claim(3, at, small, &texture), (Claim::Own, None));
        assert_eq!(spans.claim(2, a, b, &next), (Claim::Other, Some((at, more))));
    }

    /// A resource description of its own for every span.
    fn shape(start: u64, end: u64) -> [u32; 14] {
        let mut desc = [0; 14];
        (desc[0], desc[4], desc[6]) = (3, start as u32, end as u32);
        desc
    }

    /// Whose the memory of a view is: the slot's own span, no other's, or
    /// one that says otherwise.
    #[test]
    fn spans_say_whose_memory_a_view_is() {
        let mut spans = Spans::default();
        assert!(spans.note(1, 0x1000, 0x3000, &shape(0x1000, 0x3000), true).is_empty());
        assert_eq!(spans.claim(1, 0x1000, 0x3000, &shape(0x1000, 0x3000)), (Claim::Own, None));
        // Another slot's view of the same texture, or of memory nothing describes.
        assert_eq!(spans.claim(2, 0x1000, 0x3000, &shape(0x1000, 0x3000)), (Claim::Free, None));
        assert_eq!(spans.claim(2, 0x3000, 0x4000, &shape(0x3000, 0x4000)), (Claim::Free, None));
        // The same start with another length; another start.
        assert_eq!(spans.claim(2, 0x1000, 0x2000, &shape(0x1000, 0x2000)), (Claim::SameStart, Some((0x1000, 0x3000))));
        assert_eq!(spans.claim(2, 0x800, 0x2000, &shape(0x800, 0x2000)), (Claim::Other, Some((0x1000, 0x3000))));
    }

    /// A view that fell out of the queue with its memory there and nothing
    /// to show for it goes back; one a newer view speaks against does not.
    #[test]
    fn a_texture_sweep_finds_views_nothing_brings_up_to_date() {
        let Some(host) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        // Below 2^40, as a title's memory is.
        // SAFETY: new memory, left to the process.
        let at = unsafe {
            use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
            VirtualAlloc(0x7d_4000_0000usize as *const _, 1 << 20, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) as u64
        };
        if at == 0 {
            return; // taken
        }
        let mut slots = vec![[0u32; 8]; 8];
        let base = slots.as_mut_ptr() as u64;
        let handle = |index: u32| Some(base + u64::from(index) * 32);
        let resource = [3, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0];
        let view = |width: u32, address: u64| {
            let mut desc = [0u32; 16];
            (desc[10], desc[11]) = (address as u32, (address >> 32) as u32);
            let mut resource = resource;
            resource[4] = width;
            View::Placed(false, resource, desc)
        };
        super::super::record(handle(1).unwrap(), view(64, at));
        super::super::record(handle(2).unwrap(), view(64, at + 0x8_0000));
        // A newer, wider texture in the second one's memory.
        host.note_buffer(5, Some(&view(128, at + 0x8_0000)), false);
        let queued = || -> Vec<u32> { host.texture_slots_due(u64::MAX, 8).0.into_iter().map(|d| d.0).collect() };
        for slot in queued() {
            host.textures.lock().unwrap().remove(slot);
        }
        let mut counts = [0; 10];
        host.sweep_part(0..8, handle, 50, false, &mut counts);
        assert_eq!(counts, [2, 0, 0, 0, 0, 0, 1, 1, 0, 0]);
        assert_eq!(queued(), []);
        host.sweep_part(0..8, handle, 50, true, &mut counts);
        assert_eq!(queued(), [1]);
    }

    /// Memory the runtime maps under a slot's texture makes the slot due
    /// again from the next frame; a stale slot leaves the queue.
    #[test]
    fn remapped_memory_makes_its_texture_slots_due() {
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        let slots = |at: u64| -> Vec<u32> { heap.texture_slots_due(at, 8).0.into_iter().map(|d| d.0).collect() };
        // 64x64 BC7, one mip, at an address no test and no title uses.
        let at = 0x7fee_0000_0000u64;
        let view = |address: u64, width: u32| {
            let mut desc = [0u32; 16];
            (desc[10], desc[11]) = (address as u32, (address >> 32) as u32);
            View::Placed(false, [3, 0, 0, 0, width, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0], desc)
        };
        let len = crate::resource::placed_bytes(&[3, 0, 0, 0, 64, 0, 64, 0x1_0001, 98, 1, 0, 0x105, 0, 0]).unwrap();
        assert!(heap.note_buffer(1, Some(&view(at, 64)), false).is_empty());
        let (queued, write) = heap.texture_slots_due(u64::MAX, 8).0[0];
        assert_eq!(queued, 1);
        heap.texture_slot_done(1, write, None);
        assert_eq!(slots(u64::MAX), []);
        // Memory elsewhere: nothing. Under the texture: due from the next frame.
        remapped((at + 2 * len) as usize, 0x1000);
        remapped(at as usize, 0x1000);
        assert_eq!(heap.texture_slots_remapped(50), 1);
        assert_eq!(slots(50), []);
        assert_eq!(slots(51), [1]);
        // A wider texture written at the same address: slot 1 is stale.
        assert_eq!(heap.note_buffer(2, Some(&view(at, 128)), false), [1]);
        assert_eq!(slots(u64::MAX), [2]);
    }

    /// A slot a draw read through a table leaves the queue; the title's
    /// next write of it waits for its draw, and is due after the grace
    /// frames if none reads it.
    #[test]
    fn slots_read_through_tables_leave_the_texture_queue() {
        let Some(heap) = HostHeap::new(false, 8) else {
            return; // no host GPU
        };
        let frame = 100;
        let slots = |at: u64| -> Vec<u32> { heap.texture_slots_due(at, 8).0.into_iter().map(|d| d.0).collect() };
        let texture = View::Placed(false, [3, 0, 0, 0, 4, 0, 4, 0x1_0001, 28, 1, 0, 0x105, 0, 0], [0; 16]);
        assert!(is_texture_view(&texture));
        heap.queue_texture(2, true, false, frame);
        heap.queue_texture(3, true, false, frame);
        assert_eq!(slots(frame), [3, 2]);
        heap.tabled_in(3, true, frame);
        assert_eq!(slots(frame), [2]);
        // Made in place by the draw that read it: still out.
        heap.queue_texture(3, true, true, frame);
        assert_eq!(slots(frame), [2]);
        // Written again in the next frame: due once the grace frames have
        // passed without a draw that reads it.
        heap.queue_texture(3, true, false, frame + 1);
        assert_eq!(slots(frame + TABLED_GRACE), [2]);
        assert_eq!(slots(frame + 1 + TABLED_GRACE), [3, 2]);
        // The draw comes after all: out again.
        heap.tabled_in(3, true, frame + 1);
        assert_eq!(slots(u64::MAX), [2]);
        // A slot of a table no draw read stays queued; its next write waits.
        heap.tabled_in(2, false, frame + 1);
        assert_eq!(slots(frame + 1), [2]);
        heap.queue_texture(2, true, false, frame + 2);
        assert_eq!(slots(frame + 2), []);
        // Long after the last table, a write is due at once.
        heap.queue_texture(2, true, false, frame + 3 + TABLED_FRAMES);
        assert_eq!(slots(frame + 3 + TABLED_FRAMES), [2]);
    }

    #[test]
    fn dynamic_slots_are_held_until_released() {
        let Some(heap) = HostHeap::new(false, 4) else {
            return; // no host GPU
        };
        let a = heap.dynamic(heap.dynamic - 101).unwrap(); // the last slot is never given
        let b = heap.dynamic(60).unwrap();
        assert_eq!((a, b), (4, 4 + heap.dynamic - 101));
        // 40 left at the end: 50 do not fit until something is given back.
        assert_eq!(heap.dynamic(50), None);
        heap.release(a, 50);
        assert_eq!(heap.dynamic(50), Some(4));
        assert_eq!(heap.dynamic(40), Some(4 + heap.dynamic - 41));
    }

    /// The title's view heap on NVIDIA's 1,000,000 views with a
    /// heap map: title slots below 131,072 at their own index, 606,782 pool
    /// slots, the map at 737,854, and the fork's dynamic slots (from
    /// 737,855) and guard slot (999,999, the translator's limit) where they
    /// were. A window past the room is cut to it; the pool holds no more
    /// slots than the title has past the window.
    #[test]
    fn the_titles_view_heap_is_laid_out_in_a_million_slots() {
        let l = MapLayout::new(1_245_184, WINDOW, MAP_SLOT);
        assert_eq!((l.window, l.pool, l.slot), (131_072, 606_782, 737_854));
        assert_eq!(l.dynamic_first(), PORTABLE_VIEWS - DYNAMIC - 1, "the fork's first dynamic slot");
        assert_eq!(l.guard(), PORTABLE_VIEWS - 1, "the fork's guard slot");
        assert_eq!(l.size(), PORTABLE_VIEWS);
        let wide = MapLayout::new(1_245_184, 900_000, MAP_SLOT);
        assert_eq!((wide.window, wide.pool), (MAP_SLOT, 0));
        let small = MapLayout::new(20, 16, 24);
        assert_eq!((small.window, small.pool, small.dynamic_first()), (16, 4, 25));
        assert_eq!(small.size(), 25 + DYNAMIC + 1);
    }

    /// A view heap made while the translator has a heap map (as a capped
    /// host's `probe_view_limit` sets it) is laid out as [`MapLayout`] says,
    /// its map's raw view at the map slot; one no larger than the window
    /// keeps the fork's layout.
    #[test]
    fn a_view_heap_with_a_heap_map_keeps_the_dynamic_slots_after_the_map() {
        if host().is_none() {
            return;
        }
        TEST_MAP.set(Some((16, 24)));
        let mapped = HostHeap::new(false, 64);
        let fits = HostHeap::new(false, 8);
        TEST_MAP.set(None);
        let (mapped, fits) = (mapped.unwrap(), fits.unwrap());
        assert_eq!((mapped.count(), mapped.title_count, mapped.dynamic_first), (16, 64, 25));
        assert!(mapped.map.is_some());
        {
            let made = mapped.made.lock().unwrap();
            assert_eq!(made.keys.len(), 16 + 8, "the window's and the pool's");
            assert_eq!(made.map.as_ref().map(SlotMap::len), Some(48));
        }
        assert_eq!(mapped.tabled.len(), 64, "by title slot");
        assert_eq!(mapped.kept.lock().unwrap().len(), (25 + DYNAMIC) as usize);
        assert!(mapped.kept.lock().unwrap()[24].is_some(), "the map's raw view holds it");
        assert_eq!(mapped.dynamic(DYNAMIC), Some(25));
        assert_eq!(mapped.dynamic(1), None);
        assert_eq!((fits.count(), fits.dynamic_first), (8, 8));
        assert!(fits.map.is_none() && fits.made.lock().unwrap().map.is_none());
        assert_eq!(fits.dynamic(1), Some(8));
    }

    /// A title view heap of `count` slots and its first descriptor, made
    /// as the title makes them (its host heap is made at first use).
    fn title_heap(count: u32) -> (crate::object::This, u64) {
        #[repr(C)]
        struct Extended(super::super::HeapDesc, super::super::Placement);
        let desc = super::super::HeapDesc {
            kind: 0,
            count,
            flags: 1,
            nodes: 0,
        };
        let none = super::super::Placement { address: 0, size: 0 };
        let extended = Extended(desc, none);
        let mut out = std::ptr::null();
        let made = super::super::create_heap(
            std::ptr::null(),
            &extended.0,
            &super::super::HEAP_IIDS[0],
            &mut out,
        );
        assert_eq!(made, crate::object::S_OK);
        (out, super::super::heap(out).start)
    }

    /// Writes a constant buffer view of `address` into title slot `slot`.
    fn title_cbv(start: u64, slot: u32, address: u64) {
        super::super::create_cbv(
            std::ptr::null(),
            &[address as u32, (address >> 32) as u32, 256, 0],
            start + u64::from(slot) * TITLE_SLOT_BYTES,
        );
    }

    fn release(object: crate::object::This) -> u32 {
        crate::object::slot::<extern "system" fn(crate::object::This) -> u32>(object, 0x10)(object)
    }

    /// The heap map entry of title slot `title`, and its pool slot.
    fn entry(heap: &HostHeap, title: u32) -> (u32, Option<u32>) {
        let window = heap.count();
        let at = heap.map.as_ref().unwrap().at;
        // SAFETY: an entry of the mapped heap map.
        let entry = unsafe { (at as *const u32).add((title - window) as usize).read_volatile() };
        let pool = heap.made.lock().unwrap().map.as_ref().unwrap().host(title);
        (entry, pool)
    }

    /// Title slots past the identity window that the title writes take
    /// pool slots, published in the heap map (entries hold the slot's
    /// bitwise not) once made; slots below it keep their index. A slot the
    /// title writes again keeps its pool slot; one it empties gives it back
    /// (entry zeroed: the guard), quarantined while submissions run.
    #[test]
    fn title_slots_past_the_window_take_pool_slots() {
        if host().is_none() {
            return;
        }
        let (title, start) = title_heap(64);
        title_cbv(start, 3, 0x1_0000);
        title_cbv(start, 40, 0x2_0000);
        TEST_MAP.set(Some((16, 24)));
        let heap = super::super::heap(title).host().expect("a host heap");
        TEST_MAP.set(None);
        assert_eq!(heap.count(), 16);
        let (e40, pooled) = entry(heap, 40);
        let pooled = pooled.expect("a pool slot");
        assert!((16..24).contains(&pooled), "{pooled}");
        assert_eq!(e40, !pooled);
        assert_eq!(entry(heap, 41), (0, None));
        assert_eq!(heap.made.lock().unwrap().host(3), Some(3), "below the window: its own");
        assert_eq!(heap.title_start.load(std::sync::atomic::Ordering::Relaxed), start);

        // Written again: the same pool slot. Another slot: another one.
        title_cbv(start, 40, 0x3_0000);
        assert_eq!(entry(heap, 40), (!pooled, Some(pooled)));
        title_cbv(start, 63, 0x3_0000);
        let other = entry(heap, 63).1.expect("a pool slot");
        assert_ne!(other, pooled);

        // Emptied (as a write of no view finds it): the entry reads the
        // guard and the pool slot waits.
        let handle = start + 40 * TITLE_SLOT_BYTES;
        // SAFETY: a descriptor slot of the title heap.
        unsafe { (handle as *mut u8).write_bytes(0, 32) };
        assert!(heap.mirror(40, handle, false).is_none());
        assert_eq!(entry(heap, 40), (0, None));
        let stats = heap.made.lock().unwrap().map.as_ref().unwrap().stats();
        assert_eq!((stats.mapped, stats.quarantined), (1, 1));
        assert_eq!(release(title), 0);
    }

    /// With the pool used up, the pool slots of title slots that hold no
    /// live view any more (overwritten as bytes: no descriptor write says
    /// so) are given back, and the slot that asked takes one.
    #[test]
    fn a_used_up_pool_gives_back_slots_without_views() {
        if host().is_none() {
            return;
        }
        let (title, start) = title_heap(64);
        TEST_MAP.set(Some((16, 20)));
        let heap = super::super::heap(title).host().expect("a host heap");
        TEST_MAP.set(None);
        for slot in 16..20 {
            title_cbv(start, slot, 0x1_0000 + u64::from(slot) * 0x100);
        }
        let freed = entry(heap, 17).1.expect("a pool slot");
        // SAFETY: a descriptor slot of the title heap.
        unsafe { ((start + 17 * TITLE_SLOT_BYTES) as *mut u8).write_bytes(0, 32) };
        title_cbv(start, 30, 0x9_0000);
        assert_eq!(entry(heap, 17), (0, None), "given back");
        assert_eq!(entry(heap, 16).1.is_some() && entry(heap, 19).1.is_some(), true);
        // Handed out once the submissions running when it was given back
        // have finished (other tests' may be).
        let (running, _) = crate::host::submissions();
        let waited = std::time::Instant::now();
        while entry(heap, 30).1.is_none() && waited.elapsed() < std::time::Duration::from_secs(20) {
            if crate::host::submissions().1 >= running {
                let _ = heap.mirror(30, start + 30 * TITLE_SLOT_BYTES, false);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        assert_eq!(entry(heap, 30), (!freed, Some(freed)));
        assert_eq!(release(title), 0);
    }
}

#[cfg(test)]
mod pad_null {
    #[test]
    fn small_heaps_reach_the_clamp_slot() {
        assert_eq!(super::padded_to(10, 0), None);
        assert_eq!(super::padded_to(10, 999_999), Some(1_000_000));
        assert_eq!(super::padded_to(999_999, 999_999), Some(1_000_000));
        assert_eq!(super::padded_to(1_000_000, 999_999), None);
    }
}
