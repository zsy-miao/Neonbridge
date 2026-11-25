//! Host descriptor heaps behind the title's shader-visible heaps (decision
//! 0007, step 3). Translated shaders index the heaps directly at the
//! title's descriptor positions, so each host heap keeps the title heap's
//! order: slot `i` is made from what the title wrote at slot `i`, when it
//! writes it (view creation and descriptor copies) and again when a draw
//! reads it through a table and finds the slot changed, or when a bindless
//! `ResourceDescriptorHeap[]` draw or dispatch materializes and refreshes
//! deferred buffer, constant-buffer, and placed-texture slots.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::{VIEW_TAG, View, stale_views, view_at};
use crate::host::{Host, host};

pub(crate) struct HostHeap {
    pub(crate) heap: ID3D12DescriptorHeap,
    cpu: usize,
    step: u32,
    /// Slots mirroring the title's; `spare` more follow them.
    count: u32,
    /// Dynamic slots past the title's: [`DYNAMIC`] in a view heap,
    /// [`SAMPLER_DYNAMIC`] in a capped sampler heap, none in another.
    spare: u32,
    sampler: bool,
    /// The record (view index and generation) each slot was last made from.
    made: Mutex<Vec<u64>>,
    /// The host resource each slot views, held while the slot does: shaders
    /// index heaps directly, so a slot the title has not rewritten since it
    /// released the resource may still be read, and a freed host resource
    /// read there is a GPU page fault (the console reads stale memory).
    kept: Mutex<Vec<Option<ID3D12Resource>>>,
    /// Owning `HostTexture` (and its `texpool::Slot` / `ID3D12Heap`) for any
    /// slot viewing a pooled texture, so the backing heap cannot be freed while
    /// a shader-visible descriptor slot still points into it.
    kept_textures: Mutex<HashMap<u32, Arc<crate::resource::HostTexture>>>,
    /// Where the search for free dynamic slots starts, and which are taken
    /// (by lists not yet reset).
    /// Where the next search starts, the slots taken, and the smallest run
    /// known not to be free since the last release (`usize::MAX`: none).
    next: Mutex<(u32, Vec<bool>, usize)>,
    /// Per-slot element byte stride (`u32`) for structured buffers whose
    /// stride shader analysis could not recover statically.
    strides: Option<StrideTable>,
    /// Populated buffer, constant-buffer, and placed-texture slots (`< count`)
    /// tracked for bindless `ResourceDescriptorHeap[]` materialization.
    bindless: Mutex<Bindless>,
    /// A recording thread is preparing the placed-texture slots (others
    /// skip it rather than wait).
    recording: std::sync::atomic::AtomicBool,
    /// Placed-texture slots are waiting to be prepared at recording.
    record_hint: std::sync::atomic::AtomicBool,
    /// The placed-texture epoch, and the frame (plus one), recording last
    /// looked for evicted textures at.
    record_looked: [AtomicU64; 2],
    /// The frame the oldest uploaded texture is due a look again in.
    recheck_at: AtomicU64,
    /// The rewritten textures ([`crate::resource::rewrites`]) seen.
    rewrites_seen: AtomicU64,
    /// The views of memory by address made ([`crate::memory::fill`]).
    memory_seen: Mutex<u64>,
}

/// The dword of the stride table the element counts start at.
const COUNTS_AT: u32 = victory_shader::emit::OWN_SLOTS;

struct StrideTable {
    _resource: ID3D12Resource,
    ptr: usize,
    gpu: u64,
    len: u32,
}

impl StrideTable {
    /// The element counts follow the strides, from [`COUNTS_AT`] dwords on
    /// (where shaders, which know no heap's size, look for them).
    fn new(device: &ID3D12Device, len: u32) -> Option<Self> {
        let bytes = ((u64::from(COUNTS_AT) + u64::from(len)) * 4).max(4);
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut resource: Option<ID3D12Resource> = None;
        let mut p = std::ptr::null_mut();
        // SAFETY: plain committed upload buffer creation and persistent CPU map.
        let gpu = unsafe {
            device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_UPLOAD,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                    None,
                    &mut resource,
                )
                .ok()?;
            let res = resource.as_ref()?;
            res.Map(0, None, Some(&mut p)).ok()?;
            std::ptr::write_bytes(p.cast::<u8>(), 0, bytes as usize);
            res.GetGPUVirtualAddress()
        };
        Some(Self {
            _resource: resource?,
            ptr: p as usize,
            gpu,
            len,
        })
    }
}

impl Drop for HostHeap {
    fn drop(&mut self) {
        if let Ok(mut kept) = self.kept.lock() {
            for res in kept.drain(..).flatten() {
                crate::host::retire(res);
            }
        }
        if let Ok(mut texs) = self.kept_textures.lock() {
            for (_, tex) in texs.drain() {
                crate::host::retire(tex);
            }
        }
    }
}

// SAFETY: D3D12 descriptor heaps and resources are free-threaded; mutable
// slot tracking is synchronized behind mutexes and atomics.
unsafe impl Send for HostHeap {}
// SAFETY: as above.
unsafe impl Sync for HostHeap {}

const BOUNCE_CHUNK: u64 = 256 << 10;

/// Persistent 256-byte-aligned bounce slice for a misaligned bindless CBV.
struct BounceSlot {
    gpu: u64,
    cpu: usize,
    cap: u32,
    resource: ID3D12Resource,
    active: bool,
}

#[derive(Default)]
struct Bindless {
    /// Active `View::is_buffer()` slots: `slot_index -> (view_index, generation)`.
    slots: BTreeMap<u32, (u32, u32)>,
    /// Of them, those viewing placed textures.
    placed_set: BTreeMap<u32, (u32, u32)>,
    /// Placed-texture slots changed since draws last prepared them.
    record_pending: BTreeSet<u32>,
    /// Persistent 256-byte-aligned bounce slices by `slot_index`.
    cbv_bounce: HashMap<u32, BounceSlot>,
    /// Backing 256 KiB shared buffers `(resource, cpu_ptr, gpu_va, used_bytes)`.
    bounce_chunks: Vec<(ID3D12Resource, usize, u64, u64)>,
    /// Slots written since the plan was made: only they are made again.
    dirty: BTreeSet<u32>,
    /// Slots of `dirty` in the order the title wrote them (some may be made
    /// since): the last ones are the tables of the work being submitted.
    recent: Vec<u32>,
    /// What each slot made for bindless access needs at submission.
    uses: BTreeMap<u32, SlotUse>,
    /// Of them, those with work at each submission.
    busy: BTreeSet<u32>,
    /// Subset of `uses` currently marked unmapped (checked on memory remap).
    unmapped_slots: std::collections::HashSet<u32>,
    /// Subset of `uses` holding a direct-memory window (checked for `is_dead`).
    direct_slots: std::collections::HashSet<u32>,
    /// The count of windows that died ([`crate::shadow::died`]) the direct
    /// slots were last looked at with.
    died_seen: u64,
    /// Slots came out otherwise since the whole plan was last built, with
    /// nothing the plan lacks ([`LOOSE_FRAMES`]).
    loose: bool,
    /// The host resources those slots view, held by the plans until then.
    extra: Vec<ID3D12Resource>,
    /// The released-view, placed-texture, and memory-remap epochs `uses` was checked at.
    checked: (u64, u64, u64),
    /// What submissions do for the slots, until they or what they view change.
    plan: Option<Arc<BindlessPlan>>,
    /// Placed-texture slots as last prepared at recording: view key,
    /// texture.
    recorded: HashMap<u32, (u64, usize)>,
    /// Placed textures reached through slots, by resource object: the host
    /// texture (held, so its address names it alone) and the frame last
    /// uploaded.
    uploaded: HashMap<usize, (ID3D12Resource, u64)>,
    /// Uploaded textures by frame, oldest first, to look at again.
    recheck: VecDeque<(u64, usize)>,
    /// A frame and the uploads of known textures done in it.
    rechecks: (u64, u32),
    /// When the slot-making budget was last refilled, and the microseconds
    /// left: plans at submission, placed textures at recording.
    budget: Option<(std::time::Instant, [u64; 2])>,
    /// The frame (plus one) released views and evicted textures were last
    /// looked for in: at submission, at recording. Once per frame: the title
    /// releases and evicts all the time, and each look is over all slots.
    looked: [u64; 2],
    /// The ranges of [`MAPPED`] seen.
    mapped_seen: u64,
    /// The frame (plus one) the plan's lists of everything were last made in,
    /// and whether slots changed since.
    full: (u64, bool),
}

/// Time for making changed slots, at submission and at recording: an eighth
/// of the time passing (4 ms of a 30 fps frame), banked up to this many
/// microseconds; what is left over waits.
///
/// shortcut: while the title streams thousands of slots in at once, the
/// slots not made yet read nothing (null views) for a few frames, as
/// streaming pops in; making all of them at once stalled frames for
/// seconds.
const SLOT_BUDGET_US: u64 = 20_000;
/// Slots taken to make at once (the rest wait without being looked at).
const SLOT_CHUNK: usize = 512;
/// Slots made at each submission whatever time they take.
const URGENT: usize = 2048;

impl Bindless {
    /// The slot-making budget, refilled for the time passed since.
    fn budget(&mut self) -> &mut [u64; 2] {
        let now = std::time::Instant::now();
        let (at, left) = self
            .budget
            .get_or_insert((now, [SLOT_BUDGET_US; 2]));
        let add = now.duration_since(*at).as_micros() as u64 / 8;
        if add > 0 {
            *at = now;
            for l in left.iter_mut() {
                *l = (*l + add).min(SLOT_BUDGET_US);
            }
        }
        left
    }
}

/// Slots waiting to be made: at submission, at recording (for the log).
pub(crate) static BACKLOG: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// What one bindless slot needs at submission.
#[derive(Default)]
struct SlotUse {
    /// Title memory it reads (host copy, address, bytes).
    read: Option<(Arc<crate::shadow::Shadow>, usize, usize)>,
    /// What shaders may write through it.
    write: Option<(Arc<crate::shadow::Shadow>, usize, usize)>,
    /// A misaligned constant buffer: bounce slice, title address, bytes.
    bounce: Option<(usize, u64, u32)>,
    /// The host resource it views.
    retained: Option<ID3D12Resource>,
    /// The title memory it views in place.
    direct: Option<Arc<crate::shadow::Shadow>>,
    /// It views a placed texture: the texture's object (0 for none), made
    /// again when the texture is evicted.
    placed: Option<usize>,
    /// It views unmapped memory (looked at again when memory is remapped),
    /// this of it.
    unmapped: bool,
    at: (u64, u64),
}

/// Placed textures reached only through bindless slots are uploaded when a
/// slot first reaches them, and then looked at again (their memory hashed)
/// at most this often, in frames, ...
///
/// shortcut: a texture the title rewrites in place without writing a slot
/// shows its new contents up to this many frames late through bindless
/// access (tables still upload what their draws read). Hashing all of them
/// at each submission cost seconds per frame while the world streams in.
const RECHECK_FRAMES: u64 = 15;
/// ... and at most this many of them per frame.
const RECHECKS: u32 = 32;

/// Bumped when views go stale (a resource was released): their views'
/// generations are checked, all at once, at the next submission.
static VIEWED: AtomicU64 = AtomicU64::new(0);

/// Bumped when title memory is mapped or unmapped: only unmapped and direct
/// bindless slots need checking (no full `stale_views` scan over all slots).
static MEMORY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Title memory mapped or unmapped lately (start, end), and how many were.
static MAPPED: Mutex<(VecDeque<(u64, u64)>, u64)> = Mutex::new((VecDeque::new(), 0));
/// Of them, those kept; a heap that missed some looks at all its slots.
const MAPPED_KEPT: usize = 1 << 16;

/// Title memory `[start, end)` was mapped or unmapped.
pub(crate) fn bindless_memory_changed(start: u64, end: u64) {
    {
        let mut m = MAPPED.lock().unwrap_or_else(|e| e.into_inner());
        if m.0.len() == MAPPED_KEPT {
            m.0.pop_front();
        }
        m.0.push_back((start, end));
        m.1 += 1;
    }
    MEMORY_EPOCH.fetch_add(1, Ordering::Release);
}

/// The ranges mapped or unmapped since the first `seen`, sorted and merged
/// (`None` if they are not all known), and how many there were.
fn mapped_since(seen: u64) -> (Option<Vec<(u64, u64)>>, u64) {
    let m = MAPPED.lock().unwrap_or_else(|e| e.into_inner());
    let n = (m.1 - seen) as usize;
    if n > m.0.len() {
        return (None, m.1);
    }
    let mut ranges: Vec<(u64, u64)> = m.0.iter().skip(m.0.len() - n).copied().collect();
    let count = m.1;
    drop(m);
    ranges.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (a, b) in ranges {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    (Some(out), count)
}

/// Whether `[start, end)` meets one of `ranges` (sorted, merged).
fn meets(ranges: &[(u64, u64)], (start, end): (u64, u64)) -> bool {
    let i = ranges.partition_point(|r| r.1 <= start);
    ranges.get(i).is_some_and(|r| r.0 < end)
}

/// Bindless slots must be checked again: a resource their views name was
/// released, or title memory was mapped or unmapped.
pub(crate) fn bindless_viewed_changed() {
    VIEWED.fetch_add(1, Ordering::Release);
}

/// Bindless unordered access ranges larger than this are not copied back.
///
/// shortcut: copy-back finds what shaders wrote by keeping a range's bytes
/// before each submission; a bindless slot viewing a title's big shared heap
/// would cost a copy and a compare of all of it per submission whether or
/// not a shader writes there. Such ranges stay on the GPU's side.
const WRITE_CAP: usize = 8 << 20;

/// Gap below which two ranges of one host copy are refreshed as one: the
/// write-watch answers by 64 KiB page, and fewer, larger askings cost less.
const READ_GAP: usize = 64 << 10;

/// What a submission does for a heap's bindless slots, made again only when
/// the slots or what they view change ([`HostHeap::bindless_plan`]).
pub(crate) struct BindlessPlan {
    /// Title memory the slots read, merged per host copy (address, bytes):
    /// refreshed at each submission using the heap.
    pub(crate) reads: Arc<Vec<(Arc<crate::shadow::Shadow>, usize, usize)>>,
    /// The unordered access slots' ranges (up to [`WRITE_CAP`] each), merged:
    /// copied back after a submission whose shaders may write memory.
    pub(crate) writes: Arc<Vec<(Arc<crate::shadow::Shadow>, usize, usize)>>,
    /// Misaligned constant buffers: bounce slice, title address, bytes.
    bounces: Arc<Vec<(usize, u64, u32)>>,
    /// Of them, those new in this plan.
    fresh: Vec<(usize, u64, u32)>,
    fresh_reads: Vec<(Arc<crate::shadow::Shadow>, usize, usize)>,
    /// The frame (plus one) the bounces were last copied in.
    copied_in: AtomicU64,
    /// The host resources the slots view, held while a submission runs.
    _retained: Arc<Vec<ID3D12Resource>>,
    /// Those of slots made since `_retained` was listed.
    _extra: Vec<ID3D12Resource>,
}

// SAFETY: D3D12 resources are free-threaded; the rest is plain data.
unsafe impl Send for BindlessPlan {}
// SAFETY: as above.
unsafe impl Sync for BindlessPlan {}

impl BindlessPlan {
    /// Copies the misaligned constant buffers into their bounce slices.
    ///
    /// shortcut: all of them once a frame, else only for a plan not copied
    /// yet: the title writes constants before it points a slot at them.
    /// Returns the title memory to bring up to date, by the same rule.
    /// And whether it is all of it (and may be looked at less often where it
    /// does not change) rather than what is new.
    pub(crate) fn copy_constants(&self) -> (&[(Arc<crate::shadow::Shadow>, usize, usize)], bool) {
        let frame = crate::queue::presented() + 1;
        // Counted for all plans: each is the one before with a few changes.
        static FRAME: AtomicU64 = AtomicU64::new(0);
        let all = FRAME.swap(frame, Ordering::AcqRel) != frame;
        let fresh = self.copied_in.swap(frame, Ordering::AcqRel) == 0;
        let (bounces, reads) = match (all || cfg!(test), fresh) {
            (true, _) => (&*self.bounces, &*self.reads),
            (_, true) => (&self.fresh, &self.fresh_reads),
            _ => return (&[], false),
        };
        if !bounces.is_empty() {
            let started = std::time::Instant::now();
            let bytes: u64 = bounces.iter().map(|b| u64::from(b.2)).sum();
            #[cfg(feature = "diagnostics")]
            {
                use crate::copystat::{C, add};
                add(C::Copies, bounces.len() as u64);
                add(C::CopyBytes, bytes);
            }
            crate::host::in_parallel(bounces, 1024, copy_bounces);
            let us = started.elapsed().as_micros() as u64;
            crate::host::PREPARING[0].fetch_add(us, Ordering::Relaxed);
            crate::host::PREPARING[1].fetch_add(bytes, Ordering::Relaxed);
        }
        (reads, all)
    }
}

/// Copies constant buffers (destination, title address, bytes), those near
/// each other in title memory (as sorted lists have them) read at once.
fn copy_bounces(list: &[(usize, u64, u32)]) {
    const GAP: u64 = 4 << 10;
    const RUN: u64 = 128 << 10;
    let mut scratch = vec![0u8; RUN as usize];
    let mut rest = list;
    while let Some(&(_, first, len)) = rest.first() {
        let mut end = first + u64::from(len);
        let mut n = 1;
        for &(_, a, l) in &rest[1..] {
            let to = end.max(a + u64::from(l));
            if a < first || a > end + GAP || to - first > RUN {
                break;
            }
            end = to;
            n += 1;
        }
        let (run, after) = rest.split_at(n);
        rest = after;
        let whole = (end - first) as usize;
        let read = n > 1
            && whole <= scratch.len()
            && crate::host::read_into(first as *const u8, &mut scratch[..whole]);
        for &(cpu, a, l) in run {
            if read {
                let from = &scratch[(a - first) as usize..][..l as usize];
                // SAFETY: a bounce slice of at least `l` bytes, ours.
                unsafe {
                    std::ptr::copy_nonoverlapping(from.as_ptr(), cpu as *mut u8, l as usize);
                }
            } else {
                crate::host::copy_mapped(cpu as *mut u8, a as *const u8, l as usize, false);
            }
        }
    }
}

/// `ranges` (host copy, address, bytes) sorted and merged per host copy
/// where they overlap or lie within `gap` of each other.
fn merged(
    mut ranges: Vec<(Arc<crate::shadow::Shadow>, usize, usize)>,
    gap: usize,
) -> Vec<(Arc<crate::shadow::Shadow>, usize, usize)> {
    ranges.sort_unstable_by_key(|(s, a, _)| (Arc::as_ptr(s) as usize, *a));
    let mut out: Vec<(Arc<crate::shadow::Shadow>, usize, usize)> = Vec::new();
    for (shadow, address, len) in ranges {
        if let Some((s, a, l)) = out.last_mut()
            && Arc::ptr_eq(s, &shadow)
            && address <= *a + *l + gap
        {
            *l = (*l).max(address + len - *a);
            continue;
        }
        out.push((shadow, address, len));
    }
    out
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
            format!(
                "{:#x} past its end (slot {})",
                va - start - bytes,
                (va - start) / step
            )
        };
        text.push_str(&format!("; {kind} heap {start:#x}+{bytes:#x}: {place}"));
    }
    text
}

/// The host's limit for shader-visible view heaps on resource binding
/// tiers 1 and 2; tier 3 hardware may take more.
const PORTABLE_VIEWS: u32 = 1_000_000;

/// Slots past the title's in a view heap, for tables a draw needs made
/// afresh (constant buffers copied per draw). A list holds the slots it
/// took until it is reset (its submissions have run) or released. A world
/// frame of some 7,000 draws held over 400,000 of 2^19, and tables of 150
/// slots no longer fit: the draws that needed them were dropped (the
/// subsurface resolve, tree imposters). `victory_shader::emit::VIEW_SLOTS`
/// is the title's 1,245,184 slots, these and those of [`crate::memory`].
const DYNAMIC: u32 = 1 << 20;
/// Smaller dynamic reserve when the host caps the whole view heap at
/// [`PORTABLE_VIEWS`].
const DYNAMIC_CAPPED: u32 = 1 << 16;

/// The host's limit for shader-visible sampler heaps, on every binding
/// tier.
const SAMPLERS: u32 = D3D12_MAX_SHADER_VISIBLE_SAMPLER_HEAP_SIZE;

/// The slots to make of a view heap of `count` title and dynamic slots: with
/// those of [`crate::memory`] after them, and enough that shaders' indices,
/// which stay below [`victory_shader::emit::VIEW_SLOTS`], are inside it.
fn view_slots(count: u32) -> u32 {
    let slots = count + crate::memory::SLOTS;
    if cfg!(test) { slots } else { slots.max(victory_shader::emit::VIEW_SLOTS) }
}

/// Dynamic slots of a sampler heap capped at [`SAMPLERS`], for tables
/// reaching past the title slots it keeps.
const SAMPLER_DYNAMIC: u32 = 256;

/// Diagnostics of [`HostHeap::bindless_plan`]: per part (the direct slots'
/// check, the list of all slots, the look for stale views, the slots made,
/// their results applied, the whole plan built again), the microseconds,
/// the times it ran, the items it walked and those it found, summed and
/// logged every 64 presented frames.
static PLAN_NOTES: [[AtomicU64; 4]; 9] = [const { [const { AtomicU64::new(0) }; 4] }; 9];
static PLAN_FRAME: AtomicU64 = AtomicU64::new(0);
/// Slots the plan lacked something for, by what: bit 0 another write, bit 1
/// another bounce, bit 2 a read outside the plan.
static LACKS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// Frames with such a slot, and with one lacking more than a bounce: the
/// last frame counted, the count.
static LACK_FRAMES: [[AtomicU64; 2]; 2] = [const { [const { AtomicU64::new(0) }; 2] }; 2];

/// Whether the plan's unchanged part is kept from frame to frame
/// (`VICTORY_PLAN_REUSE=0`: as before, for comparison).
fn reuse() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PLAN_REUSE").is_none_or(|v| v != "0"))
}

/// A plan built some frames ago may read memory no slot views any more:
/// for at most this many frames.
///
/// shortcut: the plan is built again at once only for a slot that brings
/// what it lacks (memory to read outside its ranges, another range to
/// write, another bounce). One that only stops reading leaves its range in
/// the plan, refreshed for nothing, until the next build: of 70,000 slots
/// some hundred come out otherwise each frame, and building the plan for
/// them took 10 to 14 ms a frame (run fps1).
const LOOSE_FRAMES: u64 = 8;

type Range = Option<(Arc<crate::shadow::Shadow>, usize, usize)>;

fn same_range(x: &Range, y: &Range) -> bool {
    match (x, y) {
        (None, None) => true,
        (Some((s, a, l)), Some((t, b, m))) => Arc::ptr_eq(s, t) && (a, l) == (b, m),
        _ => false,
    }
}

/// Whether `range` lies in one of `merged` (as [`merged`] leaves them).
fn covered(merged: &[(Arc<crate::shadow::Shadow>, usize, usize)], range: &Range) -> bool {
    let Some((shadow, address, len)) = range else { return true };
    let key = (Arc::as_ptr(shadow) as usize, *address);
    let after = merged.partition_point(|(s, a, _)| (Arc::as_ptr(s) as usize, *a) <= key);
    after > 0 && {
        let (s, a, l) = &merged[after - 1];
        Arc::ptr_eq(s, shadow) && address + len <= a + l
    }
}

/// Whether two uses of a slot put the same things in a plan.
fn same_ranges(a: &SlotUse, b: &SlotUse) -> bool {
    use windows::core::Interface;
    let same = same_range;
    same(&a.read, &b.read)
        && same(&a.write, &b.write)
        && a.bounce == b.bounce
        && a.retained.as_ref().map(Interface::as_raw) == b.retained.as_ref().map(Interface::as_raw)
}

fn plan_note(part: usize, started: std::time::Instant, walked: usize, found: usize) {
    let n = &PLAN_NOTES[part];
    n[0].fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
    n[1].fetch_add(1, Ordering::Relaxed);
    n[2].fetch_add(walked as u64, Ordering::Relaxed);
    n[3].fetch_add(found as u64, Ordering::Relaxed);
    let frame = crate::queue::presented();
    let last = PLAN_FRAME.load(Ordering::Relaxed);
    if frame >= last + 64
        && PLAN_FRAME.compare_exchange(last, frame, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    {
        let frames = (frame - last).max(1) as f64;
        const NAMES: [&str; 9] = ["direct slots", "all slots listed", "stale views", "slots made", "results applied", "whole plan", "differing (found: lacking)", "lacking a write (found: a bounce)", "lacking a read (found: gone with a write or bounce)"];
        let mut text = String::new();
        for (name, n) in NAMES.iter().zip(&PLAN_NOTES) {
            let [us, runs, walked, found] = [0, 1, 2, 3].map(|k| n[k].swap(0, Ordering::Relaxed));
            text.push_str(&format!(
                "; {name}: {:.2} ms, {:.1} runs, {:.0} walked, {:.1} found",
                us as f64 / 1e3 / frames,
                runs as f64 / frames,
                walked as f64 / frames,
                found as f64 / frames
            ));
        }
        let lacks = LACKS.each_ref().map(|n| n.swap(0, Ordering::Relaxed) as f64 / frames);
        let [any, more] = LACK_FRAMES.each_ref().map(|n| n[1].swap(0, Ordering::Relaxed));
        text.push_str(&format!(
            "; slots lacking per frame, by write+bounce*2+read*4: {:.1?}; of {frames:.0} frames,              {any} had a lacking slot and {more} one lacking more than a bounce",
            &lacks[1..]
        ));
        tracing::info!("bindless plan per frame{text}");
    }
}

impl HostHeap {
    pub(crate) fn new(sampler: bool, count: u32) -> Option<Self> {
        let host = host()?;
        let kind = if sampler {
            D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER
        } else {
            D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
        };
        let create = |count: u32| -> windows::core::Result<ID3D12DescriptorHeap> {
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
        let make = |count: u32| {
            if sampler {
                return create(count);
            }
            // A host with room for fewer slots gets no padding.
            let slots = view_slots(count);
            create(slots).or_else(|_| create(count + crate::memory::SLOTS))
        };
        if sampler && count > SAMPLERS {
            // shortcut: samplers past the host's limit are left out; tables
            // reaching them are made afresh in the dynamic slots.
            tracing::warn!(count, "host sampler heap capped at {SAMPLERS}");
            let heap = match make(SAMPLERS) {
                Ok(heap) => heap,
                Err(e) => {
                    tracing::warn!(error = %e, count, sampler, "no host descriptor heap");
                    return None;
                }
            };
            return Some(Self::with(
                host,
                heap,
                kind,
                SAMPLERS - SAMPLER_DYNAMIC,
                SAMPLER_DYNAMIC,
            ));
        }
        let extra = if sampler { 0 } else { DYNAMIC };
        let wanted = count;
        let (heap, count, extra) = match make(count + extra) {
            Ok(heap) => (heap, count, extra),
            Err(e) if !sampler && count + extra + crate::memory::SLOTS > PORTABLE_VIEWS => {
                // shortcut: descriptors past the host's limit are left out;
                // draw::tables now remaps the root tables that reach them.
                tracing::warn!(
                    error = %e,
                    wanted,
                    made = PORTABLE_VIEWS - DYNAMIC_CAPPED,
                    lost = wanted + DYNAMIC_CAPPED - PORTABLE_VIEWS,
                    "host view heap capped at {PORTABLE_VIEWS}"
                );
                (
                    make(PORTABLE_VIEWS - crate::memory::SLOTS).ok()?,
                    PORTABLE_VIEWS - DYNAMIC_CAPPED - crate::memory::SLOTS,
                    DYNAMIC_CAPPED,
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, count, sampler, "no host descriptor heap");
                return None;
            }
        };
        Some(Self::with(host, heap, kind, count, extra))
    }

    /// The host heap over `heap`: `count` title slots, then `extra` dynamic
    /// ones, all made null.
    fn with(
        host: &Host,
        heap: ID3D12DescriptorHeap,
        kind: D3D12_DESCRIPTOR_HEAP_TYPE,
        count: u32,
        extra: u32,
    ) -> Self {
        let sampler = kind == D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER;
        // SAFETY: a live heap.
        let (cpu, step) = unsafe {
            (
                heap.GetCPUDescriptorHandleForHeapStart().ptr,
                host.device.GetDescriptorHandleIncrementSize(kind),
            )
        };
        tracing::info!(count, extra, sampler, "host descriptor heap");
        // SAFETY: a live shader-visible heap.
        let gpu = unsafe { heap.GetGPUDescriptorHandleForHeapStart().ptr };
        GPU_RANGES.lock().unwrap_or_else(|e| e.into_inner()).push((
            gpu,
            u64::from(count + extra) * u64::from(step),
            u64::from(step),
            sampler,
        ));
        let strides = if sampler {
            None
        } else {
            StrideTable::new(&host.device, count + extra)
        };
        let h = HostHeap {
            heap,
            cpu,
            step,
            count,
            spare: extra,
            sampler,
            made: Mutex::new(vec![0; count as usize]),
            kept: Mutex::new(vec![None; (count + extra) as usize]),
            kept_textures: Mutex::default(),
            next: Mutex::new((0, vec![false; extra as usize], usize::MAX)),
            strides,
            bindless: Mutex::default(),
            recording: std::sync::atomic::AtomicBool::new(false),
            record_hint: std::sync::atomic::AtomicBool::new(false),
            record_looked: [AtomicU64::new(0), AtomicU64::new(0)],
            recheck_at: AtomicU64::new(u64::MAX),
            rewrites_seen: AtomicU64::new(0),
            memory_seen: Mutex::new(0),
        };
        // A slot never written is undefined: shaders reading one (a slot
        // the title wrote in a way we do not know, or a table entry no
        // used-resource entry names) read a wild address. Every slot starts
        // null instead.
        let started = std::time::Instant::now();
        // SAFETY: a plain call on a live heap.
        let total = unsafe { h.heap.GetDesc() }.NumDescriptors;
        if !sampler && total < victory_shader::emit::VIEW_SLOTS {
            tracing::warn!(total, "a view heap shorter than shaders keep their indices in");
        }
        if let Err(e) = h.fill_null(kind, total) {
            tracing::warn!(error = %e, "host descriptor heap not made null");
        }
        tracing::debug!(ms = started.elapsed().as_millis(), "host descriptor heap made null");
        h
    }

    /// The first slot of memory by address, its views made.
    pub(crate) fn memory(&self) -> u32 {
        let first = self.count + self.spare;
        if let Some(host) = host() {
            let mut seen = self.memory_seen.lock().unwrap_or_else(|e| e.into_inner());
            *seen = crate::memory::fill(&host.device, *seen, |k| self.cpu(first + k));
        }
        first
    }

    /// Makes the first `total` slots null: one null slot in a CPU-side heap,
    /// doubled by copies to a block, the block copied over the heap.
    fn fill_null(&self, kind: D3D12_DESCRIPTOR_HEAP_TYPE, total: u32) -> windows::core::Result<()> {
        const BLOCK: u32 = 1 << 16;
        let Some(host) = host() else { return Ok(()) };
        // SAFETY: plain heap creation and copies inside live heaps.
        unsafe {
            let block: ID3D12DescriptorHeap =
                host.device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
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
                host.device
                    .CopyDescriptorsSimple(n, at(filled), start, kind);
                filled += n;
            }
            let mut done = 0;
            while done < total {
                let n = BLOCK.min(total - done);
                host.device
                    .CopyDescriptorsSimple(n, self.cpu(done), start, kind);
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
    /// The microseconds of slot making (`which`: 0 plans, 1 recording) left.
    fn budget_left(&self, which: usize) -> u64 {
        self.bindless
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .budget()[which]
    }

    /// Notes `us` microseconds of slot making spent.
    fn budget_spend(&self, which: usize, us: u64) {
        let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
        let left = &mut b.budget()[which];
        *left = left.saturating_sub(us);
    }

    /// `n` consecutive free dynamic slots (view heaps and capped sampler
    /// heaps), taken until [`HostHeap::release`]: the first's index.
    pub(crate) fn dynamic(&self, n: u32) -> Option<u32> {
        if n == 0 || self.spare == 0 {
            return None;
        }
        if n > self.spare {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                tracing::warn!(
                    n,
                    spare = self.spare,
                    sampler = self.sampler,
                    "a table larger than the dynamic slots is not made"
                );
            });
            return None;
        }
        let spare = self.spare as usize;
        let mut guard = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let (next, taken, no_run) = &mut *guard;
        let n = n as usize;
        // A search that found no run of this size (or of a smaller one)
        // since the last release finds none now: without this, each table
        // of a frame that does not fit walked all the slots again, and a
        // heavy place took 7 to 11 seconds a frame (runs sw1, sw2).
        if n >= *no_run {
            return None;
        }
        // Releases come all the time and each allows a search again: no
        // more than a few failed walks a frame.
        static FAILED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
        const WALKS: u64 = 8;
        let frame = crate::queue::presented();
        if FAILED[0].swap(frame, Ordering::Relaxed) != frame {
            FAILED[1].store(0, Ordering::Relaxed);
        }
        if FAILED[1].load(Ordering::Relaxed) >= WALKS {
            return None;
        }
        // Search from `next` on, then from the start.
        let starts = (*next as usize..spare).chain(0..*next as usize);
        let mut run = 0;
        let mut found = None;
        let mut last = usize::MAX;
        for at in starts {
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
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!(n, spare, "no free dynamic descriptor slots: a table is not made"));
            *no_run = n;
            FAILED[1].fetch_add(1, Ordering::Relaxed);
            return None;
        };
        taken[first..first + n].fill(true);
        #[cfg(feature = "diagnostics")]
        if !self.sampler {
            static MADE: AtomicU64 = AtomicU64::new(0);
            let before = MADE.fetch_add(n as u64, Ordering::Relaxed);
            if before >> 20 != (before + n as u64) >> 20 {
                let used = taken.iter().filter(|t| **t).count();
                tracing::info!(used, spare, "dynamic descriptor slots");
            }
        }
        *next = ((first + n) % spare) as u32;
        Some(self.count + first as u32)
    }

    /// The title slots this heap has host slots for; fewer than the title's
    /// when the host caps its size.
    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    /// Gives back `n` dynamic slots from `first` (an index [`dynamic`]
    /// returned).
    ///
    /// [`dynamic`]: HostHeap::dynamic
    pub(crate) fn release(&self, first: u32, n: u32) {
        let Some(at) = first.checked_sub(self.count) else {
            return;
        };
        let mut guard = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let end = at.saturating_add(n).min(self.spare) as usize;
        if let Some(slots) = guard.1.get_mut(at as usize..end) {
            slots.fill(false);
        }
        guard.2 = usize::MAX;
    }

    /// Makes slot `index` (a dynamic one) from `view`; a null view if the
    /// host cannot.
    pub(crate) fn make_at(&self, index: u32, view: &View) {
        let kept = make(view, self.cpu(index));
        let ok = kept.is_some();
        if !ok {
            if self.sampler {
                self.null_sampler(index);
            } else {
                null_for(view, self.cpu(index));
            }
        }
        self.keep_with_view(index, kept.flatten(), view);
        if ok {
            self.set_stride_count(index, view_stride(view), view_count(view));
        } else {
            self.set_stride(index, 0);
        }
    }

    /// Makes slot `index` read nothing: a null view, or a plain sampler.
    pub(crate) fn make_null(&self, index: u32) {
        if self.sampler {
            self.null_sampler(index);
        } else {
            null_view(self.cpu(index));
        }
        self.keep(index, None);
        self.set_stride(index, 0);
    }

    /// Makes slot `index` a null view matching the resource kind a shader
    /// expects there (CBV, buffer SRV/UAV, or 2D texture).
    pub(crate) fn make_null_kind(&self, index: u32, kind: victory_shader::ResourceType, dwords: u8) {
        if self.sampler {
            self.null_sampler(index);
            self.keep(index, None);
            return;
        }
        self.set_stride(index, 0);
        let at = self.cpu(index);
        match kind {
            victory_shader::ResourceType::Cbv => {
                if let Some(host) = host() {
                    let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC::default();
                    // SAFETY: a slot of our heap; zero location makes a null CBV.
                    unsafe { host.device.CreateConstantBufferView(Some(&desc), at) };
                }
            }
            victory_shader::ResourceType::Srv if dwords == 4 => null_buffer_view(at),
            victory_shader::ResourceType::Uav => {
                if let Some(host) = host() {
                    let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                        Format: DXGI_FORMAT_R32_UINT,
                        ViewDimension: if dwords == 4 {
                            D3D12_UAV_DIMENSION_BUFFER
                        } else {
                            D3D12_UAV_DIMENSION_TEXTURE2D
                        },
                        ..Default::default()
                    };
                    // SAFETY: a slot of our heap; null resource makes a null UAV.
                    unsafe {
                        host.device.CreateUnorderedAccessView(
                            None::<&ID3D12Resource>,
                            None::<&ID3D12Resource>,
                            Some(&desc),
                            at,
                        )
                    };
                }
            }
            _ => null_view(at),
        }
        self.keep(index, None);
    }

    /// Slot `index` now views `resource` (or nothing of the host's). A
    /// previously viewed resource is retired rather than dropped inline so an
    /// in-flight GPU submission reading that slot cannot fault on freed memory.
    fn keep(&self, index: u32, resource: Option<ID3D12Resource>) {
        self.keep_owner(index, resource, None);
    }

    fn keep_with_view(&self, index: u32, resource: Option<ID3D12Resource>, view: &View) {
        let tex = if resource.is_some() {
            match view {
                View::Shader(r, _) | View::Unordered(r, _, _) => {
                    crate::resource::host_copy(*r as crate::object::This)
                }
                _ => None,
            }
        } else {
            None
        };
        self.keep_owner(index, resource, tex);
    }

    fn keep_owner(
        &self,
        index: u32,
        resource: Option<ID3D12Resource>,
        texture: Option<Arc<crate::resource::HostTexture>>,
    ) {
        // A running submission may still read the old resource through
        // this slot; it is retired outside the lock.
        let old = {
            let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
            kept.get_mut(index as usize)
                .and_then(|slot| std::mem::replace(slot, resource))
        };
        let old_tex = {
            let mut texs = self.kept_textures.lock().unwrap_or_else(|e| e.into_inner());
            match texture {
                Some(t) => texs.insert(index, t),
                None => texs.remove(&index),
            }
        };
        if let Some(old) = old {
            crate::host::retire(old);
        }
        if let Some(old_tex) = old_tex {
            crate::host::retire(old_tex);
        }
    }

    fn set_stride(&self, index: u32, stride: u32) {
        self.set_stride_count(index, stride, 0);
    }

    /// The stride and the element count of the buffer view in slot `index`
    /// (zeros for anything else), for shaders: a size query of a structured
    /// buffer of some strides crashes the AMD driver's compiler.
    fn set_stride_count(&self, index: u32, stride: u32, count: u32) {
        if let Some(t) = &self.strides
            && index < t.len
        {
            // SAFETY: `t.ptr` points to a live mapped UPLOAD buffer of
            // `COUNTS_AT + t.len` dwords.
            unsafe {
                *(t.ptr as *mut u32).add(index as usize) = stride;
                *(t.ptr as *mut u32).add((COUNTS_AT + index) as usize) = count;
            }
        }
    }

    /// GPU virtual address of the per-slot element byte stride table.
    pub(crate) fn stride_table_gpu(&self) -> Option<u64> {
        self.strides.as_ref().map(|t| t.gpu)
    }

    /// Makes slot `index` (a dynamic one) a constant buffer view of host
    /// memory (the list's own, which it holds).
    pub(crate) fn constant_at(&self, index: u32, gpu: u64, size: u32) {
        self.keep(index, None);
        self.set_stride(index, 0);
        let Some(host) = host() else { return };
        let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
            BufferLocation: gpu,
            SizeInBytes: size.min(65_536).next_multiple_of(256),
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

    /// Makes host slot `index` from the record the title has at `handle`
    /// if it changed since, and returns the view recorded there. Buffer
    /// views are made only for draws (`for_draw`), which also copy their
    /// data: a host copy of a buffer is made on first use.
    pub(crate) fn mirror(&self, index: u32, handle: u64, for_draw: bool) -> Option<View> {
        if index >= self.count {
            // Diagnostic (NVIDIA heap cap): the highest title slot past the
            // host heap seen so far.
            static HIGHEST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if HIGHEST.fetch_max(index, Ordering::Relaxed) < index {
                tracing::warn!(
                    index,
                    count = self.count,
                    for_draw,
                    "title slot past the host heap"
                );
            }
            return None;
        }
        // SAFETY: the handle is a descriptor slot in title memory.
        let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
        // Generations start at 1, so a made slot's key is never 0.
        let key = u64::from(generation) << 32 | u64::from(i);
        let view = (tag == VIEW_TAG).then(|| view_at(i, generation)).flatten();
        let Some(view) = view else {
            // Not a view of ours (never written, or of a resource the
            // title released): nothing to read.
            {
                let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
                if made[index as usize] != 0 {
                    made[index as usize] = 0;
                    self.make_null(index);
                }
            }
            if !self.sampler {
                self.untrack_bindless(index);
            }
            return None;
        };
        let is_buf = !self.sampler && view.is_buffer();
        {
            let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
            let gone = is_buf
                && for_draw
                && self
                    .kept
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(index as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(crate::shadow::gone);
            if made[index as usize] != key || gone {
                let later = !for_draw && is_buf;
                made[index as usize] = if later { 0 } else { key };
                let kept = if later {
                    None
                } else {
                    make(&view, self.cpu(index))
                };
                if kept.is_none() && !self.sampler {
                    null_for(&view, self.cpu(index));
                }
                self.keep_with_view(index, kept.flatten(), &view);
                // The stride is the title view's, however and whenever the
                // host view is made.
                self.set_stride_count(index, view_stride(&view), view_count(&view));
            }
        }
        if !self.sampler {
            if is_buf {
                let placed = matches!(&view, View::Placed(false, r, _) if r[0] != 1);
                self.track_bindless(index, (i, generation), placed);
            } else {
                self.untrack_bindless(index);
            }
        }
        Some(view)
    }

    fn track_bindless(&self, index: u32, entry: (u32, u32), placed: bool) {
        let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
        if b.slots.insert(index, entry) != Some(entry) {
            b.dirty.insert(index);
            b.recent.push(index);
            if let Some(bounce) = b.cbv_bounce.get_mut(&index) {
                bounce.active = false;
            }
        }
        let changed = if placed {
            b.placed_set.insert(index, entry) != Some(entry)
        } else {
            b.placed_set.remove(&index).is_some()
        };
        if changed {
            b.record_pending.insert(index);
            self.record_hint.store(true, Ordering::Release);
        }
    }

    fn untrack_bindless(&self, index: u32) {
        let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
        if b.slots.remove(&index).is_some() {
            b.dirty.insert(index);
            b.recent.push(index);
            if let Some(bounce) = b.cbv_bounce.get_mut(&index) {
                bounce.active = false;
            }
        }
        if b.placed_set.remove(&index).is_some() {
            b.recorded.remove(&index);
        }
    }

    /// Slot `index` (viewing `entry`) is no longer one for bindless access.
    fn drop_bindless(&self, b: &mut Bindless, index: u32, entry: Option<(u32, u32)>) {
        b.uses.remove(&index);
        b.busy.remove(&index);
        b.unmapped_slots.remove(&index);
        b.direct_slots.remove(&index);
        b.recorded.remove(&index);
        if entry.is_none_or(|e| b.slots.get(&index) == Some(&e)) {
            b.slots.remove(&index);
            if let Some(bounce) = b.cbv_bounce.get_mut(&index) {
                bounce.active = false;
            }
            b.placed_set.remove(&index);
        }
    }

    fn clear_stale_slot(&self, index: u32) {
        let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
        if made[index as usize] != 0 {
            made[index as usize] = 0;
            self.make_null(index);
        }
    }

    fn set_unmapped_null(&self, index: u32, view: &View) {
        let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
        made[index as usize] = 0;
        if !self.sampler {
            null_for(view, self.cpu(index));
        }
        self.keep(index, None);
    }

    fn ensure_placed_slot(
        &self,
        index: u32,
        key: u64,
        shader_view: &View,
        raw_tex: *mut std::ffi::c_void,
    ) {
        use windows::core::Interface;
        let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
        let same_tex = self
            .kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(index as usize)
            .and_then(Option::as_ref)
            .is_some_and(|r| r.as_raw() == raw_tex);
        if made[index as usize] != key || !same_tex {
            let kept = make(shader_view, self.cpu(index));
            if kept.is_none() {
                null_for(shader_view, self.cpu(index));
                made[index as usize] = 0;
            } else {
                made[index as usize] = key;
            }
            self.keep_with_view(index, kept.flatten(), shader_view);
            self.set_stride(index, 0);
        }
    }

    fn ensure_buffer_slot(&self, index: u32, key: u64, view: &View) -> Option<ID3D12Resource> {
        let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
        if made[index as usize] != key {
            let kept = make(view, self.cpu(index));
            let res = kept.flatten();
            if res.is_none() {
                null_for(view, self.cpu(index));
                made[index as usize] = 0;
            } else {
                made[index as usize] = key;
            }
            self.keep(index, res.clone());
            return res;
        }
        self.kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(index as usize)
            .and_then(Option::clone)
    }

    fn ensure_cbv_bounce(
        &self,
        index: u32,
        aligned: u32,
    ) -> Option<(u64, usize, ID3D12Resource, bool)> {
        let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = b.cbv_bounce.get_mut(&index)
            && slot.cap >= aligned
        {
            let was_active = std::mem::replace(&mut slot.active, true);
            return Some((slot.gpu, slot.cpu, slot.resource.clone(), was_active));
        }
        let needed = u64::from(aligned);
        let need_chunk = b
            .bounce_chunks
            .last()
            .is_none_or(|&(_, _, _, used)| used + needed > BOUNCE_CHUNK);
        if need_chunk {
            let chunk_size = BOUNCE_CHUNK.max(needed);
            let (res, data) = host()?.shared_buffer(chunk_size)?;
            crate::host::name(&res, || "bindless cbv bounce".into());
            // SAFETY: a live buffer.
            let gpu = unsafe { res.GetGPUVirtualAddress() };
            b.bounce_chunks.push((res, data as usize, gpu, 0));
        }
        let last = b.bounce_chunks.last_mut()?;
        let at = last.3;
        last.3 += needed;
        let (gpu, cpu, res) = (last.2 + at, last.1 + at as usize, last.0.clone());
        b.cbv_bounce.insert(
            index,
            BounceSlot {
                gpu,
                cpu,
                cap: aligned,
                resource: res.clone(),
                active: true,
            },
        );
        Some((gpu, cpu, res, false))
    }

    fn materialize_bindless_cbv(
        &self,
        index: u32,
        key: u64,
        address: u64,
        capped: u32,
        reads: &mut Vec<(Arc<crate::shadow::Shadow>, usize, usize)>,
        bounces: &mut Vec<(usize, u64, u32)>,
    ) -> Option<ID3D12Resource> {
        let host = host()?;
        let aligned = capped.next_multiple_of(256);
        if address.is_multiple_of(256)
            && let Some(shadow) = crate::shadow::find(address, u64::from(capped))
            && shadow.gpu(address).is_multiple_of(256)
        {
            let bounce_active = self
                .bindless
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cbv_bounce
                .get(&index)
                .is_some_and(|s| s.active);
            {
                let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
                if made[index as usize] != key || bounce_active {
                    let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
                        BufferLocation: shadow.gpu(address),
                        SizeInBytes: aligned,
                    };
                    // SAFETY: a slot of our heap; `shadow` covers the range.
                    unsafe {
                        host.device
                            .CreateConstantBufferView(Some(&desc), self.cpu(index))
                    };
                    made[index as usize] = key;
                    self.keep(index, Some(shadow.buffer.clone()));
                }
            }
            if bounce_active
                && let Some(slot) = self
                    .bindless
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .cbv_bounce
                    .get_mut(&index)
            {
                slot.active = false;
            }
            let len = u64::from(capped).min(shadow.left(address)) as usize;
            if len > 0 {
                reads.push((shadow.clone(), address as usize, len));
            }
            return Some(shadow.buffer.clone());
        }

        let (gpu, cpu, res, was_active) = self.ensure_cbv_bounce(index, aligned)?;
        {
            let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
            if made[index as usize] != key || !was_active {
                let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
                    BufferLocation: gpu,
                    SizeInBytes: aligned,
                };
                // SAFETY: a slot of our heap; `gpu` is 256-byte aligned in `res`.
                unsafe {
                    host.device
                        .CreateConstantBufferView(Some(&desc), self.cpu(index))
                };
                made[index as usize] = key;
                self.keep(index, Some(res.clone()));
            }
        }
        bounces.push((cpu, address, capped));
        Some(res)
    }

    /// At draw/dispatch recording time: makes the placed-texture slots
    /// changed since (or whose textures were evicted), and records on `list`
    /// the upload of the textures they newly reach and of a few known ones
    /// due a look ([`RECHECK_FRAMES`]), each moved to the shader resource
    /// state. Buffer and constant-buffer views are materialized and refreshed
    /// at submission from [`HostHeap::bindless_plan`].
    ///
    /// shortcut: a texture is moved to the shader resource state in the list
    /// that uploads it and keeps that state after; one another list moves
    /// (a copy into it, say) is read in that state through bindless slots.
    pub(crate) fn prepare_bindless_record(&self, list: &crate::host::HostList) {
        if self.sampler {
            return;
        }
        let epoch = crate::resource::placed_epoch();
        let frame = crate::queue::presented();
        // Most draws find nothing to do: a few atomics, no lock.
        let evictions = self.record_looked[0].load(Ordering::Acquire) != epoch
            && self.record_looked[1].load(Ordering::Acquire) != frame + 1;
        let rewrites = self.rewrites_seen.load(Ordering::Acquire) != crate::resource::rewrites();
        if !evictions
            && !rewrites
            && !self.record_hint.load(Ordering::Acquire)
            && self.recheck_at.load(Ordering::Acquire) > frame
        {
            return;
        }
        // One recording thread prepares at a time; the others go on.
        if self.recording.swap(true, Ordering::Acquire) {
            return;
        }
        struct Done<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _done = Done(&self.recording);
        let started = std::time::Instant::now();
        // Asked before taking the heap's lock: the placed textures' lock is
        // taken first where both are held.
        let live = evictions.then(crate::resource::placed_live);
        let (todo, due) = {
            // The submission may hold the heap's lock for a while: the work
            // waits for a later draw rather than this one waiting.
            let mut b = match self.bindless.try_lock() {
                Ok(b) => b,
                Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return,
            };
            let b = &mut *b;
            if let Some(live) = &live {
                b.uploaded.retain(|t, _| live.contains(t));
                for (&slot, &(_, t)) in &b.recorded {
                    if !live.contains(&t) {
                        b.record_pending.insert(slot);
                    }
                }
                b.recorded.retain(|_, (_, t)| live.contains(t));
                self.record_looked[0].store(epoch, Ordering::Release);
                self.record_looked[1].store(frame + 1, Ordering::Release);
            }
            let mut todo: Vec<(u32, Option<(u32, u32)>)> = Vec::new();
            if b.budget()[1] > 0 {
                while todo.len() < SLOT_CHUNK
                    && let Some(i) = b.record_pending.pop_first()
                {
                    todo.push((i, b.placed_set.get(&i).copied()));
                }
            }
            let mut due = Vec::new();
            if rewrites {
                let (list, now) =
                    crate::resource::rewritten_since(self.rewrites_seen.load(Ordering::Acquire));
                self.rewrites_seen.store(now, Ordering::Release);
                let list: std::collections::HashSet<usize> = list.into_iter().collect();
                for t in list {
                    if b.uploaded.remove(&t).is_some() {
                        due.push(t as crate::object::This);
                    }
                }
            }
            let always = crate::resource::textures() == crate::resource::Textures::Always;
            let rechecks = due.len() + RECHECKS as usize;
            while (due.len() < rechecks && (b.rechecks.0 != frame || b.rechecks.1 < RECHECKS)
                || always)
                && let Some(&(f, t)) = b.recheck.front()
                && f + if always { 1 } else { RECHECK_FRAMES } <= frame
            {
                b.recheck.pop_front();
                due.push(t as crate::object::This);
            }
            self.recheck_at.store(
                b.recheck.front().map_or(u64::MAX, |&(f, _)| f + RECHECK_FRAMES),
                Ordering::Release,
            );
            self.record_hint
                .store(!b.record_pending.is_empty(), Ordering::Release);
            (todo, due)
        };
        if todo.is_empty() && due.is_empty() {
            return;
        }
        let mut stale = Vec::new();
        let mut learnt = Vec::new();
        let mut reached = Vec::new();
        let left = self.budget_left(1);
        let mut todo = todo.into_iter();
        let mut over = Vec::new();
        for (index, entry) in todo.by_ref() {
            let Some((vi, generation)) = entry else {
                continue;
            };
            if started.elapsed().as_micros() as u64 > left {
                // Over budget: the rest waits (this one too).
                over.push(index);
                break;
            }
            match self.resolve_placed(index, vi, generation) {
                Ok(Some(t)) => {
                    let key = u64::from(generation) << 32 | u64::from(vi);
                    learnt.push((index, (key, t as usize)));
                    reached.push(t);
                }
                Ok(None) => {}
                Err(()) => stale.push(index),
            }
        }
        over.extend(todo.map(|(i, _)| i));
        reached.extend(due);
        for texture in reached {
            // The texture may be evicted meanwhile: a reference keeps it.
            let Some(_held) = crate::resource::hold_placed(texture) else {
                continue;
            };
            let Some(host_tex) = crate::resource::host_resource(texture) else {
                continue;
            };
            if crate::texture::title_written(host_tex) && self.upload_due(texture, host_tex, frame)
            {
                crate::texture::upload_first(texture, host_tex);
                crate::texture::upload(list, texture, host_tex);
            }
            const SHADER_RESOURCE: u32 = 0x40 | 0x80;
            crate::list::ensure_state(list, texture, SHADER_RESOURCE);
        }
        {
            let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
            b.recorded.extend(learnt);
            for idx in stale {
                self.drop_bindless(&mut b, idx, None);
            }
            b.record_pending.extend(over);
            self.record_hint
                .store(!b.record_pending.is_empty(), Ordering::Release);
        }
        let us = started.elapsed().as_micros() as u64;
        self.budget_spend(1, us);
        crate::host::BINDLESS_US[1].fetch_add(us, Ordering::Relaxed);
    }

    /// The placed texture slot `index` views, made in the slot: `Err` if the
    /// view went stale, `None` if it is not a placed texture or cannot be made
    /// (the slot is then null).
    fn resolve_placed(
        &self,
        index: u32,
        vi: u32,
        generation: u32,
    ) -> Result<Option<crate::object::This>, ()> {
        use windows::core::Interface;
        let Some(view) = view_at(vi, generation) else {
            self.clear_stale_slot(index);
            return Err(());
        };
        let View::Placed(false, resource, desc) = &view else {
            return Ok(None);
        };
        const BUFFER: u32 = 1;
        if resource[0] == BUFFER {
            return Ok(None);
        }
        let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
        if !plausible(address, 1) {
            self.set_unmapped_null(index, &view);
            return Ok(None);
        }
        let Some(shader_view @ View::Shader(tex_res, _)) = placed_texture(&view) else {
            self.set_unmapped_null(index, &view);
            return Ok(None);
        };
        let tex_this = tex_res as crate::object::This;
        let Some(host_tex) = crate::resource::host_resource(tex_this) else {
            self.set_unmapped_null(index, &view);
            return Ok(None);
        };
        let key = u64::from(generation) << 32 | u64::from(vi);
        self.ensure_placed_slot(index, key, &shader_view, host_tex.as_raw());
        Ok(Some(tex_this))
    }

    /// Whether to upload placed texture `texture` (reached through a slot)
    /// in a list recorded in `frame`: the first time, then as
    /// [`RECHECK_FRAMES`] allows.
    fn upload_due(&self, texture: crate::object::This, host: &ID3D12Resource, frame: u64) -> bool {
        use windows::core::Interface;
        let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
        let b = &mut *b;
        let mode = crate::resource::textures();
        let due = match b.uploaded.get(&(texture as usize)) {
            None => true,
            Some((h, _)) if mode == crate::resource::Textures::Keep && h.as_raw() == host.as_raw() => false,
            // Another texture at the object's address.
            Some((h, _)) if h.as_raw() != host.as_raw() => true,
            Some((_, last)) if mode == crate::resource::Textures::Always => frame > *last,
            Some((_, last)) if frame >= last + RECHECK_FRAMES => {
                if b.rechecks.0 != frame {
                    b.rechecks = (frame, 0);
                }
                b.rechecks.1 += 1;
                b.rechecks.1 <= RECHECKS
            }
            Some(_) => false,
        };
        if due {
            b.uploaded
                .insert(texture as usize, (host.clone(), frame));
            b.recheck.push_back((frame, texture as usize));
            self.recheck_at.store(
                b.recheck.front().map_or(u64::MAX, |&(f, _)| f + RECHECK_FRAMES),
                Ordering::Release,
            );
        }
        due
    }

    /// At submission: what the heap's bindless slots need. Only slots
    /// written since the last plan are made again; released views are found
    /// by one look at all slots' generations, and slots of evicted placed
    /// textures (or unmapped memory, once remapped) are made again. With
    /// nothing changed it is the last plan, whose reads the caller
    /// refreshes, which costs little when the title changed nothing.
    pub(crate) fn bindless_plan(&self) -> Option<Arc<BindlessPlan>> {
        if self.sampler {
            return None;
        }
        let epochs = (
            VIEWED.load(Ordering::Acquire),
            crate::resource::placed_epoch(),
            MEMORY_EPOCH.load(Ordering::Acquire),
        );
        // Asked before taking the heap's lock: the placed textures' lock is
        // taken first where both are held.
        let frame = crate::queue::presented() + 1;
        let evictions = {
            let b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
            b.checked.1 != epochs.1 && b.looked[0] != frame
        };
        let live = evictions.then(crate::resource::placed_live);
        let (mut todo, check, remade, exempt) = {
            let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
            let b = &mut *b;
            // Released views, evicted textures, and remapped memory are looked for once a frame.
            let look = b.looked[0] != frame && b.checked != epochs;
            let remake = b.full.1 && b.full.0 != frame;
            if b.dirty.is_empty()
                && !look
                && !remake
                && let Some(plan) = &b.plan
            {
                return Some(plan.clone());
            }
            if !look
                && !remake
                && b.recent.is_empty()
                && b.budget()[0] == 0
                && let Some(plan) = &b.plan
            {
                return Some(plan.clone());
            }
            let viewed = look && b.checked.0 != epochs.0;
            let mem_changed = look && b.checked.2 != epochs.2;
            let live = if look { live.as_ref() } else { None };
            if look {
                b.checked = epochs;
                b.looked[0] = frame;
            }
            let mapped = if mem_changed || viewed {
                let (ranges, seen) = mapped_since(b.mapped_seen);
                b.mapped_seen = seen;
                ranges
            } else {
                Some(Vec::new())
            };
            let mut remade: Vec<u32> = Vec::new();
            let mut dirty = BTreeSet::new();
            // What the title wrote last comes first and is made whatever it
            // takes: shaders run with a table not made read zeros as indices
            // of other slots, and a texture's slot read as a buffer's stops
            // the host GPU.
            let mut urgent = Vec::new();
            while urgent.len() < URGENT
                && let Some(i) = b.recent.pop()
            {
                if b.dirty.remove(&i) {
                    urgent.push(i);
                }
            }
            if b.dirty.is_empty() {
                b.recent.clear();
            }
            while dirty.len() < 4 * SLOT_CHUNK
                && let Some(i) = b.dirty.pop_first()
            {
                dirty.insert(i);
            }
            if let Some(l) = live {
                for (&i, u) in &b.uses {
                    if u.placed.is_some_and(|t| !l.contains(&t)) {
                        dirty.insert(i);
                    }
                }
            }
            if viewed || mem_changed {
                let t = std::time::Instant::now();
                // Only when a window died since: walking 65,000 to 85,000
                // slots a frame found none dead in a frame out of two
                // (7 to 10 ms, run bp1).
                let died = crate::shadow::died();
                let walked = if died != b.died_seen || cfg!(test) || !reuse() {
                    b.died_seen = died;
                    for &i in &b.direct_slots {
                        if b.uses
                            .get(&i)
                            .and_then(|u| u.direct.as_ref())
                            .is_some_and(|s| s.is_dead())
                        {
                            remade.push(i);
                            dirty.insert(i);
                        }
                    }
                    b.direct_slots.len()
                } else {
                    0
                };
                plan_note(0, t, walked, remade.len());
                for &i in &b.unmapped_slots {
                    if let Some(u) = b.uses.get(&i)
                        && (cfg!(test)
                            || (i + frame as u32).is_multiple_of(64)
                            || mapped.as_ref().is_none_or(|m| meets(m, u.at)))
                    {
                        dirty.insert(i);
                    }
                }
            }
            let t = std::time::Instant::now();
            let check: Vec<(u32, (u32, u32))> = if viewed {
                b.slots
                    .iter()
                    .filter(|(i, _)| !dirty.contains(i))
                    .map(|(&i, &v)| (i, v))
                    .collect()
            } else {
                Vec::new()
            };
            if viewed {
                plan_note(1, t, check.len(), 0);
            }
            for i in &urgent {
                dirty.remove(i);
            }
            let exempt = urgent.len();
            let todo: Vec<(u32, Option<(u32, u32)>)> = urgent
                .into_iter()
                .chain(dirty)
                .map(|i| (i, b.slots.get(&i).copied()))
                .collect();
            (todo, check, remade, exempt)
        };
        if !remade.is_empty() {
            let mut made = self.made.lock().unwrap_or_else(|e| e.into_inner());
            for i in remade {
                made[i as usize] = 0;
            }
        }
        let t = std::time::Instant::now();
        let stale = stale_views(&check);
        if !check.is_empty() {
            plan_note(2, t, check.len(), stale.len());
        }
        todo.extend(stale.into_iter().map(|(i, v)| (i, Some(v))));
        let (started, left) = (std::time::Instant::now(), self.budget_left(0));
        let mut made_any = false;
        let mut fresh = Vec::new();
        let mut fresh_reads = Vec::new();
        let mut results: Vec<(u32, Option<((u32, u32), Option<SlotUse>)>)> =
            Vec::with_capacity(todo.len());
        let mut over: Option<u32> = None;
        let mut todo = todo.into_iter();
        let mut count = 0usize;
        for (index, entry) in todo.by_ref() {
            if count >= exempt && (count & 15) == 0 && started.elapsed().as_micros() as u64 > left {
                // Over budget: the rest waits for the next submission.
                over = Some(index);
                break;
            }
            count += 1;
            made_any = true;
            let made = entry.map(|e| (e, self.use_slot(index, e)));
            results.push((index, made));
        }
        self.budget_spend(0, started.elapsed().as_micros() as u64);
        plan_note(3, started, count, 0);
        let mut b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
        let t_apply = std::time::Instant::now();
        let applied = results.len();
        // Whether a slot made reads, writes or keeps something else than
        // before: only then is the whole plan built again. A thousand slots
        // are made a frame, nearly all to what they were (run bp1).
        let mut changed = !reuse();
        // Diagnostics: slots that came out otherwise, of them those the
        // plan lacks something for.
        let (mut differing, mut added) = (0usize, 0usize);
        // Why: another write, another bounce, a read outside the plan, gone.
        let mut why = [0usize; 4];
        let planned = b.plan.as_ref().map(|p| p.reads.clone());
        for (index, made) in results {
            let old = b.uses.get(&index);
            let differs = match (&made, old) {
                (Some((_, Some(new))), Some(old)) => !same_ranges(new, old),
                (None, None) | (Some((_, None)), None) => false,
                _ => true,
            };
            if differs {
                differing += 1;
                let (wrote, bounced) = old.map_or((&None, None), |o| (&o.write, o.bounce));
                let new = made.as_ref().and_then(|m| m.1.as_ref());
                let mut bits = 0u8;
                let lacks = match (new, &planned) {
                    (_, None) => true,
                    (Some(new), Some(reads)) => {
                        // No submission uses the plan's writes outside
                        // tests (`Batch::prepare_bindless`), so another
                        // write alone leaves the plan as good as it was.
                        let lacking = [
                            cfg!(test) && !same_range(&new.write, wrote),
                            new.bounce != bounced,
                            !covered(reads, &new.read),
                        ];
                        for (n, l) in why.iter_mut().zip(lacking) {
                            *n += usize::from(l);
                        }
                        bits |= u8::from(lacking[0]) | u8::from(lacking[1]) << 1 | u8::from(lacking[2]) << 2;
                        lacking.contains(&true)
                    }
                    (None, Some(_)) => {
                        let wrote = cfg!(test) && wrote.is_some();
                        let gone = wrote || bounced.is_some();
                        why[3] += usize::from(gone);
                        bits |= u8::from(wrote) | u8::from(bounced.is_some()) << 1;
                        gone
                    }
                };
                if bits != 0 {
                    LACKS[bits as usize].fetch_add(1, Ordering::Relaxed);
                    for (seen, only) in LACK_FRAMES.iter().zip([true, bits & 5 != 0]) {
                        if only && seen[0].swap(frame, Ordering::Relaxed) != frame {
                            seen[1].fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                if lacks {
                    added += 1;
                    changed = true;
                } else {
                    b.loose = true;
                    let held = new.filter(|n| n.read.is_some()).and_then(|n| n.retained.clone());
                    b.extra.extend(held);
                }
            }
            match made {
                None => {
                    b.uses.remove(&index);
                    b.busy.remove(&index);
                    b.unmapped_slots.remove(&index);
                    b.direct_slots.remove(&index);
                }
                Some((_, Some(u))) => {
                    fresh.extend(u.bounce);
                    fresh_reads.extend(u.read.clone());
                    // Drawn textures are brought up to date at recording.
                    if u.placed.is_some_and(|t| t != 0) && !b.recorded.contains_key(&index) {
                        b.record_pending.insert(index);
                        self.record_hint.store(true, Ordering::Release);
                    }
                    if u.read.is_some() || u.write.is_some() || u.bounce.is_some() {
                        b.busy.insert(index);
                    } else {
                        b.busy.remove(&index);
                    }
                    if u.unmapped {
                        b.unmapped_slots.insert(index);
                    } else {
                        b.unmapped_slots.remove(&index);
                    }
                    if u.direct.is_some() {
                        b.direct_slots.insert(index);
                    } else {
                        b.direct_slots.remove(&index);
                    }
                    b.uses.insert(index, u);
                }
                Some((e, None)) => self.drop_bindless(&mut b, index, Some(e)),
            }
        }
        plan_note(4, t_apply, applied, differing);
        plan_note(6, t_apply, differing, added);
        plan_note(7, t_apply, why[0], why[1]);
        plan_note(8, t_apply, why[2], why[3]);
        if let Some(index) = over {
            b.dirty.insert(index);
        }
        b.dirty.extend(todo.map(|(i, _)| i));
        BACKLOG[0].store(b.dirty.len() as u64, Ordering::Relaxed);
        BACKLOG[1].store(b.record_pending.len() as u64, Ordering::Relaxed);
        b.full.1 |= made_any && changed || b.loose && frame >= b.full.0 + LOOSE_FRAMES;
        let remake = b.full.1 && b.full.0 != frame || cfg!(test);
        if !made_any && (!b.full.1 || !remake) && let Some(plan) = &b.plan {
            return Some(plan.clone());
        }
        // All of it is copied by the first submission of a frame only: the
        // later ones take what is new.
        if !remake && let Some(plan) = &b.plan {
            let plan = Arc::new(BindlessPlan {
                reads: plan.reads.clone(),
                writes: plan.writes.clone(),
                bounces: plan.bounces.clone(),
                fresh,
                fresh_reads,
                copied_in: AtomicU64::new(0),
                _retained: plan._retained.clone(),
                _extra: b.extra.clone(),
            });
            b.plan = Some(plan.clone());
            return Some(plan);
        }
        b.full = (frame, false);
        b.loose = false;
        b.extra.clear();
        let t_full = std::time::Instant::now();
        let (mut reads, mut writes, mut bounces, mut retained) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // What slots view is held by the slots, and for some frames after
        // (`host::retire`).
        for u in b.busy.iter().filter_map(|i| b.uses.get(i)) {
            reads.extend(u.read.clone());
            writes.extend(u.write.clone());
            bounces.extend(u.bounce);
            retained.extend(u.read.is_some().then(|| u.retained.clone()).flatten());
        }
        bounces.sort_unstable_by_key(|b| b.1);
        let (n_busy, n_reads) = (b.busy.len(), reads.len());
        let plan = Arc::new(BindlessPlan {
            reads: Arc::new(merged(reads, READ_GAP)),
            writes: Arc::new(merged(writes, 0)),
            bounces: Arc::new(bounces),
            fresh,
            fresh_reads,
            copied_in: AtomicU64::new(0),
            _retained: Arc::new(retained),
            _extra: Vec::new(),
        });
        b.plan = Some(plan.clone());
        plan_note(5, t_full, n_busy, n_reads);
        Some(plan)
    }

    /// Materializes bindless slot `index` from its view `(vi, generation)`
    /// and finds what it reads and writes; `None` if the slot is no longer
    /// one for bindless access (its view went stale, or is not of a buffer).
    fn use_slot(&self, index: u32, (vi, generation): (u32, u32)) -> Option<SlotUse> {
        use windows::core::Interface;
        let Some(view) = view_at(vi, generation) else {
            self.clear_stale_slot(index);
            return None;
        };
        if !view.is_buffer() {
            return None;
        }
        let unmapped = |view: &View, address: u64, len: u64| {
            self.set_unmapped_null(index, view);
            Some(SlotUse {
                unmapped: true,
                at: (address, address.saturating_add(len.max(1))),
                ..SlotUse::default()
            })
        };
        let key = u64::from(generation) << 32 | u64::from(vi);
        let mut slot = SlotUse::default();
        match &view {
            &View::Constant(address, size) => {
                let capped = size.min(65_536);
                if !valid_title_address(address, u64::from(capped)) {
                    return unmapped(&view, address, u64::from(capped));
                }
                let (mut reads, mut bounces) = (Vec::new(), Vec::new());
                match self.materialize_bindless_cbv(
                    index,
                    key,
                    address,
                    capped,
                    &mut reads,
                    &mut bounces,
                ) {
                    Some(res) => slot.retained = Some(res),
                    None => return unmapped(&view, address, u64::from(capped)),
                }
                match reads.pop() {
                    Some((s, _, _)) if s.is_direct() => slot.direct = Some(s),
                    read => slot.read = read,
                }
                slot.bounce = bounces.pop();
            }
            View::Placed(false, resource, desc) if resource[0] != 1 => {
                slot.placed = Some(0);
                let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
                if !plausible(address, 1) {
                    self.set_unmapped_null(index, &view);
                    return Some(slot);
                }
                if let Some(shader_view @ View::Shader(tex_res, _)) = placed_texture(&view)
                    && let Some(host_tex) =
                        crate::resource::host_resource(tex_res as crate::object::This)
                {
                    if crate::texture::title_written(host_tex) {
                        crate::texture::upload_first(tex_res as crate::object::This, host_tex);
                    }
                    self.ensure_placed_slot(index, key, &shader_view, host_tex.as_raw());
                    slot.placed = Some(tex_res);
                    slot.retained = Some(host_tex.clone());
                } else {
                    self.set_unmapped_null(index, &view);
                    // Its pages may come later.
                    slot.unmapped = true;
                    slot.at = (address, address + (4 << 20));
                }
            }
            _ => {
                let Some((address, len)) = buffer_range(&view) else {
                    self.set_unmapped_null(index, &view);
                    return Some(slot);
                };
                if !valid_title_address(address, len) {
                    return unmapped(&view, address, len);
                }
                slot.retained = self.ensure_buffer_slot(index, key, &view);
                if let Some(shadow) = crate::shadow::find(address, len) {
                    let clamped = len.min(shadow.left(address)) as usize;
                    if shadow.is_direct() {
                        slot.direct = Some(shadow);
                    } else if clamped > 0 {
                        slot.read = Some((shadow.clone(), address as usize, clamped));
                        if matches!(view, View::Unordered(..) | View::Placed(true, ..)) {
                            if clamped <= WRITE_CAP {
                                slot.write = Some((shadow, address as usize, clamped));
                            } else {
                                static ONCE: std::sync::Once = std::sync::Once::new();
                                ONCE.call_once(|| {
                                    tracing::warn!(
                                        bytes = clamped,
                                        "a bindless unordered access view too big to copy back"
                                    );
                                });
                            }
                        }
                    }
                }
            }
        }
        Some(slot)
    }

    /// The view host slot `index` was made from, in a few words, with the
    /// title and host addresses of a buffer view (diagnostics). "null" for
    /// a slot never made or left null.
    #[cfg(feature = "diagnostics")]
    /// What a slot views and how it was made, for the device-removed report.
    #[cfg(feature = "diagnostics")]
    pub(crate) fn report_slot(&self, index: u32) -> String {
        let mut text = self.describe_slot(index).unwrap_or_else(|| "?".into());
        let b = self.bindless.lock().unwrap_or_else(|e| e.into_inner());
        if b.dirty.contains(&index) {
            text.push_str(" DIRTY");
        }
        match b.uses.get(&index) {
            None => text.push_str(" no use"),
            Some(u) => {
                use std::fmt::Write;
                let _ = write!(text, " at {:#x}..{:#x} unmapped {}", u.at.0, u.at.1, u.unmapped);
                if let Some(s) = &u.direct {
                    let _ = write!(text, " direct dead {}", s.is_dead());
                }
                if let Some(t) = u.placed {
                    let _ = write!(text, " placed {t:#x}");
                }
                if let Some(r) = &u.retained {
                    // SAFETY: a live resource.
                    let d = unsafe { r.GetDesc() };
                    let _ = write!(text, " retained dim {} {}x{} f{} flags {:#x}", d.Dimension.0, d.Width, d.Height, d.Format.0, d.Flags.0);
                    if d.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
                        // SAFETY: a live buffer.
                        let _ = write!(text, " va {:#x}", unsafe { r.GetGPUVirtualAddress() });
                    }
                }
                if let Some((s, a, n)) = &u.read {
                    let _ = write!(text, " read {a:#x}+{n:#x} va {:#x}", s.gpu(*a as u64));
                }
            }
        }
        text
    }

    pub(crate) fn describe_slot(&self, index: u32) -> Option<String> {
        let key = *self
            .made
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(index as usize)?;
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

/// Whether `[address, address + len)` may be title memory: inside the
/// title's address range, not null.
fn plausible(address: u64, len: u64) -> bool {
    address >= 0x10000
        && len > 0
        && address
            .checked_add(len)
            .is_some_and(|end| end <= (1u64 << 48))
}

/// Whether `[address, address + len)` is title memory: plausible, and in a
/// title buffer or committed. The kernel is asked only outside buffers:
/// in the title's big pools each asking costs milliseconds.
fn valid_title_address(address: u64, len: u64) -> bool {
    plausible(address, len)
        && (crate::shadow::in_buffer(address, len)
            || crate::texture::committed(address as *const u8, 1))
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
    let first_off = buffer.FirstElement.checked_mul(element)?;
    let byte_len = u64::from(buffer.NumElements).checked_mul(element)?;
    Some((base.checked_add(first_off)?, byte_len))
}

fn uav_element_size(desc: &D3D12_UNORDERED_ACCESS_VIEW_DESC) -> Option<u64> {
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    if buffer.StructureByteStride > 0 {
        Some(u64::from(buffer.StructureByteStride))
    } else if buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
        Some(4)
    } else {
        crate::resource::element(desc.Format.0 as u32).map(|(bytes, _)| bytes)
    }
}

/// Element byte stride of `view` for the runtime stride table (`0` for
/// non-buffer views or descriptors without a known element size).
fn view_stride(view: &View) -> u32 {
    let bytes = match view {
        View::Shader(_, Some(desc)) => element_size(&srv_desc(desc)),
        View::Placed(false, resource, desc) if resource[0] == 1 => desc[..10]
            .try_into()
            .ok()
            .map_or(0, |w: &[u32; 10]| element_size(&srv_desc(w))),
        View::Unordered(_, _, Some(desc)) => uav_element_size(&uav_desc(desc)).unwrap_or(0),
        View::Placed(true, resource, desc) if resource[0] == 1 => desc[..10]
            .try_into()
            .ok()
            .and_then(|w: &[u32; 10]| uav_element_size(&uav_desc(w)))
            .unwrap_or(0),
        _ => 0,
    };
    u32::try_from(bytes).unwrap_or(0)
}

/// The elements of the title's buffer view `view` (0 for other views).
fn view_count(view: &View) -> u32 {
    let srv = |w: &[u32; 10]| {
        let desc = srv_desc(w);
        // SAFETY: a buffer view's union member.
        (desc.ViewDimension == D3D12_SRV_DIMENSION_BUFFER)
            .then(|| unsafe { desc.Anonymous.Buffer }.NumElements)
    };
    let uav = |w: &[u32; 10]| {
        let desc = uav_desc(w);
        // SAFETY: a buffer view's union member.
        (desc.ViewDimension == D3D12_UAV_DIMENSION_BUFFER)
            .then(|| unsafe { desc.Anonymous.Buffer }.NumElements)
    };
    match view {
        View::Shader(_, Some(desc)) => srv(desc),
        View::Placed(false, resource, desc) if resource[0] == 1 => {
            desc[..10].try_into().ok().and_then(srv)
        }
        View::Unordered(_, _, Some(desc)) => uav(desc),
        View::Placed(true, resource, desc) if resource[0] == 1 => {
            desc[..10].try_into().ok().and_then(uav)
        }
        _ => None,
    }
    .unwrap_or(0)
}

/// [`buffer_range`] of an unordered access view of memory at `base`.
fn unordered_range(base: u64, words: &[u32; 10]) -> Option<(u64, u64)> {
    let desc = uav_desc(words);
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { desc.Anonymous.Buffer };
    let element = uav_element_size(&desc)?;
    Some((
        base + buffer.FirstElement * element,
        u64::from(buffer.NumElements) * element,
    ))
}

/// Whether a raw buffer view may start at element `first` (4 bytes each):
/// the host wants 16-byte aligned starts, the console only 4, and refuses
/// the call (removing the device) for the rest.
/// shortcut: a view the host refuses reads nothing (a null view); starting
/// it 16 bytes lower would need the shader to add the difference.
fn raw_start_fits(first: u64) -> bool {
    first.is_multiple_of(4)
}

/// The host resource a slot made by [`make`] views, if any.
type Kept = Option<ID3D12Resource>;

/// A structured view's bytes as a raw view of its host copy: the first
/// dword and the dword count, for a view `offset` bytes into the copy whose
/// description starts at element `first` and has `count` elements of
/// `stride` bytes.
///
/// The host places structured element `i` at byte `i * stride` of the
/// resource, so a structured view whose start in the copy is not a multiple
/// of its stride (a title buffer's view 16 bytes past a 32-byte element)
/// cannot be made there. A raw view of the same bytes can if its start is
/// 16-byte aligned (`D3D12_RAW_UAV_SRV_BYTE_ALIGNMENT`; the host removes the
/// device for one that is not), and it reads and writes the copy every other
/// view of that memory uses. Translated shaders read buffer views raw unless
/// they index them (`victory-shader`, "Buffer views in the heap").
fn raw_window(offset: u64, first: u64, count: u32, stride: u32) -> Option<(u64, u32)> {
    if stride == 0 || !stride.is_multiple_of(4) {
        return None;
    }
    let start = offset.checked_add(first.checked_mul(u64::from(stride))?)?;
    if !start.is_multiple_of(u64::from(D3D12_RAW_UAV_SRV_BYTE_ALIGNMENT)) {
        return None;
    }
    let dwords = u64::from(count) * u64::from(stride / 4);
    Some((start / 4, u32::try_from(dwords).ok()?))
}

/// Whether structured buffers of this stride are raw views on the host (as
/// the shaders take them: `emit::lower`).
fn raw_stride(stride: u32) -> bool {
    stride >= 16 && stride.is_multiple_of(16)
}

fn buffer_srv(
    address: u64,
    len: u64,
    mut desc: D3D12_SHADER_RESOURCE_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) -> Option<Kept> {
    // SAFETY: a buffer view's union member.
    let structured = raw_stride(unsafe { desc.Anonymous.Buffer }.StructureByteStride);
    let element = if structured { 0 } else { element_size(&desc) };
    let shadow = crate::shadow::for_view(address, len, element)?;
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { &mut desc.Anonymous.Buffer };
    let offset = shadow.offset(address);
    if element != 0 && offset % element == 0 {
        buffer.FirstElement += offset / element;
        if buffer.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0
            && !raw_start_fits(buffer.FirstElement)
        {
            let first = buffer.FirstElement;
            once!(first, "raw buffer view not on 16 bytes of its host copy");
            return None;
        }
    } else if let Some((first, count)) = raw_window(
        offset,
        buffer.FirstElement,
        buffer.NumElements,
        buffer.StructureByteStride,
    ) {
        // Shaders read all structured buffers as raw ones (AMD reads element
        // 0 for every element of a raw view through a structured buffer).
        *buffer = D3D12_BUFFER_SRV {
            FirstElement: first,
            NumElements: count,
            StructureByteStride: 0,
            Flags: D3D12_BUFFER_SRV_FLAG_RAW,
        };
        desc.Format = DXGI_FORMAT_R32_TYPELESS;
    } else {
        once!(
            offset,
            element,
            "buffer view not on an element of its host copy"
        );
        return None;
    }
    let size = if buffer.StructureByteStride > 0 { u64::from(buffer.StructureByteStride) } else { element_size_of(buffer.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0, element) };
    if !fits_buffer(&shadow, buffer.FirstElement, buffer.NumElements, size) {
        return None;
    }
    let host = host()?;
    // SAFETY: a live buffer and a slot of our heap.
    unsafe {
        host.device
            .CreateShaderResourceView(&shadow.buffer, Some(&desc), at)
    };
    Some(Some(shadow.buffer.clone()))
}

fn element_size_of(raw: bool, element: u64) -> u64 {
    if raw { 4 } else { element }
}

/// Whether a view of `count` elements of `size` bytes from `first` on lies in
/// the buffer (the host removes the device for one that does not, or is
/// empty).
fn fits_buffer(shadow: &crate::shadow::Shadow, first: u64, count: u32, size: u64) -> bool {
    // SAFETY: a live buffer.
    let width = unsafe { shadow.buffer.GetDesc() }.Width;
    let end = first
        .checked_add(u64::from(count))
        .and_then(|n| n.checked_mul(size.max(1)));
    let fits = count > 0 && size > 0 && end.is_some_and(|e| e <= width);
    if !fits {
        once!(first, count, size, width, "a buffer view the host does not allow");
    }
    fits
}

/// An unordered access view of title buffer memory at `address` (`len`
/// bytes) on its host copy, as [`buffer_srv`]; false if the copy cannot be
/// written by shaders (upload memory).
fn buffer_uav(
    address: u64,
    len: u64,
    mut desc: D3D12_UNORDERED_ACCESS_VIEW_DESC,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
) -> Option<Kept> {
    // SAFETY: a buffer view's union member.
    let structured = raw_stride(unsafe { desc.Anonymous.Buffer }.StructureByteStride);
    let whole = if structured { 0 } else { uav_element_size(&desc).unwrap_or(0) };
    let shadow = crate::shadow::for_view(address, len, whole)?;
    // SAFETY: a live buffer.
    let flags = unsafe { shadow.buffer.GetDesc() }.Flags;
    if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
        once!("unordered access to a buffer whose host copy shaders cannot write");
        return None;
    }
    // SAFETY: a buffer view's union member.
    let buffer = unsafe { &mut desc.Anonymous.Buffer };
    let element = if structured {
        0
    } else if buffer.StructureByteStride > 0 {
        u64::from(buffer.StructureByteStride)
    } else if buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0 {
        4
    } else {
        crate::resource::element(desc.Format.0 as u32).map_or(0, |(bytes, _)| bytes)
    };
    let offset = shadow.offset(address);
    buffer.CounterOffsetInBytes = 0;
    if element != 0 && offset % element == 0 {
        buffer.FirstElement += offset / element;
        if buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0
            && !raw_start_fits(buffer.FirstElement)
        {
            let first = buffer.FirstElement;
            once!(
                first,
                "raw unordered buffer view not on 16 bytes of its host copy"
            );
            return None;
        }
    } else if let Some((first, count)) = raw_window(
        offset,
        buffer.FirstElement,
        buffer.NumElements,
        buffer.StructureByteStride,
    ) {
        // As in `buffer_srv`; what shaders write lands in the shared copy,
        // in the range `buffer_range` gives (copied back to title memory).
        *buffer = D3D12_BUFFER_UAV {
            FirstElement: first,
            NumElements: count,
            StructureByteStride: 0,
            CounterOffsetInBytes: 0,
            Flags: D3D12_BUFFER_UAV_FLAG_RAW,
        };
        desc.Format = DXGI_FORMAT_R32_TYPELESS;
    } else {
        once!(offset, element, "unordered buffer view not on an element of its host copy");
        return None;
    }
    let size = if buffer.StructureByteStride > 0 { u64::from(buffer.StructureByteStride) } else { element_size_of(buffer.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0, element) };
    if !fits_buffer(&shadow, buffer.FirstElement, buffer.NumElements, size) {
        return None;
    }
    let host = host()?;
    // SAFETY: a live buffer and a slot of our heap.
    unsafe {
        host.device.CreateUnorderedAccessView(
            &shadow.buffer,
            None::<&ID3D12Resource>,
            Some(&desc),
            at,
        )
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
    // A 32-bit texture shaders write, read as shared-exponent colors.
    if view == crate::resource::SHARED_EXPONENT
        && family(own) == 39
        && d.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0
    {
        return !unordered;
    }
    // A packed 16-bit target made R8G8B8A8_UNORM on this GPU
    // (`resource::target_format`): views name the format it has.
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

/// `d` made a view of a texture without the `skip` largest mips.
fn without_mips(d: &mut D3D12_SHADER_RESOURCE_VIEW_DESC, skip: u32) {
    let cut = |first: &mut u32, levels: &mut u32, clamp: Option<&mut f32>| {
        if *levels != u32::MAX {
            let end = (*first + *levels).saturating_sub(skip).max(1);
            *levels = end - first.saturating_sub(skip).min(end - 1);
        }
        *first = first.saturating_sub(skip);
        if let Some(clamp) = clamp {
            *clamp = (*clamp - skip as f32).max(0.0);
        }
    };
    // SAFETY: the member the dimension names.
    unsafe {
        match d.ViewDimension {
            D3D12_SRV_DIMENSION_TEXTURE2D => {
                let t = &mut d.Anonymous.Texture2D;
                cut(&mut t.MostDetailedMip, &mut t.MipLevels, Some(&mut t.ResourceMinLODClamp));
            }
            D3D12_SRV_DIMENSION_TEXTURE2DARRAY => {
                let t = &mut d.Anonymous.Texture2DArray;
                cut(&mut t.MostDetailedMip, &mut t.MipLevels, Some(&mut t.ResourceMinLODClamp));
            }
            D3D12_SRV_DIMENSION_TEXTURECUBE => {
                let t = &mut d.Anonymous.TextureCube;
                cut(&mut t.MostDetailedMip, &mut t.MipLevels, Some(&mut t.ResourceMinLODClamp));
            }
            D3D12_SRV_DIMENSION_TEXTURECUBEARRAY => {
                let t = &mut d.Anonymous.TextureCubeArray;
                cut(&mut t.MostDetailedMip, &mut t.MipLevels, Some(&mut t.ResourceMinLODClamp));
            }
            _ => {}
        }
    }
}

/// Makes the host descriptor for `view` at `at`: the host resource it
/// views, or `None` if it cannot.
fn make(view: &View, at: D3D12_CPU_DESCRIPTOR_HANDLE) -> Option<Kept> {
    let host = host()?;
    let device = &host.device;
    match view {
        &View::Constant(address, size) => {
            if address == 0 || size == 0 {
                return None;
            }
            let capped = size.min(65_536);
            let shadow = crate::shadow::find(address, u64::from(capped))?;
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
                SizeInBytes: capped.next_multiple_of(256),
            };
            // SAFETY: a slot of our heap; the range lies in the host copy.
            unsafe { device.CreateConstantBufferView(Some(&desc), at) };
            Some(Some(shadow.buffer.clone()))
        }
        View::Shader(resource, desc) => {
            let resource = *resource as crate::object::This;
            if let Some(texture) = crate::resource::host_resource(resource) {
                let mut desc = desc.as_ref().map(srv_desc);
                if !fits(texture, desc.as_mut().map(|d| &mut d.Format), false) {
                    return None;
                }
                let skip = crate::resource::skipped_mips(resource);
                if skip > 0
                    && let Some(d) = &mut desc
                {
                    without_mips(d, skip);
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
            if let Some(twin) = crate::resource::depth_twin(resource) {
                return crate::twin::view(twin, desc.as_ref(), at).map(Some);
            }
            let mut desc = desc.as_ref().map(uav_desc);
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
                once!(
                    dimension = resource[0],
                    "placed unordered texture view not mirrored"
                );
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

/// The largest mip of a placed 2D texture (or array, or cube) that a shader
/// view reads, if the host texture can do without the larger ones.
fn largest_mip(resource: &[u32; 14], desc: &[u32; 16]) -> u32 {
    const TEXTURE2D: u32 = 3;
    // Textures, arrays, cubes and arrays of cubes: their descriptions start
    // with the largest mip.
    if resource[0] != TEXTURE2D || !matches!(desc[1], 4 | 5 | 9 | 10) || cfg!(test) {
        return 0;
    }
    let (width, height) = (resource[4], resource[6]);
    let (format, mips) = (resource[8], resource[7] >> 16);
    // The view may also say so with a least level of detail while it
    // starts at mip 0: the title streams the larger mips in later, and their
    // memory holds nothing yet (`VICTORY_LOD_CLAMP=0`: as before, when the
    // host texture was made whole for such a view).
    let clamp = {
        let words: &[u32; 10] = desc[..10].try_into().unwrap_or(&[0; 10]);
        let d = srv_desc(words);
        // SAFETY: the member the dimension names.
        let clamp = unsafe {
            match d.ViewDimension {
                D3D12_SRV_DIMENSION_TEXTURE2D => d.Anonymous.Texture2D.ResourceMinLODClamp,
                D3D12_SRV_DIMENSION_TEXTURE2DARRAY => d.Anonymous.Texture2DArray.ResourceMinLODClamp,
                D3D12_SRV_DIMENSION_TEXTURECUBE => d.Anonymous.TextureCube.ResourceMinLODClamp,
                D3D12_SRV_DIMENSION_TEXTURECUBEARRAY => d.Anonymous.TextureCubeArray.ResourceMinLODClamp,
                _ => 0.0,
            }
        };
        if clamp.is_finite() && clamp >= 1.0 { clamp.floor().min(32.0) as u32 } else { 0 }
    };
    static CLAMPED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| std::env::var_os("VICTORY_LOD_CLAMP").is_none_or(|v| v != "0"));
    let n = CLAMPED[usize::from(clamp > desc[4])].fetch_add(1, Ordering::Relaxed);
    if clamp > desc[4] && n.is_power_of_two() {
        tracing::info!(
            views = n + 1,
            others = CLAMPED[0].load(Ordering::Relaxed),
            clamp,
            first = desc[4],
            width,
            height,
            on,
            "a placed texture's view holds its detail back"
        );
    }
    let mut first = desc[4].max(if on { clamp } else { 0 }).min(mips.saturating_sub(1));
    let block = (70..=84).contains(&format) || (94..=99).contains(&format);
    let fits = |k: u32| {
        (width >> k) > 0
            && (height >> k) > 0
            && (!block || (width >> k).is_multiple_of(4) && (height >> k).is_multiple_of(4))
    };
    while first > 0 && !fits(first) {
        first -= 1;
    }
    // Not textures the host GPU writes.
    if resource[12] & 7 != 0 { 0 } else { first }
}

/// A shader view of a texture the title placed in memory itself, as a view
/// of the resource made for that memory (on first use).
pub(crate) fn placed_texture(view: &View) -> Option<View> {
    const BUFFER: u32 = 1;
    let View::Placed(false, resource, desc) = view else {
        return None;
    };
    if resource[0] == BUFFER {
        return None;
    }
    let address = u64::from(desc[10]) | u64::from(desc[11]) << 32;
    #[cfg(feature = "diagnostics")]
    crate::texture::flips::view(address, resource, *desc);
    let texture = crate::resource::texture_at(address, resource, largest_mip(resource, desc))?;
    Some(View::Shader(texture as usize, desc[..10].try_into().ok()))
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
    use crate::descriptor::{
        HeapDesc, copy_simple, create_cbv, create_heap, create_placed_srv, create_srv, create_uav,
        heap,
    };
    use crate::object::{Iid, S_OK, This, guid, slot};
    use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::core::s;

    const HEAP_IID: Iid = guid(
        0x8efb471d,
        0x616c,
        0x4f49,
        [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51],
    );
    const RESOURCE_IID: Iid = guid(
        0x696442be,
        0xa72e,
        0x4059,
        [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad],
    );

    #[repr(C)]
    struct ExtendedHeapDesc {
        desc: HeapDesc,
        addr: u64,
        size: u64,
    }

    fn make_shader_heap(count: u32) -> (This, u64) {
        let ext = ExtendedHeapDesc {
            desc: HeapDesc {
                kind: 0,
                count,
                flags: 1,
                nodes: 0,
            },
            addr: 0,
            size: 0,
        };
        let mut out = std::ptr::null();
        assert_eq!(
            create_heap(std::ptr::null(), &ext.desc, &HEAP_IID, &mut out),
            S_OK
        );
        let start = slot::<extern "system" fn(This) -> u64>(out, 0x48)(out);
        (out, start)
    }

    fn make_title_buffer(address: u64, size: u64, uav: bool) -> This {
        let words: [u32; 14] = [
            1, // BUFFER
            0,
            0,
            0,
            size as u32,
            (size >> 32) as u32,
            1,
            0x0001_0001,
            0,
            1,
            0,
            1, // ROW_MAJOR
            if uav { 4 } else { 0 },
            0,
        ];
        let mut out = std::ptr::null();
        assert_eq!(
            crate::resource::create_at_address(
                std::ptr::null(),
                address,
                words.as_ptr().cast(),
                0,
                std::ptr::null(),
                &RESOURCE_IID,
                &mut out,
            ),
            S_OK
        );
        out
    }

    fn compile_probe_cs(device: &ID3D12Device) -> (ID3D12RootSignature, ID3D12PipelineState) {
        const PROBE_HLSL: &str = r#"
#define RS "DescriptorTable(SRV(t0, numDescriptors=1)), \
            DescriptorTable(CBV(b0, numDescriptors=1)), \
            DescriptorTable(SRV(t1, numDescriptors=1)), \
            DescriptorTable(UAV(u0, numDescriptors=1)), \
            DescriptorTable(SRV(t2, numDescriptors=1)), \
            DescriptorTable(CBV(b1, numDescriptors=1))"
ByteAddressBuffer srv_buf : register(t0);
cbuffer Cbv16 : register(b0) { uint4 cb_vals; };
Texture2D<float4> placed_tex : register(t1);
RWByteAddressBuffer uav_buf : register(u0);
ByteAddressBuffer copied_srv : register(t2);
cbuffer NullCbv : register(b1) { uint4 null_vals; };

[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {
    uint v_srv = srv_buf.Load(0);
    uint v_cbv = cb_vals.x + cb_vals.y + cb_vals.z + cb_vals.w;
    float4 px = placed_tex.Load(int3(0, 0, 0));
    uint v_tex = (uint(round(px.x * 255.0)) << 0)
               | (uint(round(px.y * 255.0)) << 8)
               | (uint(round(px.z * 255.0)) << 16)
               | (uint(round(px.w * 255.0)) << 24);
    uint v_copied = copied_srv.Load(0);
    uint v_null = null_vals.x;
    uav_buf.Store(0, v_srv);
    uav_buf.Store(4, v_cbv);
    uav_buf.Store(8, v_tex);
    uav_buf.Store(12, v_copied);
    uav_buf.Store(16, v_null);
}
"#;
        let mut code: Option<ID3DBlob> = None;
        // SAFETY: static source and entry point strings; out pointer ours.
        unsafe {
            D3DCompile(
                PROBE_HLSL.as_ptr().cast(),
                PROBE_HLSL.len(),
                s!("bindless_probe"),
                None,
                None,
                s!("main"),
                s!("cs_5_1"),
                0,
                0,
                &mut code,
                None,
            )
            .unwrap();
            let code = code.unwrap();
            let bytes = std::slice::from_raw_parts(
                code.GetBufferPointer().cast::<u8>(),
                code.GetBufferSize(),
            );
            let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes).unwrap();
            let pso = device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: bytes.as_ptr().cast(),
                        BytecodeLength: bytes.len(),
                    },
                    ..Default::default()
                })
                .unwrap();
            (root, pso)
        }
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
    fn dynamic_slots_are_held_until_released() {
        let Some(heap) = HostHeap::new(false, 4) else {
            return; // no host GPU
        };
        let a = heap.dynamic(heap.spare - 100).unwrap();
        let b = heap.dynamic(60).unwrap();
        assert_eq!((a, b), (4, 4 + heap.spare - 100));
        // 40 left at the end: 50 do not fit until something is given back.
        assert_eq!(heap.dynamic(50), None);
        heap.release(a, 50);
        assert_eq!(heap.dynamic(50), Some(4));
        assert_eq!(heap.dynamic(40), Some(4 + heap.spare - 40));
    }

    /// A sampler heap past the host's limit is capped, not lost, and keeps
    /// dynamic slots for the tables reaching past what it holds.
    #[test]
    fn large_sampler_heap_is_capped() {
        let Some(heap) = HostHeap::new(true, 4 * SAMPLERS) else {
            return; // no host GPU
        };
        let count = SAMPLERS - SAMPLER_DYNAMIC;
        assert_eq!(heap.count(), count);
        assert_eq!(heap.dynamic(SAMPLER_DYNAMIC + 1), None);
        let a = heap.dynamic(SAMPLER_DYNAMIC).unwrap();
        assert_eq!(a, count);
        assert_eq!(heap.dynamic(1), None);
        heap.release(a, 8);
        assert_eq!(heap.dynamic(8), Some(count));
    }

    /// A sampler heap within the limit has a slot per title slot and no
    /// dynamic ones.
    #[test]
    fn small_sampler_heap_has_no_dynamic_slots() {
        let Some(heap) = HostHeap::new(true, 16) else {
            return; // no host GPU
        };
        assert_eq!(heap.count(), 16);
        assert_eq!(heap.dynamic(1), None);
    }

    /// A structured view whose start in its host copy is not on an element
    /// becomes the raw view of the same bytes, where those start 16-byte
    /// aligned.
    #[test]
    fn structured_views_off_an_element_are_raw_views_of_the_same_bytes() {
        // (offset in the copy, first element, elements, stride)
        for (offset, first, count, stride) in [
            (16, 0, 4, 32),
            (48, 3, 7, 32),
            (4096 + 16, 1, 1, 48),
            (32, 0, 9, 12),
        ] {
            let (dword, dwords) = raw_window(offset, first, count, stride).unwrap();
            assert_eq!(dword * 4, offset + first * u64::from(stride));
            assert_eq!(u64::from(dwords) * 4, u64::from(count) * u64::from(stride));
        }
        // Not 16-byte aligned, strides raw views cannot cover, too long.
        assert_eq!(raw_window(8, 0, 4, 32), None);
        assert_eq!(raw_window(16, 1, 4, 12), None);
        assert_eq!(raw_window(16, 0, 4, 6), None);
        assert_eq!(raw_window(16, 0, 4, 0), None);
        assert_eq!(raw_window(16, 0, u32::MAX, 32), None);
    }

    /// A structured view (32-byte elements) 16 bytes into its host copy,
    /// read and another written by a shader: it reads the title's bytes from
    /// there, and what it writes lands at its own bytes of title memory.
    #[test]
    fn structured_views_off_an_element_read_and_write_their_bytes() {
        use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
        use windows::Win32::Graphics::Direct3D::ID3DBlob;
        use windows::core::s;
        let Some(host) = host() else {
            return; // no host GPU
        };
        const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0), UAV(u0, numDescriptors = 2))"
ByteAddressBuffer view : register(t0);
RWByteAddressBuffer window : register(u0);
RWByteAddressBuffer seen : register(u1);
[RootSignature(RS)]
[numthreads(32, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    seen.Store(id.x * 4, view.Load(id.x * 4));
    window.Store(id.x * 4, 0xc0de0000u | id.x);
}
"#;
        let mut code: Option<ID3DBlob> = None;
        // SAFETY: the source and names live through the call; out pointer ours.
        unsafe {
            D3DCompile(
                SOURCE.as_ptr().cast(),
                SOURCE.len(),
                s!("views"),
                None,
                None,
                s!("main"),
                s!("cs_5_1"),
                0,
                0,
                &mut code,
                None,
            )
        }
        .unwrap();
        let code = code.unwrap();
        let device = &host.device;
        // SAFETY: the blob's own bytes, with the root signature inside;
        // plain object creation.
        let (root, pso, views, step) = unsafe {
            let bytes = std::slice::from_raw_parts(
                code.GetBufferPointer().cast::<u8>(),
                code.GetBufferSize(),
            );
            let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes).unwrap();
            let pso: ID3D12PipelineState = device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: bytes.as_ptr().cast(),
                        BytecodeLength: bytes.len(),
                    },
                    ..Default::default()
                })
                .unwrap();
            let views: ID3D12DescriptorHeap = device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 3,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
                .unwrap();
            let step =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            (root, pso, views, step as usize)
        };
        // SAFETY: a live heap.
        let first = unsafe { views.GetCPUDescriptorHandleForHeapStart() }.ptr;
        let at = |i: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: first + i * step,
        };

        // A title buffer of 1 KiB, numbered dwords; four 32-byte elements
        // from byte 16 are read, four from byte 272 written.
        let before: Vec<u32> = (0..256).map(|i| 0x7171_0000 | i).collect();
        let mut title = before.clone();
        let start = title.as_mut_ptr() as u64;
        crate::shadow::register(start as usize, start, 1024);
        let shadow = crate::shadow::find(start, 1024).unwrap();
        let structured = D3D12_BUFFER_SRV {
            NumElements: 4,
            StructureByteStride: 32,
            ..Default::default()
        };
        let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_UNKNOWN,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: structured },
        };
        let uav = |buffer| D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_UNKNOWN,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Buffer: buffer },
        };
        let written = D3D12_BUFFER_UAV {
            NumElements: 4,
            StructureByteStride: 32,
            ..Default::default()
        };
        // Views the host refuses are left null, as `HostHeap::mirror` leaves them.
        let mut placed = [0; 14];
        placed[0] = 1; // a buffer
        if buffer_srv(start + 16, 128, srv, at(0)).is_none() {
            null_for(&View::Placed(false, placed, [0; 16]), at(0));
        }
        if buffer_uav(start + 272, 128, uav(written), at(1)).is_none() {
            null_for(&View::Placed(true, placed, [0; 16]), at(1));
        }
        // What the shader read, as a raw view of 32 dwords of our own.
        let (seen, out) = host.shared_buffer(128).unwrap();
        let mut raw = uav(D3D12_BUFFER_UAV {
            NumElements: 32,
            Flags: D3D12_BUFFER_UAV_FLAG_RAW,
            ..Default::default()
        });
        raw.Format = DXGI_FORMAT_R32_TYPELESS;
        // SAFETY: a live buffer and a slot of our heap.
        unsafe {
            device.CreateUnorderedAccessView(&seen, None::<&ID3D12Resource>, Some(&raw), at(2))
        };

        let list = host.new_list().unwrap();
        shadow.refresh(&list, start, 1024);
        shadow.gpu_writes(&list, start + 272, 128);
        // SAFETY: an open list; our heap, root signature and pipeline.
        unsafe {
            list.list.SetDescriptorHeaps(&[Some(views.clone())]);
            list.list.SetComputeRootSignature(&root);
            list.list.SetPipelineState(&pso);
            list.list
                .SetComputeRootDescriptorTable(0, views.GetGPUDescriptorHandleForHeapStart());
            list.list.Dispatch(1, 1, 1);
        }
        list.close();
        host.execute(&[&list]);
        crate::shadow::unregister(start as usize, start);

        // SAFETY: 32 dwords the GPU wrote, read after it finished.
        let seen = unsafe { std::slice::from_raw_parts(out.cast::<u32>(), 32) };
        assert_eq!(seen, &before[4..36], "what the read view gave");
        let marks: Vec<u32> = (0..32).map(|i| 0xc0de_0000 | i).collect();
        assert_eq!(&title[68..100], &marks[..], "what the written view left");
        assert_eq!(
            (&title[..68], &title[100..]),
            (&before[..68], &before[100..])
        );
    }

    /// End-to-end GPU execution test proving that buffer SRVs, 16-byte-aligned
    /// CBVs, copied buffer SRVs, `View::Placed` streamed textures, and buffer
    /// UAVs in a shader-visible heap materialize non-null host descriptors,
    /// refresh modified title memory across submissions, copy bindless UAV
    /// writes back to title memory, and map null/invalid addresses safely to 0.
    #[test]
    fn bindless_slots_materialize_refresh_and_copy_back_on_gpu() {
        let Some(host) = host() else {
            return;
        };
        let (title_heap, base) = make_shader_heap(16);
        let slot_handle = |i: u64| base + i * 32;

        // Backing title memory (64 KiB aligned).
        let mem = crate::resource::Memory::zeroed(256 << 10, 64 << 10).unwrap();
        let mem_base = mem.address();
        let srv_addr = mem_base;
        let cbv_16b_addr = mem_base + 0x30; // 16B-aligned, NOT 256B-aligned!
        let copy_srv_addr = mem_base + 0x1000;
        let uav_addr = mem_base + 0x2000;
        let placed_tex_addr = mem_base + (64 << 10);

        let srv_res = make_title_buffer(srv_addr, 256, false);
        let copy_srv_res = make_title_buffer(copy_srv_addr, 256, false);
        let uav_res = make_title_buffer(uav_addr, 256, true);

        // Initial CPU data in title memory.
        // SAFETY: offsets lie within the 256 KiB allocation `mem`.
        unsafe {
            (srv_addr as *mut u32).write(13);
            (cbv_16b_addr as *mut [u32; 4]).write([10, 20, 30, 40]);
            (copy_srv_addr as *mut u32).write(0xCAFE_1234);
            (uav_addr as *mut [u32; 8]).write([0; 8]);
            // First pixel of linear 4x4 RGBA8 placed texture at `placed_tex_addr`.
            (placed_tex_addr as *mut [u8; 4]).write([0x12, 0x34, 0x56, 0x78]);
        }

        // Raw ByteAddressBuffer SRV/UAV descriptions in host D3D12 layout.
        let srv_desc_host = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_SRV {
                    FirstElement: 0,
                    NumElements: 16,
                    StructureByteStride: 0,
                    Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                },
            },
        };
        let uav_desc_host = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: 16,
                    StructureByteStride: 0,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                },
            },
        };
        // SAFETY: both D3D12 view descriptions are 40-byte POD values (`[u32; 10]`).
        let (raw_srv_words, raw_uav_words): ([u32; 10], [u32; 10]) = unsafe {
            (
                std::ptr::from_ref(&srv_desc_host)
                    .cast::<[u32; 10]>()
                    .read(),
                std::ptr::from_ref(&uav_desc_host)
                    .cast::<[u32; 10]>()
                    .read(),
            )
        };

        // Slot 0: Buffer SRV.
        create_srv(std::ptr::null(), srv_res, &raw_srv_words, slot_handle(0));
        // Slot 1: 16B-aligned CBV (size 64 bytes, address % 256 == 0x30).
        create_cbv(
            std::ptr::null(),
            &[cbv_16b_addr as u32, (cbv_16b_addr >> 32) as u32, 64, 0],
            slot_handle(1),
        );
        // Slot 2: Placed 4x4 R8G8B8A8_UNORM linear texture (`layout = 0x100`).
        let placed_res_words: [u32; 14] = [
            3, // TEXTURE2D
            0,
            0,
            0,
            4,
            0,
            4,
            0x0001_0001,
            DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32,
            1,
            0,
            0x100, // console LINEAR layout
            0,
            0,
        ];
        let tex_srv_host = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        let mut placed_view_words = [0u32; 16];
        // SAFETY: `D3D12_SHADER_RESOURCE_VIEW_DESC` is 40 bytes (`[u32; 10]`).
        placed_view_words[..10].copy_from_slice(&unsafe {
            std::ptr::from_ref(&tex_srv_host).cast::<[u32; 10]>().read()
        });
        placed_view_words[10] = placed_tex_addr as u32;
        placed_view_words[11] = (placed_tex_addr >> 32) as u32;
        create_placed_srv(
            std::ptr::null(),
            &placed_res_words,
            &placed_view_words,
            slot_handle(2),
        );
        // Slot 3: Buffer UAV.
        create_uav(
            std::ptr::null(),
            uav_res,
            std::ptr::null(),
            &raw_uav_words,
            slot_handle(3),
        );
        // Slot 6 -> copied to Slot 4 via `copy_simple`.
        create_srv(
            std::ptr::null(),
            copy_srv_res,
            &raw_srv_words,
            slot_handle(6),
        );
        copy_simple(std::ptr::null(), 1, slot_handle(4), slot_handle(6), 0);
        // Slot 5: Unmapped/null CBV (address = 0) -> must read 0 safely.
        create_cbv(std::ptr::null(), &[0, 0, 256, 0], slot_handle(5));

        let host_heap = heap(title_heap).host_arc().unwrap();
        // Before bindless preparation, `mirror(..., false)` deferred all
        // `View::is_buffer()` slots (`made[idx] == 0`).
        for idx in 0..=5 {
            assert_eq!(
                host_heap.made.lock().unwrap()[idx],
                0,
                "slot {idx} starts deferred"
            );
        }

        let (root, pso) = compile_probe_cs(&host.device);
        let list = host.new_list().unwrap();

        let record_probe = |list: &crate::host::HostList| {
            host_heap.prepare_bindless_record(list);
            list.defer_bindless(&host_heap, true);
            // SAFETY: open command list, shader-visible heap, and matching root/PSO.
            unsafe {
                let gpu_start = host_heap.heap.GetGPUDescriptorHandleForHeapStart().ptr;
                let gpu_at = |slot_idx: u32| D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu_start + u64::from(slot_idx) * u64::from(host_heap.step),
                };
                list.list
                    .SetDescriptorHeaps(&[Some(host_heap.heap.clone())]);
                list.list.SetComputeRootSignature(&root);
                list.list.SetPipelineState(&pso);
                list.list.SetComputeRootDescriptorTable(0, gpu_at(0));
                list.list.SetComputeRootDescriptorTable(1, gpu_at(1));
                list.list.SetComputeRootDescriptorTable(2, gpu_at(2));
                list.list.SetComputeRootDescriptorTable(3, gpu_at(3));
                list.list.SetComputeRootDescriptorTable(4, gpu_at(4));
                list.list.SetComputeRootDescriptorTable(5, gpu_at(5));
                list.list.Dispatch(1, 1, 1);
            }
            list.close();
        };

        record_probe(&list);
        host.execute(&[&list]);
        list.reset();

        // SAFETY: GPU finished and `Host::execute` copied UAV writes back to `uav_addr`.
        let out1 = unsafe { (uav_addr as *const [u32; 5]).read_volatile() };
        assert_eq!(
            out1[0], 13,
            "bindless buffer SRV materialized and refreshed"
        );
        assert_eq!(
            out1[1], 100,
            "16B-aligned bindless CBV materialized via bounce"
        );
        assert_eq!(
            out1[2], 0x7856_3412,
            "placed streamed texture uploaded and sampled"
        );
        assert_eq!(out1[3], 0xCAFE_1234, "copied bindless SRV materialized");
        assert_eq!(out1[4], 0, "null-address CBV safely reads zero");

        // Modify CPU memory for both the SRV buffer and the 16B-aligned CBV,
        // overwrite slot 5 with a `u64::MAX` garbage address CBV, and release
        // `copy_srv_res` so slot 4 goes stale -> both slots 4 and 5 read 0!
        create_cbv(
            std::ptr::null(),
            &[u32::MAX, u32::MAX, 256, 0],
            slot_handle(5),
        );
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        assert_eq!(release(copy_srv_res), 0);
        // SAFETY: offsets lie within `mem`.
        unsafe {
            (srv_addr as *mut u32).write_volatile(500);
            (cbv_16b_addr as *mut [u32; 4]).write_volatile([100, 200, 300, 400]);
        }

        // Released views are looked for once a frame.
        crate::queue::pass_frame();
        record_probe(&list);
        host.execute(&[&list]);
        list.reset();

        // SAFETY: GPU finished and copied back.
        let out2 = unsafe { (uav_addr as *const [u32; 5]).read_volatile() };
        assert_eq!(
            out2[0], 500,
            "dirty SRV title memory refreshed on second submit"
        );
        assert_eq!(
            out2[1], 1000,
            "dirty 16B-aligned CBV refreshed on second submit"
        );
        assert_eq!(out2[3], 0, "released resource slot pruned and reads zero");
        assert_eq!(
            out2[4], 0,
            "overflowing u64::MAX garbage address safely reads zero"
        );

        assert_eq!(release(srv_res), 0);
        assert_eq!(release(uav_res), 0);
        assert_eq!(release(title_heap), 0);
    }

    /// Submissions with nothing changed reuse a heap's bindless plan (no
    /// slot is made or range found again); writing a slot makes a new one,
    /// whose reads of one host copy are merged into one range.
    #[test]
    fn bindless_plan_is_remade_only_when_slots_change() {
        if host().is_none() {
            return;
        }
        let (title_heap, base) = make_shader_heap(8);
        let mem = crate::resource::Memory::zeroed(256 << 10, 64 << 10).unwrap();
        let cbv = |at: u64, slot: u64| {
            create_cbv(
                std::ptr::null(),
                &[at as u32, (at >> 32) as u32, 256, 0],
                base + slot * 32,
            );
        };
        cbv(mem.address(), 0);
        let host_heap = heap(title_heap).host_arc().unwrap();
        let first = host_heap.bindless_plan().unwrap();
        assert_eq!(first.reads.len(), 1);
        assert_eq!(first.reads[0].2, 256);
        assert!(first.writes.is_empty(), "constant buffers are not written");
        // Other tests may release resources meanwhile, which remakes plans.
        let reused = (0..100).any(|_| {
            let a = host_heap.bindless_plan().unwrap();
            Arc::ptr_eq(&a, &host_heap.bindless_plan().unwrap())
        });
        assert!(reused, "unchanged slots reuse the plan");

        // Slot 0 marked unmade: were it made again, it would be made anew.
        host_heap.made.lock().unwrap()[0] = 0;
        cbv(mem.address() + 0x1000, 1);
        let second = host_heap.bindless_plan().unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "a written slot remakes the plan"
        );
        assert_eq!(second.reads.len(), 1, "nearby reads merge");
        assert_eq!(second.reads[0].1, mem.address() as usize);
        assert_eq!(second.reads[0].2, 0x1100);
        assert_eq!(
            host_heap.made.lock().unwrap()[0],
            0,
            "only the written slot is made again"
        );
        assert_ne!(host_heap.made.lock().unwrap()[1], 0);

        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        assert_eq!(release(title_heap), 0);
    }

    /// Placed textures reached through slots upload the first time, then
    /// only after [`RECHECK_FRAMES`] (a few per frame), or when another
    /// texture takes the object's address.
    #[test]
    fn bindless_texture_uploads_are_throttled() {
        let Some(host) = host() else {
            return;
        };
        let Some(heap) = HostHeap::new(false, 4) else {
            return;
        };
        let (a, _) = host.shared_buffer(1 << 16).unwrap();
        let (b, _) = host.shared_buffer(1 << 16).unwrap();
        let tex = 0x1000 as crate::object::This;
        assert!(heap.upload_due(tex, &a, 10), "first reach uploads");
        assert!(!heap.upload_due(tex, &a, 10));
        assert!(!heap.upload_due(tex, &a, 10 + RECHECK_FRAMES - 1));
        assert!(heap.upload_due(tex, &b, 11), "a new texture at the address");
        assert!(heap.upload_due(tex, &b, 11 + RECHECK_FRAMES), "looked at again");
        // At most `RECHECKS` known textures per frame.
        let frame = 1000;
        let many: Vec<_> = (0..2 * RECHECKS as usize)
            .map(|i| {
                let t = (0x2000 + i) as crate::object::This;
                assert!(heap.upload_due(t, &a, 0));
                t
            })
            .collect();
        let again = many.iter().filter(|&&t| heap.upload_due(t, &a, frame)).count();
        assert_eq!(again, RECHECKS as usize);
    }

    /// A list queues each heap once, keeping whether any draw may write.
    #[test]
    fn bindless_heaps_queue_once_per_list() {
        let Some(host) = host() else {
            return;
        };
        let (title_heap, _) = make_shader_heap(8);
        let host_heap = heap(title_heap).host_arc().unwrap();
        let list = host.new_list().unwrap();
        list.defer_bindless(&host_heap, false);
        list.defer_bindless(&host_heap, true);
        list.defer_bindless(&host_heap, false);
        assert_eq!(list.bindless_queued(), vec![true]);
        list.close();
        host.execute(&[&list]);
        list.reset();
        assert!(list.bindless_queued().is_empty());

        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        assert_eq!(release(title_heap), 0);
    }

    /// Verifies `PLACED_TEXTURES` entries overlapping a remapped virtual range
    /// are evicted on `title_memory_remapped` while non-overlapping entries stay.
    #[test]
    fn placed_textures_evict_on_title_memory_remapped() {
        if host().is_none() {
            return;
        }
        let mem = crate::resource::Memory::zeroed(256 << 10, 64 << 10).unwrap();
        let addr_a = mem.address();
        let addr_b = mem.address() + (128 << 10);
        let desc: [u32; 14] = [
            3, // TEXTURE2D
            0,
            0,
            0,
            16,
            0,
            16,
            0x0001_0001,
            DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32,
            1,
            0,
            0x100, // LINEAR
            0,
            0,
        ];

        assert!(crate::resource::texture_at(addr_a, &desc, 0).is_some());
        assert!(crate::resource::texture_at(addr_b, &desc, 0).is_some());
        assert!(crate::resource::has_placed_texture(addr_a, &desc));
        assert!(crate::resource::has_placed_texture(addr_b, &desc));

        let epoch_before = crate::resource::placed_epoch();
        // Remap 256 bytes inside `addr_a`'s footprint (not even starting at `addr_a`!).
        crate::watch::title_memory_remapped((addr_a + 128) as usize, 256);

        assert!(
            !crate::resource::has_placed_texture(addr_a, &desc),
            "overlapping placed texture must be evicted"
        );
        assert!(
            crate::resource::has_placed_texture(addr_b, &desc),
            "non-overlapping placed texture must remain cached"
        );
        assert!(crate::resource::placed_epoch() > epoch_before);

        // Clean up `addr_b` too.
        crate::watch::title_memory_remapped(addr_b as usize, 64 << 10);
        assert!(!crate::resource::has_placed_texture(addr_b, &desc));
    }
}
