//! Draws on the host GPU (decision 0007, step 3). A list keeps the state
//! the title sets for its draws (pipeline, descriptor heaps, root
//! arguments, vertex and index buffers, targets); each draw then brings the
//! host list up to date: the host pipeline and our root signature, the
//! user data the translated shaders read, the host copies of the buffers
//! and descriptors they read, and the states of the textures involved.
//! State the host keeps across draws (viewports, scissors, blend factor,
//! stencil reference, depth bounds, topology, targets) is set on the host
//! list as the title sets it.

use std::sync::atomic::{AtomicU64, Ordering};

use victory_shader::{ResourceType, RootType, UsedResource};

use crate::descriptor::View;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::Interface;

use crate::host::HostList;
use crate::object::This;
use crate::pipeline::host::HostPipeline;

/// A root argument as the title set it.
#[derive(Debug, Clone, Default)]
enum Root {
    #[default]
    None,
    /// A descriptor table's first descriptor (an address in a heap).
    Table(u64),
    Constants(Vec<u32>),
    /// A root view's GPU address.
    View(u64),
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VertexView {
    address: u64,
    size: u32,
    stride: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct IndexView {
    address: u64,
    size: u32,
    format: u32,
}

/// Fixed-function state and the views bound for output.
#[derive(Default)]
struct Fixed {
    viewports: Vec<D3D12_VIEWPORT>,
    scissors: Vec<RECT>,
    blend: Option<Option<[f32; 4]>>,
    stencil: Option<u32>,
    views: Option<(Vec<crate::descriptor::RtvRecord>, Option<crate::descriptor::RtvRecord>)>,
}

// SAFETY: the views only name title resources.
unsafe impl Send for Fixed {}

/// Where the clamps of a run of indirect draws go: the list `clamps`, open,
/// that runs before the host list at `into`, as long as that one has
/// `writes` dispatches and copies (a later one may write the arguments).
pub(crate) struct Hoist {
    pub(crate) clamps: std::sync::Arc<HostList>,
    pub(crate) into: usize,
    pub(crate) writes: u64,
}

/// What one title list has set, and what its host list was last given.
#[derive(Default)]
pub(crate) struct State {
    /// The list before the host list recorded into that holds the clamps of
    /// its indirect draws (`crate::indirect::hoisting`), still open.
    hoist: Option<Hoist>,
    pipeline: usize,
    /// View heap, sampler heap.
    heaps: [usize; 2],
    graphics: Vec<Root>,
    compute: Vec<Root>,
    vertex: [VertexView; 32],
    index: IndexView,
    /// Bound render target and depth resources, for their host states.
    targets: Vec<usize>,
    depth: usize,
    /// None leaves the host list's default bounds (0..1) in place.
    depth_bounds: Option<(f32, f32)>,
    /// The title's primitive topology (the host's may differ).
    topology: u32,
    /// What was set on the host list at once, for the one a cut list goes on
    /// in.
    fixed: Fixed,
    /// What the host list has: pipeline, root signature, heaps.
    host_pipeline: usize,
    /// Graphics, compute.
    host_roots: [usize; 2],
    host_heaps: [usize; 2],
    /// Tables made afresh in this list, by a hash of the table and what it
    /// holds ([`table_hash`]).
    fresh_tables: std::collections::HashMap<u64, Fresh, std::hash::BuildHasherDefault<Plain>>,
    /// The record words of [`Fresh`] tables, one after the other.
    fresh_records: Vec<[u32; 3]>,
    /// Of them, large ones by table: the records it was made from and the
    /// first dynamic slot. A shader that reads less of one takes the same.
    large_tables: std::collections::HashMap<u64, Vec<(Vec<[u32; 3]>, u32)>>,
    /// What [`State::tables`] made for the stage being bound: root slot, byte
    /// offset of its fresh table. Kept to reuse its storage across draws.
    made: Vec<(u8, u32)>,
    /// The dynamic slots the list holds: title heap, first, count; given
    /// back when the list is reset or released.
    dynamic: Vec<(usize, u32, u32)>,
    /// Whether the last [`State::bind`] left a used unordered access view
    /// with nothing behind it (a null view).
    null_uav: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        for &(heap, first, n) in &self.dynamic {
            crate::descriptor::release_dynamic(heap, first, n);
        }
    }
}

/// Draws recorded on the host GPU since the last frame line.
static DRAWN: AtomicU64 = AtomicU64::new(0);

/// Draws run on the host so far.
pub(crate) fn drawn() -> u64 {
    DRAWN.load(Ordering::Relaxed)
}

/// The draws traced since the last present (diagnostics builds, with
/// debug logging for this module): logged when the frame comes out black.
#[cfg(feature = "diagnostics")]
static FRAME: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The draws traced since the last call.
#[cfg(feature = "diagnostics")]
pub(crate) fn take_frame() -> Vec<String> {
    std::mem::take(&mut *FRAME.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Draws, mesh dispatches and dispatches recorded but not run on the host
/// (no host pipeline) since the last call.
pub(crate) fn take_dropped() -> u64 {
    DROPPED.swap(0, Ordering::Relaxed)
}
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Groups the host runs per dimension of a dispatch (and of a mesh
/// dispatch). A count past it is undefined behaviour on the host; drivers
/// may run it for seconds and reset the display driver.
const MAX_GROUPS: u32 = 65_535;

/// Whether dispatches that write through a null unordered access view are
/// dropped (the default; `VICTORY_NULL_UAV_DISPATCH=1` runs them). Reads of
/// a null view give zeros and its atomics return zero, so a compute pass
/// that loops on a counter or flag kept there (work queues, spin waits)
/// never sees it move and runs to the translator's loop limit in every
/// lane: seconds of GPU time, and the host resets the display driver
/// (DXGI_ERROR_DEVICE_HUNG, 2026-11-24, in the world on an RX 7600; the
/// device-removed data named a dispatch with two null views in its table).
/// shortcut: what such a pass writes to its other views is lost too.
fn skip_null_uav_dispatches() -> bool {
    static SKIP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SKIP.get_or_init(|| std::env::var_os("VICTORY_NULL_UAV_DISPATCH").is_none_or(|v| v != "1"))
}

/// Logs, once per pipeline, a dispatch dropped for a null unordered access
/// view.
fn null_uav_dropped(pipeline: &str) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 256 && !seen.iter().any(|s| s == pipeline) {
        seen.push(pipeline.to_owned());
        tracing::warn!(pipeline, "a dispatch writing through a null unordered access view dropped");
    }
}

/// Whether the host can run a dispatch of `x * y * z` groups.
fn dispatch_fits(x: u32, y: u32, z: u32) -> bool {
    x <= MAX_GROUPS && y <= MAX_GROUPS && z <= MAX_GROUPS
}

/// Marks the last traced draw as not run on the host.
#[cfg(feature = "diagnostics")]
fn dropped(pipeline: usize) {
    crate::pipeline::note_drop(pipeline as This);
    if let Some(last) = FRAME.lock().unwrap_or_else(|e| e.into_inner()).last_mut() {
        last.push_str(" DROPPED");
    }
}

/// Dispatches run on the host so far.
static DISPATCHED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn dispatched() -> u64 {
    DISPATCHED.load(Ordering::Relaxed)
}

/// The host address of a root view: its buffer's host copy, or for a
/// constant buffer not 256-byte aligned there (the console needs 16) a
/// copy in upload memory.
/// shortcut: a root view has no size; constant buffers are taken as 64 KiB,
/// other views as 1 MiB (both cut at their buffer's end).
fn root_view(list: &HostList, address: u64, kind: u8) -> Option<u64> {
    let reach = if kind == 0 { 1 << 16 } else { 1 << 20 };
    let shadow = crate::shadow::find_reaching(address, reach)?;
    let mut len = shadow.left(address).min(reach);
    // In a sparse copy only the tiles asked for are there, and a shader that
    // reads a root view past them faults (the structure builders' views have
    // no length: Radeon GPU Detective, run rt10, a read at an address of no
    // resource). To the end of the title's buffer then, within reason.
    // shortcut: 64 MiB at most; a longer buffer is cut there.
    if kind != 0
        && shadow.is_sparse()
        && let Some(end) = crate::shadow::buffer_end(address)
    {
        len = len.max((end - address).min(64 << 20)).min(shadow.left(address));
    }
    let gpu = shadow.gpu(address);
    if kind != 0 || gpu % 256 == 0 {
        // The list holds the copy it gives the address of: the title may
        // release the buffer before it submits the list, and a retired copy
        // only outlives the submissions started by then and a few frames
        // (menus present a hundred a second). The structure builders' lists
        // read and wrote freed copies (page fault in a copy loop of root
        // views, Radeon GPU Detective, run rt2, 2026-11-25).
        if kind != 0 && list.first_use(shadow.buffer.as_raw() as usize) {
            list.keep(&shadow.buffer);
        }
        shadow.refresh(list, address, len);
        if kind == 2 {
            shadow.gpu_writes(list, address, len);
        }
        return Some(gpu);
    }
    list.fill_scratch(address as *const u8, len)
}

/// Tables from this many entries on are made once for every length shaders
/// read them to.
const LARGE: u32 = 64;

/// Root slots (bytes) as a set that lives on the stack: a draw collects them
/// without the heap, and they come out in order.
#[derive(Default)]
struct Slots([u64; 4]);

impl Slots {
    fn insert(&mut self, slot: u8) {
        self.0[usize::from(slot / 64)] |= 1 << (slot % 64);
    }

    fn contains(&self, slot: u8) -> bool {
        (self.0[usize::from(slot / 64)] >> (slot % 64)) & 1 != 0
    }

    fn iter(&self) -> impl Iterator<Item = u8> {
        let words = self.0;
        (0..4u8).flat_map(move |w| {
            let mut bits = words[usize::from(w)];
            std::iter::from_fn(move || {
                (bits != 0).then(|| {
                    let bit = bits.trailing_zeros() as u8;
                    bits &= bits - 1;
                    w * 64 + bit
                })
            })
        })
    }
}

/// A table made afresh: what it was made from (the `n` records at `at` in
/// [`State::fresh_records`]) and its first dynamic slot.
struct Fresh {
    table: u64,
    n: u32,
    at: usize,
    first: u32,
}

/// A hash that is mixed already, used as it is.
#[derive(Default)]
struct Plain(u64);

impl std::hash::Hasher for Plain {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0 << 8 | u64::from(b);
        }
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = x;
    }
}

/// A quick hash of `table` and its `n` records. It only finds the table
/// made before: the records are compared too, so a collision costs a copy,
/// not wrong descriptors.
fn table_hash(table: u64, n: u32) -> u64 {
    let mix = |h: u64, x: u64| (h.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95);
    let h = (0..n).fold(table, |h, k| {
        let [a, b, c] = record(table, k);
        mix(mix(h, u64::from(a) | u64::from(b) << 32), u64::from(c))
    });
    h ^ h >> 32
}

/// The words of entry `k` of `table`.
fn record(table: u64, k: u32) -> [u32; 3] {
    // SAFETY: a descriptor slot in title memory.
    unsafe { ((table + u64::from(k) * 32) as *const [u32; 3]).read_unaligned() }
}

/// The longest `table` of `heap` was asked for, now for `n` entries.
fn longest(heap: &crate::descriptor::Heap, table: u64, n: u32) -> u32 {
    static LONGEST: std::sync::Mutex<Option<std::collections::HashMap<u64, u32>>> =
        std::sync::Mutex::new(None);
    let mut all = LONGEST.lock().unwrap_or_else(|e| e.into_inner());
    let most = all.get_or_insert_default().entry(table).or_insert(n);
    *most = (*most).max(n);
    if heap.index(table + u64::from(*most - 1) * 32).is_some() { *most } else { n }
}

/// The last byte of the root view at `address` four dwords can be read from:
/// to the end of its host copy ([`root_view`]).
fn root_limit(address: u64) -> u32 {
    const REACH: u64 = 1 << 20;
    crate::shadow::find_reaching(address, REACH).map_or(0, |s| {
        s.left(address)
            .saturating_sub(16)
            .min(u64::from(u32::MAX) - 16) as u32
            & !3
    })
}

/// Why the last draw was not prepared.
static WHY_NOT: std::sync::Mutex<&'static str> = std::sync::Mutex::new("");

/// `false`, for `reason`.
fn no(reason: &'static str) -> bool {
    *WHY_NOT.lock().unwrap_or_else(|e| e.into_inner()) = reason;
    false
}

macro_rules! once {
    ($($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!($($arg)*));
    }};
}

fn set(roots: &mut Vec<Root>, slot: u32, root: Root) {
    let slot = slot as usize;
    if roots.len() <= slot {
        roots.resize(slot + 1, Root::None);
    }
    roots[slot] = root;
}

impl State {
    pub(crate) fn reset(&mut self, pipeline: This) {
        *self = State::default();
        self.pipeline = pipeline as usize;
    }

    /// The title's topology; console-only ones become host lists (see
    /// [`crate::primitive`]).
    pub(crate) fn set_topology(&mut self, list: &HostList, topology: u32) {
        self.topology = topology;
        let host = match topology {
            RECTANGLES => 4,
            TRIANGLE_FAN | LINE_LOOP | QUADS | QUAD_STRIP => {
                once!(topology, "console topology drawn as a triangle list");
                4
            }
            t => t,
        };
        // SAFETY: an open list.
        unsafe {
            list.list
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY(host as i32))
        };
    }

    pub(crate) fn set_pipeline(&mut self, pipeline: This) {
        self.pipeline = pipeline as usize;
    }

    pub(crate) fn set_heaps(&mut self, heaps: &[This]) {
        self.heaps = [0; 2];
        for &h in heaps {
            if !h.is_null() {
                let samplers = crate::descriptor::heap(h).samplers();
                self.heaps[usize::from(samplers)] = h as usize;
            }
        }
    }

    fn roots(&mut self, compute: bool) -> &mut Vec<Root> {
        if compute {
            &mut self.compute
        } else {
            &mut self.graphics
        }
    }

    pub(crate) fn set_table(&mut self, compute: bool, slot: u32, handle: u64) {
        set(self.roots(compute), slot, Root::Table(handle));
    }

    pub(crate) fn set_view(&mut self, compute: bool, slot: u32, address: u64) {
        set(self.roots(compute), slot, Root::View(address));
    }

    pub(crate) fn set_constants(&mut self, compute: bool, slot: u32, values: &[u32], offset: u32) {
        let roots = self.roots(compute);
        let s = slot as usize;
        if !matches!(roots.get(s), Some(Root::Constants(_))) {
            set(roots, slot, Root::Constants(Vec::new()));
        }
        if let Root::Constants(c) = &mut roots[s] {
            let end = offset as usize + values.len();
            if c.len() < end {
                c.resize(end, 0);
            }
            c[offset as usize..end].copy_from_slice(values);
        }
    }

    pub(crate) fn set_vertex_buffers(&mut self, start: u32, views: &[VertexView]) {
        for (i, v) in views.iter().enumerate() {
            if let Some(slot) = self.vertex.get_mut(start as usize + i) {
                *slot = *v;
            }
        }
    }

    pub(crate) fn clear_vertex_buffers(&mut self, start: u32, count: u32) {
        for slot in self.vertex.iter_mut().skip(start as usize).take(count as usize) {
            *slot = VertexView::default();
        }
    }

    pub(crate) fn set_index_buffer(&mut self, view: Option<&IndexView>) {
        self.index = view.copied().unwrap_or_default();
    }

    /// Whether the clamps of indirect draws recorded in `list` now go to
    /// the list before it.
    pub(crate) fn hoists_into(&self, list: &HostList) -> bool {
        self.hoist
            .as_ref()
            .is_some_and(|h| h.into == std::ptr::from_ref(list) as usize && h.writes == list.writes())
    }

    /// From here on the clamps of `into`'s indirect draws go to `clamps`.
    pub(crate) fn hoist(&mut self, clamps: std::sync::Arc<HostList>, into: &HostList) {
        self.end_hoist();
        // The clamped arguments live as long as the list that reads them.
        into.keep_until_run(Box::new(clamps.clone()));
        self.hoist = Some(Hoist {
            clamps,
            into: std::ptr::from_ref(into) as usize,
            writes: into.writes(),
        });
    }

    /// Ends and closes the list of hoisted clamps, if any.
    pub(crate) fn end_hoist(&mut self) {
        if let Some(h) = self.hoist.take() {
            h.clamps.clamp_end();
            h.clamps.close();
        }
    }

    /// Goes on in another host list, open: gives it the state set so far.
    pub(crate) fn continue_in(&mut self, list: &HostList) {
        self.host_pipeline = 0;
        self.host_roots = [0; 2];
        self.host_heaps = [0; 2];
        // Making one refreshes what it names, which is each list's own work.
        self.fresh_tables.clear();
        self.fresh_records.clear();
        self.large_tables.clear();
        if self.topology != 0 {
            self.set_topology(list, self.topology);
        }
        if !self.fixed.viewports.is_empty() {
            viewports(list, &self.fixed.viewports);
        }
        if !self.fixed.scissors.is_empty() {
            scissors(list, &self.fixed.scissors);
        }
        if let Some(factor) = &self.fixed.blend {
            blend_factor(list, factor.as_ref());
        }
        if let Some(reference) = self.fixed.stencil {
            stencil_ref(list, reference);
        }
        if let Some((targets, depth)) = self.fixed.views.take() {
            self.bind_targets(list, targets, depth);
        }
    }

    pub(crate) fn set_viewports(&mut self, list: &HostList, items: &[D3D12_VIEWPORT]) {
        self.fixed.viewports = items.to_vec();
        viewports(list, items);
    }

    pub(crate) fn set_scissors(&mut self, list: &HostList, rects: &[RECT]) {
        self.fixed.scissors = rects.to_vec();
        scissors(list, rects);
    }

    pub(crate) fn set_blend_factor(&mut self, list: &HostList, factor: Option<&[f32; 4]>) {
        self.fixed.blend = Some(factor.copied());
        blend_factor(list, factor);
    }

    pub(crate) fn set_stencil_ref(&mut self, list: &HostList, reference: u32) {
        self.fixed.stencil = Some(reference);
        stencil_ref(list, reference);
    }

    /// Render targets and depth, set on the host list at once.
    pub(crate) fn set_targets(&mut self, list: &HostList, handles: &[u64], depth: u64) {
        let mut targets = Vec::new();
        for &h in handles {
            match crate::descriptor::rtv_record(h) {
                Some(record) => targets.push(record),
                None => once!(
                    handle = format_args!("{h:#x}"),
                    "render target without a recorded view"
                ),
            }
        }
        self.bind_targets(list, targets, crate::descriptor::dsv_record(depth));
    }

    fn bind_targets(
        &mut self,
        list: &HostList,
        targets: Vec<crate::descriptor::RtvRecord>,
        depth: Option<crate::descriptor::RtvRecord>,
    ) {
        let Some(host) = crate::host::host() else {
            return;
        };
        self.targets.clear();
        let mut rtvs = Vec::new();
        for record in &targets {
            let Some(texture) = crate::resource::host_resource(record.resource) else {
                continue;
            };
            self.targets.push(record.resource as usize);
            #[cfg(feature = "diagnostics")]
            crate::capture::note_target(record.resource);
            rtvs.push(host.rtv(texture, record.desc().as_ref()));
        }
        self.depth = 0;
        let dsv = depth.and_then(|record| {
            let texture = crate::resource::host_resource(record.resource)?;
            self.depth = record.resource as usize;
            #[cfg(feature = "diagnostics")]
            crate::capture::note_target(record.resource);
            Some(host.dsv(texture, record.depth_desc().as_ref()))
        });
        // SAFETY: an open list; the views were just made.
        unsafe {
            list.list.OMSetRenderTargets(
                rtvs.len() as u32,
                (!rtvs.is_empty()).then_some(rtvs.as_ptr()),
                false,
                dsv.as_ref().map(std::ptr::from_ref),
            )
        };
        self.fixed.views = Some((targets, depth));
    }

    /// The user-data value of a root argument, as translated shaders read
    /// it: a table's byte offset in its heap, a constant's value.
    fn user_value(&self, compute: bool, slot: u8, root: RootType, dword: u32) -> u32 {
        match (self.roots_ref(compute).get(usize::from(slot)), root) {
            (Some(Root::Table(handle)), RootType::Table) => match self.heap_of(*handle) {
                Some(heap) => (handle - heap.start) as u32,
                None => {
                    once!(
                        handle = format_args!("{handle:#x}"),
                        "descriptor table outside the bound heaps"
                    );
                    0
                }
            },
            (Some(Root::View(address)), RootType::Descriptor) => (address >> (32 * dword.min(1))) as u32,
            (Some(Root::Constants(c)), RootType::Constants) => {
                c.get(dword as usize).copied().unwrap_or(0)
            }
            _ => {
                once!(
                    slot,
                    ?root,
                    "a shader reads a root argument the title did not set"
                );
                0
            }
        }
    }

    /// For shaders that reach memory by its address: the memory the roots
    /// lead to made reachable, and the bound heap's first slot of it (a
    /// heap of ours bound if the title bound none).
    fn memory(&mut self, list: &HostList, compute: bool) -> Option<u32> {
        // How much a root view or a pointer leads to is not known.
        const REACH: u64 = 1 << 20;
        for root in self.roots_ref(compute) {
            match root {
                Root::View(address) => crate::shadow::declare(list, *address, REACH),
                Root::Constants(words) => {
                    for w in words.windows(2) {
                        let address = u64::from(w[0]) | u64::from(w[1]) << 32;
                        if (1..=0xff).contains(&w[1]) && w[0] != u32::MAX && address % 4 == 0 {
                            crate::shadow::declare(list, address, REACH);
                        }
                    }
                    // Pointers over 256, as the acceleration structures'.
                    for &w in words {
                        if (0x50_0000..0xff_0000).contains(&(w >> 4)) {
                            crate::shadow::declare(list, u64::from(w) << 8, REACH);
                        }
                    }
                }
                _ => {}
            }
        }
        if self.heaps[0] != 0 {
            return Some(crate::descriptor::heap(self.heaps[0] as This).host()?.memory());
        }
        static OURS: std::sync::OnceLock<Option<crate::descriptor::host::HostHeap>> =
            std::sync::OnceLock::new();
        let heap = OURS
            .get_or_init(|| crate::descriptor::host::HostHeap::new(false, 16))
            .as_ref()?;
        const OURS_BOUND: [usize; 2] = [usize::MAX, 0];
        if self.host_heaps != OURS_BOUND {
            // SAFETY: an open list and a shader-visible heap.
            unsafe { list.list.SetDescriptorHeaps(&[Some(heap.heap.clone())]) };
            self.host_heaps = OURS_BOUND;
            self.host_roots = [0; 2];
        }
        Some(heap.memory())
    }

    /// After a dispatch of `p`: the least indirect light, if it is the
    /// program that resolves that.
    fn ambient(&self, list: &HostList, p: &crate::pipeline::host::HostPipeline) -> bool {
        let Some(stage) = p
            .stages
            .first()
            .filter(|s| s.shader.resolves_light || s.shader.ends_reflections)
        else {
            return false;
        };
        let view = |kind: ResourceType, (slot, offset): (u8, u32)| {
            let u = stage.translated().used.iter().find(|u| {
                u.kind == kind
                    && u.root == RootType::Table
                    && (u.slot, u.table_offset_dwords) == (slot, offset)
            })?;
            let Some(Root::Table(table)) = self.roots_ref(true).get(usize::from(u.slot)) else {
                return None;
            };
            crate::descriptor::view(table + u64::from(u.table_offset_dwords) * 4)
        };
        #[cfg(feature = "diagnostics")]
        {
            static LISTED: std::sync::Once = std::sync::Once::new();
            LISTED.call_once(|| {
                for u in stage.translated().used.iter().filter(|u| u.root == RootType::Table) {
                    let r = match view(u.kind, (u.slot, u.table_offset_dwords)) {
                        Some(View::Shader(r, _)) | Some(View::Unordered(r, ..)) => r,
                        _ => continue,
                    };
                    let Some(t) = crate::resource::host_resource(r as This) else { continue };
                    // SAFETY: a live resource.
                    let d = unsafe { t.GetDesc() };
                    tracing::info!(
                        kind = ?u.kind, slot = u.slot, offset = u.table_offset_dwords,
                        format = d.Format.0, width = d.Width, height = d.Height,
                        depth = d.DepthOrArraySize, flags = d.Flags.0,
                        "the light resolve reads"
                    );
                }
            });
        }
        if !stage.shader.resolves_light {
            // The reflections, held to the light around.
            return match view(ResourceType::Uav, crate::ambient::REFLECTIONS) {
                Some(View::Unordered(r, ..)) if r != 0 => crate::ambient::hold(list, r as This),
                _ => false,
            };
        }
        let light = match view(ResourceType::Uav, crate::ambient::LIGHT) {
            Some(View::Unordered(r, ..)) => r,
            _ => 0,
        };
        if light == 0 {
            once!("no ambient light: other tables");
            return false;
        }
        let depth = match view(ResourceType::Srv, crate::ambient::DEPTH) {
            Some(View::Shader(r, _)) => Some(r as This),
            _ => None,
        };
        crate::ambient::raise(list, light as This, depth)
    }

    /// The bound heap holding `handle`.
    fn heap_of(&self, handle: u64) -> Option<&'static crate::descriptor::Heap> {
        self.heaps
            .iter()
            .filter(|&&h| h != 0)
            .map(|&h| crate::descriptor::heap(h as This))
            .find(|h| h.index(handle).is_some())
    }

    pub(crate) fn set_depth_bounds(&mut self, list: &HostList, min: f32, max: f32) {
        self.depth_bounds = Some((min, max));
        if list.depth_bounds.is_none() {
            crate::host::warn_depth_bounds();
        }
        self.apply_depth_bounds(list);
    }

    fn apply_depth_bounds(&self, list: &HostList) {
        if let (Some(list), Some((min, max))) = (&list.depth_bounds, self.depth_bounds) {
            // SAFETY: the host supports depth bounds and the list is open.
            unsafe { list.OMSetDepthBounds(min, max) };
        }
    }

    /// Brings the host list up to date for a draw; false if the draw cannot
    /// run on the host.
    fn prepare(&mut self, list: &HostList, indexed: bool) -> bool {
        self.prepare_for(list, indexed, false)
    }

    /// As [`State::prepare`], for draws (`mesh` false) or mesh dispatches.
    fn prepare_for(&mut self, list: &HostList, indexed: bool, mesh: bool) -> bool {
        #[cfg(feature = "diagnostics")]
        if crate::pipeline::is_skipped(self.pipeline as This) {
            return no("skipped by name");
        }
        let Some(p) = crate::pipeline::host_pipeline(self.pipeline as This) else {
            return no("no host pipeline");
        };
        if p.compute || p.mesh != mesh {
            once!(
                mesh,
                "a draw or mesh dispatch with another kind of pipeline"
            );
            return no("another kind of pipeline");
        }
        if !self.bind(list, p, false) {
            return false;
        }
        if p.stages.iter().any(|s| s.shader.draws_sky)
            && let Some(&sky) = self.targets.first()
        {
            crate::ambient::sky_drawn(sky);
        }
        let l = &list.list;
        // Vertex buffers the input layout reads.
        let mut views = [D3D12_VERTEX_BUFFER_VIEW::default(); 32];
        let mut count = 0;
        for (slot, (view, &v)) in views.iter_mut().zip(&self.vertex).enumerate() {
            if p.slots & 1 << slot == 0 {
                continue;
            }
            count = slot + 1;
            if v.address == 0 || v.size == 0 {
                continue;
            }
            let Some(shadow) = crate::shadow::find(v.address, u64::from(v.size)) else {
                continue;
            };
            shadow.refresh(list, v.address, u64::from(v.size));
            *view = D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: shadow.gpu(v.address),
                SizeInBytes: v.size,
                StrideInBytes: v.stride,
            };
        }
        // SAFETY: an open list; the views name live host buffers.
        unsafe {
            if count > 0 {
                l.IASetVertexBuffers(0, Some(&views[..count]));
            }
        }
        if indexed {
            let v = self.index;
            let Some(shadow) = crate::shadow::find(v.address, u64::from(v.size)) else {
                return no("no index buffer");
            };
            shadow.refresh(list, v.address, u64::from(v.size));
            let view = D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: shadow.gpu(v.address),
                SizeInBytes: v.size,
                Format: DXGI_FORMAT(v.format as i32),
            };
            // SAFETY: an open list; the view names a live host buffer.
            unsafe { l.IASetIndexBuffer(Some(&view)) };
        }
        const RENDER_TARGET: u32 = 0x4;
        const DEPTH_WRITE: u32 = 0x10;
        for &t in &self.targets {
            crate::list::ensure_state(list, t as This, RENDER_TARGET);
        }
        if self.depth != 0 {
            crate::list::ensure_state(list, self.depth as This, DEPTH_WRITE);
        }
        true
    }

    /// A descriptor a shader reads through a table: its host slot made if
    /// it changed, constant data copied, textures made readable.
    /// What draws and dispatches share: heaps, our root signature, the
    /// pipeline, and per stage the user data, root views and the
    /// descriptors its tables hold.
    fn bind(&mut self, list: &HostList, p: &HostPipeline, compute: bool) -> bool {
        self.null_uav = false;
        let l = &list.list;
        let by_address = p.stages.iter().any(|s| {
            s.translated()
                .host
                .user_data
                .iter()
                .any(|u| u.root == RootType::MEMORY)
        });
        // Heaps first: a root signature with directly indexed heaps must be
        // set after them.
        let memory = by_address.then(|| self.memory(list, compute)).flatten();
        if by_address && memory.is_none() {
            return no("no view of memory");
        }
        if self.host_heaps != self.heaps && !(by_address && self.heaps[0] == 0) {
            let heaps: Vec<Option<ID3D12DescriptorHeap>> = self
                .heaps
                .iter()
                .filter(|&&h| h != 0)
                .filter_map(|&h| crate::descriptor::heap(h as This).host())
                .map(|h| Some(h.heap.clone()))
                .collect();
            heaps.iter().flatten().for_each(|h| list.keep(h));
            // SAFETY: an open list and shader-visible heaps.
            unsafe { l.SetDescriptorHeaps(&heaps) };
            self.host_heaps = self.heaps;
            self.host_roots = [0; 2];
        }
        let root = p.root.as_raw() as usize;
        if self.host_roots[usize::from(compute)] != root {
            // SAFETY: an open list and a live root signature.
            unsafe {
                if compute {
                    l.SetComputeRootSignature(&p.root);
                } else {
                    l.SetGraphicsRootSignature(&p.root);
                }
            }
            self.host_roots[usize::from(compute)] = root;
        }
        // By host object: a title pipeline released and another made at its
        // address is another host pipeline (the list keeps the old one).
        if self.host_pipeline != p.pso.as_raw() as usize {
            // SAFETY: an open list and a live pipeline.
            unsafe { l.SetPipelineState(&p.pso) };
            // Also reached after the indirect clamp invalidates the pipeline.
            if !compute {
                self.apply_depth_bounds(list);
            }
            self.host_pipeline = p.pso.as_raw() as usize;
            // The title may release the pipeline before it submits the list.
            list.bind_pipeline(p);
        }
        if p.stages.iter().any(|s| s.translated().resource_heap)
            && self.heaps[0] != 0
            && let Some(host_heap) = crate::descriptor::heap(self.heaps[0] as This).host_arc()
        {
            host_heap.prepare_bindless_record(list);
            let writes = p.stages.iter().any(|s| s.translated().writes_memory);
            list.defer_bindless(&host_heap, writes);
        }
        for stage in &p.stages {
            let t = stage.translated();
            if !self.tables(list, &t.used, compute) {
                return no("a table not made");
            }
            if let Some((param, dwords)) = stage.user_data {
                // A root signature holds at most 64 dwords, constants included.
                let mut stack = [0u32; 64];
                let values = &mut stack[..dwords as usize];
                for u in &t.host.user_data {
                    let fresh = self
                        .made
                        .iter()
                        .find(|m| m.0 == u.slot && u.root == RootType::Table);
                    if u.root == RootType::MEMORY {
                        values[u.index as usize] = memory.unwrap_or(0);
                        continue;
                    }
                    if u.root == RootType::THREADS {
                        values[u.index as usize] = u32::MAX;
                        continue;
                    }
                    if u.root == RootType::GROUPS {
                        continue;
                    }
                    if u.root == RootType::LIMIT {
                        values[u.index as usize] =
                            match self.roots_ref(compute).get(u.dword as usize) {
                                Some(Root::View(address)) => root_limit(*address),
                                _ => 0,
                            };
                        continue;
                    }
                    values[u.index as usize] = match fresh {
                        Some(&(_, offset)) => offset,
                        None => self.user_value(compute, u.slot, u.root, u.dword),
                    };
                }
                // SAFETY: an open list; `values` covers the parameter.
                unsafe {
                    if compute {
                        l.SetComputeRoot32BitConstants(param, dwords, values.as_ptr().cast(), 0);
                    } else {
                        l.SetGraphicsRoot32BitConstants(param, dwords, values.as_ptr().cast(), 0);
                    }
                }
            }
            for (views, kind) in [
                (&stage.root_cbvs, 0u8),
                (&stage.root_srvs, 1u8),
                (&stage.root_uavs, 2u8),
            ] {
                for &(slot, param) in views {
                    let Some(Root::View(address)) = self.roots_ref(compute).get(usize::from(slot))
                    else {
                        once!(slot, kind, "a shader reads a root view the title did not set");
                        return no("a root view not set");
                    };
                    let Some(gpu) = root_view(list, *address, kind) else {
                        once!(
                            slot,
                            kind,
                            address = format_args!("{address:#x}"),
                            "a shader reads a root view outside title buffers"
                        );
                        return no("a root view outside title buffers");
                    };
                    // SAFETY: an open list; the address is in live memory.
                    unsafe {
                        match (compute, kind) {
                            (false, 0) => l.SetGraphicsRootConstantBufferView(param, gpu),
                            (false, 1) => l.SetGraphicsRootShaderResourceView(param, gpu),
                            (false, _) => l.SetGraphicsRootUnorderedAccessView(param, gpu),
                            (true, 0) => l.SetComputeRootConstantBufferView(param, gpu),
                            (true, 1) => l.SetComputeRootShaderResourceView(param, gpu),
                            (true, _) => l.SetComputeRootUnorderedAccessView(param, gpu),
                        }
                    }
                }
            }
            if let Some(param) = stage.stride_table {
                let gpu = (self.heaps[0] != 0)
                    .then(|| crate::descriptor::heap(self.heaps[0] as This).host())
                    .flatten()
                    .and_then(|h| h.stride_table_gpu());
                if let Some(gpu) = gpu {
                    // SAFETY: an open list; the stride table buffer lives with the host heap.
                    unsafe {
                        if compute {
                            l.SetComputeRootShaderResourceView(param, gpu);
                        } else {
                            l.SetGraphicsRootShaderResourceView(param, gpu);
                        }
                    }
                } else {
                    once!("a shader reads the runtime stride table without a host view heap");
                }
            }
        }
        true
    }

    /// Runs a dispatch on the host.
    /// With `threads`, `x` counts threads, not groups (the console's
    /// `USE_THREAD_DIMENSIONS`).
    // shortcut: y and z stay groups; such dispatches are one-dimensional.
    pub(crate) fn dispatch(&mut self, list: &HostList, x: u32, y: u32, z: u32, threads: bool) {
        #[cfg(feature = "diagnostics")]
        if crate::pipeline::is_skipped(self.pipeline as This) {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            crate::pipeline::note_drop(self.pipeline as This);
            return;
        }
        let Some(p) = crate::pipeline::host_pipeline(self.pipeline as This) else {
            once!(
                pipeline = format_args!("{:#x}", self.pipeline),
                "a dispatch without a host pipeline dropped"
            );
            DROPPED.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "diagnostics")]
            crate::pipeline::note_drop(self.pipeline as This);
            #[cfg(feature = "diagnostics")]
            self.note_dropped(x, y, z);
            return;
        };
        if !p.compute {
            once!("a dispatch with a graphics pipeline");
            return;
        }
        let limit = p.stages.first().and_then(|s| {
            let data = &s.translated().host.user_data;
            let t = data.iter().find(|u| u.root == RootType::THREADS)?;
            Some((s.user_data?.0, t.index, t.dword))
        });
        let (x, most) = match limit {
            Some((_, _, group)) if threads => (x.div_ceil(group.max(1)), Some(x)),
            _ => (x, None),
        };
        if threads && limit.is_none() {
            once!(name = p.name, x, y, z, "a dispatch of threads runs as groups: dropped");
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // Programs that take their groups' start from us run in parts.
        let start = p.stages.first().and_then(|s| {
            let groups = s.translated().host.user_data.iter().find(|u| u.root == RootType::GROUPS);
            Some((s.user_data?.0, groups?.index))
        });
        if !dispatch_fits(if start.is_some() { 1 } else { x }, y, z) {
            #[cfg(feature = "diagnostics")]
            tracing::info!(name = p.name, x, "past the group limits");
            once!(x, y, z, "a dispatch past the host's group limits dropped");
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        crate::twin::begin();
        if !self.bind(list, p, true) {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.null_uav && skip_null_uav_dispatches() {
            null_uav_dropped(&p.name);
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        #[cfg(feature = "diagnostics")]
        list.note_op(|| self.op_text(format!("dispatch {x}x{y}x{z}"), true, false));
        #[cfg(feature = "diagnostics")]
        crate::capture::pass(list, self.targets.first().copied().unwrap_or(0), || self.op_text(format!("dispatch {x}x{y}x{z}"), true, true));
        if let (Some((param, index, _)), Some(most)) = (limit, most) {
            // SAFETY: an open list; a dword of the program's user data.
            unsafe { list.list.SetComputeRoot32BitConstant(param, most, index) };
        }
        // SAFETY: an open list with everything set.
        unsafe { list.list.Dispatch(x.min(MAX_GROUPS), y, z) };
        if let Some((param, index)) = start {
            let mut at = MAX_GROUPS;
            while at < x {
                // SAFETY: as before; a dword of the program's user data.
                unsafe {
                    list.list.SetComputeRoot32BitConstant(param, at, index);
                    list.list.Dispatch((x - at).min(MAX_GROUPS), y, z);
                }
                at += MAX_GROUPS;
            }
        }
        list.count_write();
        list.count_work();
        DISPATCHED.fetch_add(1, Ordering::Relaxed);
        // Depth textures it wrote as images get what it wrote; then the
        // title's state goes back on the list.
        let lit = self.ambient(list, p);
        if crate::twin::resolve(list) | lit {
            self.continue_in(list);
        }
    }

    /// Diagnostics (`VICTORY_DROPPED=<n>`): the roots of the first dispatches
    /// of each pipeline not run.
    #[cfg(feature = "diagnostics")]
    fn note_dropped(&self, x: u32, y: u32, z: u32) {
        use std::fmt::Write;
        static MOST: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let most = *MOST.get_or_init(|| {
            std::env::var("VICTORY_DROPPED").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
        });
        static SEEN: std::sync::Mutex<Option<std::collections::HashMap<usize, u32>>> =
            std::sync::Mutex::new(None);
        {
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            let n = seen.get_or_insert_default().entry(self.pipeline).or_default();
            if *n >= most {
                return;
            }
            *n += 1;
        }
        let mut text = String::new();
        for (i, root) in self.roots_ref(true).iter().enumerate() {
            match root {
                Root::View(a) => {
                    let host = crate::shadow::find_reaching(*a, 1)
                        .map(|s| if s.is_direct() { "in place" } else if s.is_sparse() { "sparse" } else { "copy" });
                    let _ = write!(text, " {i}:view {a:#x} {host:?}");
                }
                Root::Constants(w) => {
                    let _ = write!(text, " {i}:constants {w:x?}");
                }
                _ => {}
            }
        }
        tracing::info!("dropped {} {x}x{y}x{z}{text}", crate::pipeline::names(self.pipeline as This));
    }

    /// What a draw or dispatch runs, for the device-removed report
    /// (diagnostics): the pipeline, then per stage the user data it gets
    /// (with the view at that heap index, if one is), the host addresses
    /// of its root views and the views its tables name.
    #[cfg(feature = "diagnostics")]
    fn op_text(&self, kind: String, compute: bool, full: bool) -> String {
        use std::fmt::Write;
        let Some(p) = crate::pipeline::host_pipeline(self.pipeline as This) else {
            return kind;
        };
        let mut text = format!("{kind} {}", p.name);
        // What it reads takes far longer to write down than the draw.
        static FULL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !full && !*FULL.get_or_init(|| std::env::var_os("VICTORY_OP_TEXT").is_some()) {
            return text;
        }
        let views = (self.heaps[0] != 0)
            .then(|| crate::descriptor::heap(self.heaps[0] as This).host())
            .flatten();
        HEAP.store(self.heaps[0], Ordering::Relaxed);
        for stage in &p.stages {
            let t = stage.translated();
            let _ = write!(text, " | {} {}:", stage.shader.name(), stage.shader.hash);
            for u in &t.host.user_data {
                let v = self.user_value(compute, u.slot, u.root, u.dword);
                let _ = write!(text, " u{}={v:#x}", u.index);
                let index = if u.root == RootType::Table { v / 32 } else { v };
                if let Some(slot) = views.and_then(|h| h.describe_slot(index)) {
                    let _ = write!(text, " [heap {index}: {slot}]");
                }
            }
            for &(slot, _) in stage
                .root_cbvs
                .iter()
                .chain(&stage.root_srvs)
                .chain(&stage.root_uavs)
            {
                if let Some(Root::View(address)) = self.roots_ref(compute).get(usize::from(slot)) {
                    let host = crate::shadow::find(*address, 1)
                        .map(|s| {
                            let kind = if s.is_direct() {
                                "in place"
                            } else if s.is_sparse() {
                                "sparse"
                            } else {
                                "copy"
                            };
                            format!("{kind} {:#x} left {:#x}", s.gpu(*address), s.left(*address))
                        });
                    let _ = write!(text, " root{slot}={address:#x} host {host:?}");
                    let _ = write!(text, " holds {}", title_words(*address, 24));
                }
            }
        }
        if !compute && self.index.size != 0 {
            let v = self.index;
            let _ = write!(
                text,
                " index {:#x}+{:#x} f{} holds {}",
                v.address,
                v.size,
                v.format,
                title_words(v.address, 24)
            );
        }
        let _ = write!(text, " tables {:?}", self.reads(compute));
        if !compute {
            let _ = write!(text, " targets {:#x?} depth {:#x}", self.targets, self.depth);
        }
        text
    }

    /// Runs a mesh dispatch of `x` groups on the host (the title's mesh
    /// programs are one-dimensional).
    pub(crate) fn dispatch_mesh(&mut self, list: &HostList, x: u32) {
        #[cfg(feature = "diagnostics")]
        self.trace("mesh", x, 0);
        if x > MAX_GROUPS {
            once!(x, "a mesh dispatch past the host's group limit dropped");
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let ran = self.prepare_for(list, false, true)
            && match list.list.cast::<ID3D12GraphicsCommandList6>() {
                Ok(l6) => {
                    #[cfg(feature = "diagnostics")]
                    list.note_op(|| self.op_text(format!("mesh {x}"), false, false));
        #[cfg(feature = "diagnostics")]
        crate::capture::pass(list, self.targets.first().copied().unwrap_or(0), || self.op_text(format!("mesh {x}"), false, true));
                    // SAFETY: an open list with everything set.
                    unsafe { l6.DispatchMesh(x, 1, 1) };
                    list.count_work();
                    DRAWN.fetch_add(1, Ordering::Relaxed);
                    true
                }
                Err(_) => false,
            };
        if !ran {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            list.missed_draw();
            #[cfg(feature = "diagnostics")]
            dropped(self.pipeline);
        }
    }

    fn roots_ref(&self, compute: bool) -> &Vec<Root> {
        if compute {
            &self.compute
        } else {
            &self.graphics
        }
    }

    fn table_entry(&mut self, list: &HostList, u: &UsedResource, compute: bool) {
        let Some(Root::Table(table)) = self.roots_ref(compute).get(usize::from(u.slot)) else {
            return;
        };
        let handle = table + u64::from(u.table_offset_dwords) * 4;
        let Some(heap) = self.heap_of(handle) else {
            return;
        };
        let (Some(host), Some(index)) = (heap.host(), heap.index(handle)) else {
            return;
        };
        match host.mirror(index, handle, true) {
            Some(view) => readable(list, u, &view),
            None => self.null_uav |= u.kind == ResourceType::Uav,
        }
    }

    /// Gives tables with shader resources or constants slots owned by the
    /// command list. Later title descriptor writes cannot change what a
    /// recorded draw samples. Constants also get 256-byte-aligned storage,
    /// filled at submission (the host wants constant buffers 256-byte
    /// aligned, the console only 16). Identical descriptors share slots
    /// within a list. Returns the fresh tables' new byte offsets by root
    /// parameter in `self.made`, or `false` when the draw must be dropped (a
    /// constant table found no dynamic slots).
    fn tables(&mut self, list: &HostList, used: &[UsedResource], compute: bool) -> bool {
        let tables = |u: &&UsedResource| u.root == RootType::Table;
        // Taken out so `self` stays free to borrow; a draw dropped below
        // leaves it empty, and it grows again.
        let mut made = std::mem::take(&mut self.made);
        made.clear();
        let mut fresh = Slots::default();
        for u in used
            .iter()
            .filter(tables)
            .filter(|u| matches!(u.kind, ResourceType::Cbv | ResourceType::Srv))
        {
            fresh.insert(u.slot);
        }
        // Tables reaching past a host heap smaller than the title's (the
        // host's limit, 1,000,000 views on NVIDIA, 2,048 samplers anywhere)
        // are made afresh too: the title's slots there have no host slot of
        // their own.
        for u in used.iter().filter(tables) {
            if fresh.contains(u.slot) {
                continue;
            }
            let Some(&Root::Table(table)) = self.roots_ref(compute).get(usize::from(u.slot)) else {
                continue;
            };
            let end = table + u64::from(u.table_offset_dwords) * 4;
            let Some(heap) = self.heap_of(table) else {
                continue;
            };
            let (Some(host), Some(index)) = (heap.host(), heap.index(end)) else {
                continue;
            };
            if index >= host.count() {
                fresh.insert(u.slot);
            }
        }
        for u in used.iter().filter(tables) {
            if !fresh.contains(u.slot) {
                self.table_entry(list, u, compute);
            }
        }
        for slot in fresh.iter() {
            let Some(&Root::Table(table)) = self.roots_ref(compute).get(usize::from(slot)) else {
                continue;
            };
            let entries: Vec<&UsedResource> = used
                .iter()
                .filter(tables)
                .filter(|u| u.slot == slot)
                .collect();
            let Some(heap) = self.heap_of(table) else {
                continue;
            };
            let Some(host) = heap.host() else {
                continue;
            };
            let n = entries
                .iter()
                .map(|u| u.table_offset_dwords / 8 + 1)
                .max()
                .unwrap_or(1);
            let last = table + u64::from(n - 1) * 32;
            // Shaders read such a table to many lengths, and a copy for each
            // ran out of slots: the longest seen serves all.
            let n = if n >= LARGE { longest(heap, table, n) } else { n };
            if heap.index(last).is_none() {
                for u in entries {
                    self.table_entry(list, u, compute);
                }
                continue;
            }
            let records: Vec<[u32; 3]> = if n >= LARGE {
                (0..n)
                    // SAFETY: descriptor slots in title memory.
                    .map(|k| unsafe {
                        ((table + u64::from(k) * 32) as *const [u32; 3]).read_unaligned()
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let needed = entries
                .iter()
                .map(|u| u.table_offset_dwords as usize / 8 + 1)
                .max()
                .unwrap_or(1);
            let same = self.large_tables.get(&table).and_then(|made| {
                made.iter()
                    .find(|(r, _)| r.len() >= needed && r[..needed] == records[..needed.min(records.len())])
                    .map(|&(_, first)| first)
            });
            if n >= LARGE
                && let Some(first) = same
            {
                for u in entries.iter().filter(|u| u.kind != ResourceType::Cbv) {
                    let handle = table + u64::from(u.table_offset_dwords) * 4;
                    match crate::descriptor::view(handle) {
                        Some(view) => readable(list, u, &view),
                        None => self.null_uav |= u.kind == ResourceType::Uav,
                    }
                }
                made.push((slot, first * 32));
                continue;
            }
            // The table and what its entries hold (record words).
            let key = table_hash(table, n);
            let same = self.fresh_tables.get(&key).filter(|f| {
                f.table == table
                    && f.n == n
                    && self.fresh_records[f.at..f.at + n as usize]
                        .iter()
                        .enumerate()
                        .all(|(k, r)| *r == record(table, k as u32))
            });
            if let Some(&Fresh { first, .. }) = same {
                for u in entries.iter().filter(|u| u.kind != ResourceType::Cbv) {
                    let handle = table + u64::from(u.table_offset_dwords) * 4;
                    match crate::descriptor::view(handle) {
                        Some(view) => readable(list, u, &view),
                        None => self.null_uav |= u.kind == ResourceType::Uav,
                    }
                }
                made.push((slot, first * 32));
                continue;
            }
            let Some(first) = host.dynamic(n) else {
                // A table holding constants made from the title's slots
                // would view them misaligned: the draw is dropped. Other
                // tables fall back to the title's slots.
                if entries.iter().any(|u| u.kind == ResourceType::Cbv) {
                    once!(n, "a constant table did not fit in command-list slots; draw dropped");
                    return false;
                }
                once!(n, "descriptor table did not fit in command-list slots");
                for u in entries {
                    self.table_entry(list, u, compute);
                }
                continue;
            };
            let at = self.fresh_records.len();
            self.fresh_records.extend((0..n).map(|k| record(table, k)));
            self.fresh_tables.insert(key, Fresh { table, n, at, first });
            if n >= LARGE {
                self.large_tables.entry(table).or_default().push((records, first));
            }
            self.dynamic
                .push((std::ptr::from_ref(heap) as usize, first, n));
            // The whole range: a shader may index past the entries its
            // used-resource list names (descriptor arrays).
            for k in 0..n {
                let index = first + k;
                let used = entries.iter().find(|u| u.table_offset_dwords / 8 == k);
                match crate::descriptor::view(table + u64::from(k) * 32) {
                    Some(View::Constant(address, size)) => {
                        let Some((gpu, to)) = list.scratch(u64::from(size)) else {
                            host.make_null_kind(index, ResourceType::Cbv, 4);
                            continue;
                        };
                        list.defer_fill(to, address as *const u8, size as usize);
                        host.constant_at(index, gpu, size);
                    }
                    Some(view) => {
                        host.make_at(index, &view);
                        if let Some(u) = used {
                            readable(list, u, &view);
                        }
                    }
                    None => {
                        if let Some(u) = used {
                            host.make_null_kind(index, u.kind, u.dwords);
                            once!(slot, "a table entry without a view");
                            self.null_uav |= u.kind == ResourceType::Uav;
                        } else {
                            host.make_null(index);
                        }
                    }
                }
            }
            made.push((slot, first * 32));
        }
        self.made = made;
        true
    }

    /// The first draws, at debug level: what they bind and read
    /// (diagnostics builds).
    #[cfg(feature = "diagnostics")]
    fn trace(&self, kind: &str, count: u32, first: u32) {
        static TRACED: AtomicU64 = AtomicU64::new(0);
        if !tracing::enabled!(tracing::Level::DEBUG) {
            return;
        }
        let listed = crate::capture::listed(crate::queue::presented() + 1);
        let logged = listed || TRACED.fetch_add(1, Ordering::Relaxed) < 64;
        let targets: Vec<String> = self
            .targets
            .iter()
            .filter_map(|&t| crate::resource::host_resource(t as This))
            .map(|t| {
                // SAFETY: a live resource.
                let d = unsafe { t.GetDesc() };
                format!("{}x{} f{}", d.Width, d.Height, d.Format.0)
            })
            .collect();
        let v = self.vertex[0];
        let line = format!(
            "{kind} {count} from {first} topology {} pipeline {:#x} targets {targets:?} vb {:#x}+{} stride {} reads {:?}",
            self.topology,
            self.pipeline,
            v.address,
            v.size,
            v.stride,
            self.reads(false)
        );
        if logged {
            tracing::debug!("draw {line}");
        }
        let mut frame = FRAME.lock().unwrap_or_else(|e| e.into_inner());
        if frame.len() < 256 {
            frame.push(line);
        }
    }

    /// What a draw's shaders read, one line per used resource (diagnostics
    /// builds).
    #[cfg(feature = "diagnostics")]
    fn reads(&self, compute: bool) -> Vec<String> {
        let Some(p) = crate::pipeline::host_pipeline(self.pipeline as This) else {
            return vec!["no host pipeline".into()];
        };
        let mut out = Vec::new();
        for stage in &p.stages {
            let t = stage.translated();
            for u in &t.used {
                let what = match (u.root, self.roots_ref(compute).get(usize::from(u.slot))) {
                    (RootType::Table, Some(Root::Table(table))) => {
                        let handle = table + u64::from(u.table_offset_dwords) * 4;
                        describe(crate::descriptor::view(handle).as_ref())
                    }
                    (root, _) => format!("{root:?}"),
                };
                out.push(format!(
                    "{} {:?} s{}+{}: {what}",
                    stage.shader.hash, u.kind, u.slot, u.table_offset_dwords
                ));
            }
        }
        out
    }

    pub(crate) fn draw(
        &mut self,
        list: &HostList,
        vertices: u32,
        instances: u32,
        first: u32,
        first_instance: u32,
    ) {
        #[cfg(feature = "diagnostics")]
        self.trace("draw", vertices, first);
        let rects = self.topology == RECTANGLES && !self.draws_rectangles();
        let ran = self.prepare(list, false) && (!rects || self.rectangles(list, vertices, first));
        if ran {
            // Rectangles were expanded into their own vertex buffers.
            let (count, from) = if rects {
                (vertices / 3 * 6, 0)
            } else {
                (vertices, first)
            };
            #[cfg(feature = "diagnostics")]
            list.note_op(|| self.op_text(format!("draw {count}x{instances}"), false, false));
        #[cfg(feature = "diagnostics")]
        crate::capture::pass(list, self.targets.first().copied().unwrap_or(0), || self.op_text(format!("draw {count}x{instances}"), false, true));
            // SAFETY: an open list with everything set.
            unsafe {
                list.list
                    .DrawInstanced(count, instances, from, first_instance)
            };
            list.count_work();
            DRAWN.fetch_add(1, Ordering::Relaxed);
        }
        if !ran {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            list.missed_draw();
            #[cfg(feature = "diagnostics")]
            dropped(self.pipeline);
        }
    }

    /// The pipeline takes rectangle lists as they are.
    fn draws_rectangles(&self) -> bool {
        crate::pipeline::host_pipeline(self.pipeline as This).is_some_and(|p| p.rectangles)
    }

    /// Binds vertex buffers holding the rectangles of vertices `first..first
    /// + count` as triangles, made when the list is submitted.
    fn rectangles(&self, list: &HostList, count: u32, first: u32) -> bool {
        let Some(p) = crate::pipeline::host_pipeline(self.pipeline as This) else {
            return false;
        };
        let rects = u64::from(count / 3);
        for slot in 0..32 {
            let elements: Vec<(u32, u32)> = p
                .inputs
                .iter()
                .filter(|e| e.slot == slot && !e.per_instance)
                .map(|e| (e.offset, e.format))
                .collect();
            if elements.is_empty() {
                continue;
            }
            let v = self.vertex[slot as usize];
            let stride = u64::from(v.stride);
            let from = v.address + u64::from(first) * stride;
            if v.address == 0
                || stride == 0
                || u64::from(first + count) * stride > u64::from(v.size)
                || crate::shadow::find(from, rects * 3 * stride).is_none()
            {
                once!(slot, "rectangle vertices outside their buffer");
                return false;
            }
            let Some((gpu, to)) = list.scratch(rects * 6 * stride) else {
                return false;
            };
            let (from, to, stride) = (from as usize, to as usize, stride as usize);
            let n = rects as usize;
            list.defer_host(move || {
                // SAFETY: the vertices lie in a title buffer (checked when
                // recorded) and the scratch memory is the list's.
                let (src, dst) = unsafe {
                    (
                        std::slice::from_raw_parts(from as *const u8, n * 3 * stride),
                        std::slice::from_raw_parts_mut(to as *mut u8, n * 6 * stride),
                    )
                };
                crate::primitive::expand_rects(src, dst, stride, &elements);
            });
            let view = D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: gpu,
                SizeInBytes: (rects * 6 * stride as u64) as u32,
                StrideInBytes: v.stride,
            };
            // SAFETY: an open list; the view names live scratch memory.
            unsafe { list.list.IASetVertexBuffers(slot, Some(&[view])) };
        }
        true
    }

    pub(crate) fn draw_indexed(
        &mut self,
        list: &HostList,
        indices: u32,
        instances: u32,
        first: u32,
        base: i32,
        first_instance: u32,
    ) {
        #[cfg(feature = "diagnostics")]
        self.trace("indexed", indices, first);
        let rects = self.topology == RECTANGLES && !self.draws_rectangles();
        if rects {
            once!("indexed rectangles not drawn");
        }
        let ran = !rects && self.prepare(list, true);
        if ran {
            #[cfg(feature = "diagnostics")]
            list.note_op(|| self.op_text(format!("indexed {indices}x{instances}"), false, false));
        #[cfg(feature = "diagnostics")]
        crate::capture::pass(list, self.targets.first().copied().unwrap_or(0), || self.op_text(format!("indexed {indices}x{instances}"), false, true));
            // SAFETY: an open list with everything set.
            unsafe {
                list.list
                    .DrawIndexedInstanced(indices, instances, first, base, first_instance)
            };
            list.count_work();
            DRAWN.fetch_add(1, Ordering::Relaxed);
        }
        if !ran {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            list.missed_draw();
            #[cfg(feature = "diagnostics")]
            dropped(self.pipeline);
        }
    }
}

impl State {
    /// For an indirect draw of rectangles: every per-vertex buffer the
    /// pipeline reads expanded into triangles, and the records rewritten
    /// for them, both on the GPU ([`crate::rects`]). Records before the
    /// pipeline is bound (it replaces it). `None` if this draw cannot be
    /// expanded (indexed, unaligned, too many elements, no pipeline).
    #[allow(clippy::type_complexity)]
    fn indirect_rectangles(
        &self,
        list: &HostList,
        kind: u32,
        bounded: &ID3D12Resource,
        stride: u32,
        max: u32,
    ) -> Option<(ID3D12Resource, Vec<(u32, D3D12_VERTEX_BUFFER_VIEW)>)> {
        if kind != 0 {
            return None;
        }
        let host = crate::host::host()?;
        let p = crate::pipeline::host_pipeline(self.pipeline as This)?;
        if p.compute || p.mesh {
            return None;
        }
        let mut views = Vec::new();
        for slot in 0..32u32 {
            let elements: Vec<(u32, u32)> = p
                .inputs
                .iter()
                .filter(|e| e.slot == slot && !e.per_instance)
                .map(|e| (e.offset, e.format))
                .collect();
            if elements.is_empty() {
                continue;
            }
            let v = self.vertex[slot as usize];
            if v.address == 0 || v.size == 0 || v.stride == 0 {
                return None;
            }
            let shadow = crate::shadow::find(v.address, u64::from(v.size))?;
            shadow.refresh(list, v.address, u64::from(v.size));
            let (_, gpu, size) =
                crate::rects::vertices(list, host, shadow.gpu(v.address), u64::from(v.size), v.stride, &elements)?;
            views.push((
                slot,
                D3D12_VERTEX_BUFFER_VIEW {
                    BufferLocation: gpu,
                    SizeInBytes: size,
                    StrideInBytes: v.stride,
                },
            ));
        }
        // The clamped records (`crate::indirect::bound`'s scratch, from 0).
        // SAFETY: a live buffer.
        let from = unsafe { bounded.GetGPUVirtualAddress() };
        let records = crate::rects::records(list, host, bounded, from, stride, max)?;
        Some((records, views))
    }

    /// Indirect execution of a one-argument signature of `kind` (PC
    /// numbering: 0 draw, 1 indexed draw, 2 dispatch, 10 mesh dispatch) with
    /// records `stride` bytes apart at title address `arguments`, and the
    /// record count at `count` if the title gives one: the GPU reads them
    /// from the buffer's host copy, where earlier dispatches may have
    /// written them. False if it cannot run on the host.
    pub(crate) fn execute_indirect(
        &mut self,
        list: &HostList,
        (kind, stride, max): (u32, u32, u32),
        arguments: u64,
        count: Option<u64>,
    ) -> bool {
        // Skip before the argument clamp can mutate the indirect buffer.
        #[cfg(feature = "diagnostics")]
        if crate::pipeline::is_skipped(self.pipeline as This) {
            crate::pipeline::note_drop(self.pipeline as This);
            return false;
        }
        const DRAW: u32 = 0;
        const INDEXED: u32 = 1;
        const DISPATCH: u32 = 2;
        const MESH: u32 = 10;
        #[cfg(feature = "diagnostics")]
        let why = |what: &str| crate::pipeline::note_why(format!("indirect kind {kind}: {what}"));
        #[cfg(not(feature = "diagnostics"))]
        let why = |_: &str| {};
        let Some(p) = crate::pipeline::host_pipeline(self.pipeline as This) else {
            #[cfg(feature = "diagnostics")]
            crate::pipeline::note_drop(self.pipeline as This);
            return false;
        };
        let matches_kind = match kind {
            DRAW | INDEXED => !p.compute && !p.mesh,
            MESH => !p.compute && p.mesh,
            DISPATCH => p.compute,
            _ => false,
        };
        if !matches_kind {
            why(&format!(
                "pipeline mismatch (compute {}, mesh {}, topology {})",
                p.compute, p.mesh, self.topology
            ));
            return false;
        }
        let len = u64::from(stride) * u64::from(max.max(1));
        let Some(args) = crate::shadow::find(arguments, len) else {
            why("arguments outside title buffers");
            return false;
        };
        let counter = match count {
            Some(at) => match crate::shadow::find(at, 4) {
                Some(s) => Some((s, at)),
                None => {
                    why("count outside title buffers");
                    return false;
                }
            },
            None => None,
        };
        let Some(signature) = matches!(kind, DRAW | INDEXED | DISPATCH | MESH)
            .then(|| host_signature(kind, stride))
            .flatten()
        else {
            why("no host signature");
            return false;
        };
        if let Some((s, at)) = &counter {
            s.refresh(list, *at, 4);
        }
        let count_arg = counter.as_ref().map(|(s, at)| (&s.buffer, s.offset(*at)));
        // Draws of a run: clamped in the list before, which the run's first
        // draw opened (`crate::list`), so that nothing of ours comes between
        // the draws.
        let before = (kind != DISPATCH
            && match crate::indirect::hoist_mode() {
                0 | 6 => false,
                3 => kind == MESH,
                4 => kind != MESH,
                _ => true,
            }
            && !(matches!(kind, DRAW | INDEXED) && self.topology == RECTANGLES)
            && self.hoists_into(list))
        .then(|| self.hoist.as_ref().map(|h| h.clamps.clone()))
        .flatten()
        .and_then(|clamps| {
            args.refresh(&clamps, arguments, len);
            if let Some((s, at)) = &counter {
                s.refresh(&clamps, *at, 4);
            }
            #[cfg(feature = "diagnostics")]
            clamps.note_op(|| format!("hoisted clamp of kind {kind}"));
            crate::indirect::bound_before(
                &clamps,
                (&args.buffer, args.offset(arguments)),
                count_arg,
                (kind, stride, max),
            )
        });
        let inline = before.is_none();
        let (bounded, bounded_at) = match before {
            Some(b) => b,
            None => {
                args.refresh(list, arguments, len);
                // Bounded first: the clamp binds a compute pipeline of its own and
                // copies clamped active records into a scratch buffer at offset 0.
                #[cfg(feature = "diagnostics")]
                list.note_op(|| format!("clamp of kind {kind}"));
                let Some(bounded) = crate::indirect::bound(
                    list,
                    (&args.buffer, args.offset(arguments)),
                    count_arg,
                    (kind, stride, max),
                ) else {
                    why("not bounded (no clamp, or arguments not 4-byte aligned)");
                    return false;
                };
                (bounded, 0)
            }
        };
        // Rectangle lists: their vertices expanded and the records rewritten
        // on the GPU, before the draw's own pipeline is bound.
        let mut rect_args: Option<ID3D12Resource> = None;
        let mut rect_vertices: Vec<(u32, D3D12_VERTEX_BUFFER_VIEW)> = Vec::new();
        if matches!(kind, DRAW | INDEXED) && self.topology == RECTANGLES && !self.draws_rectangles() {
            match self.indirect_rectangles(list, kind, &bounded, stride, max) {
                Some((a, views)) => {
                    rect_args = Some(a);
                    rect_vertices = views;
                }
                None => {
                    crate::indirect::done(list, &bounded);
                    why("a rectangle list (not expanded: indexed, unaligned or unknown inputs)");
                    return false;
                }
            }
        }
        if inline {
            self.host_pipeline = 0;
            self.host_roots[1] = 0;
        }
        let ran = match kind {
            DRAW if rect_args.is_some() => {
                let ok = self.prepare(list, false);
                if ok {
                    for (slot, view) in &rect_vertices {
                        // SAFETY: an open list; the view names a buffer the list keeps.
                        unsafe { list.list.IASetVertexBuffers(*slot, Some(&[*view])) };
                    }
                }
                ok
            }
            DRAW | INDEXED => {
                (self.topology != RECTANGLES || self.draws_rectangles())
                    && self.prepare(list, kind == INDEXED)
            }
            MESH => self.prepare_for(list, false, true),
            DISPATCH => match crate::pipeline::host_pipeline(self.pipeline as This) {
                Some(p) if p.compute => {
                    crate::twin::begin();
                    if !self.bind(list, p, true) {
                        false
                    } else {
                        let skip = self.null_uav && skip_null_uav_dispatches();
                        if skip {
                            null_uav_dropped(&p.name);
                        }
                        !skip
                    }
                }
                _ => false,
            },
            _ => false,
        };
        if !ran {
            if inline {
                crate::indirect::done(list, &bounded);
            }
            #[cfg(feature = "diagnostics")]
            match crate::pipeline::host_pipeline(self.pipeline as This) {
                None => crate::pipeline::note_drop(self.pipeline as This),
                Some(p) => why(&format!(
                    "prepare failed: {} (topology {}, vertex slots {:#x}, {})",
                    std::mem::take(&mut *WHY_NOT.lock().unwrap_or_else(|e| e.into_inner())),
                    self.topology,
                    p.slots,
                    p.name
                )),
            }
            return false;
        }
        #[cfg(feature = "diagnostics")]
        list.note_op(|| {
            self.op_text(
                format!("indirect kind {kind} max {max} at {arguments:#x}"),
                kind == DISPATCH,
                false,
            )
        });
        #[cfg(feature = "diagnostics")]
        crate::capture::pass(list, self.targets.first().copied().unwrap_or(0), || {
            self.op_text(
                format!(
                    "indirect kind {kind} max {max} stride {stride} at {arguments:#x} args {} all {}",
                    crate::shadow::args_text(arguments),
                    title_words(arguments, 8)
                ),
                kind == DISPATCH,
                true,
            )
        });
        let arg_buffer = rect_args.as_ref().unwrap_or(&bounded);
        if !inline && crate::indirect::hoist_mode() == 2 {
            let barrier = D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                ..Default::default()
            };
            // SAFETY: an open list; the barrier names no resource.
            unsafe { list.list.ResourceBarrier(&[barrier]) };
        }
        // SAFETY: an open list with everything set; the scratch buffers and
        // host copies are kept alive by the list, and the signature is cached.
        unsafe {
            list.list.ExecuteIndirect(
                &signature,
                max,
                arg_buffer,
                bounded_at,
                counter.as_ref().map(|(s, _)| &s.buffer),
                counter.as_ref().map_or(0, |(s, at)| s.offset(*at)),
            )
        };
        if inline {
            crate::indirect::done(list, &bounded);
        }
        if kind == DISPATCH {
            list.count_write();
            list.count_work();
            DISPATCHED.fetch_add(1, Ordering::Relaxed);
            let lit = self.ambient(list, p);
            if crate::twin::resolve(list) | lit {
                self.continue_in(list);
            }
        } else {
            list.count_work();
            DRAWN.fetch_add(1, Ordering::Relaxed);
        }
        true
    }
}

/// The host command signature for one argument of `kind` with records
/// `stride` bytes apart (no root arguments, so no root signature), made
/// `n` dwords of title memory at `address` (diagnostics).
#[cfg(feature = "diagnostics")]
fn title_words(address: u64, n: usize) -> String {
    let mut words = vec![0u32; n];
    // SAFETY: plain words.
    let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), 4 * n) };
    if !crate::host::read_into(address as *const u8, bytes) {
        return "unreadable".into();
    }
    format!("{words:x?}")
}

/// once.
fn host_signature(kind: u32, stride: u32) -> Option<ID3D12CommandSignature> {
    static MADE: std::sync::Mutex<Vec<((u32, u32), ID3D12CommandSignature)>> =
        std::sync::Mutex::new(Vec::new());
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, s)) = made.iter().find(|(k, _)| *k == (kind, stride)) {
        return Some(s.clone());
    }
    let argument = D3D12_INDIRECT_ARGUMENT_DESC {
        Type: D3D12_INDIRECT_ARGUMENT_TYPE(kind as i32),
        ..Default::default()
    };
    let desc = D3D12_COMMAND_SIGNATURE_DESC {
        ByteStride: stride,
        NumArgumentDescs: 1,
        pArgumentDescs: &argument,
        NodeMask: 0,
    };
    let mut signature: Option<ID3D12CommandSignature> = None;
    // SAFETY: a description of one argument; no root arguments.
    let result = unsafe {
        crate::host::host()?.device.CreateCommandSignature(
            &desc,
            None::<&ID3D12RootSignature>,
            &mut signature,
        )
    };
    if let Err(e) = result {
        once!(error = %e, kind, stride, "host command signature refused");
        return None;
    }
    let signature = signature?;
    made.push(((kind, stride), signature.clone()));
    Some(signature)
}

/// Gets what a shader reads or writes through a view ready: buffer data
/// copied, textures moved to the state they need.
fn readable(list: &HostList, u: &UsedResource, view: &View) {
    const SHADER_RESOURCE: u32 = 0x40 | 0x80;
    const UNORDERED_ACCESS: u32 = 0x8;
    let placed = crate::descriptor::host::placed_texture(view);
    let view = placed.as_ref().unwrap_or(view);
    // Buffer views read title memory: their range is copied at submission;
    // what shaders write through unordered access goes back to it after.
    if let Some((address, len)) = crate::descriptor::host::buffer_range(view)
        && len > 0
        && let Some(shadow) = crate::shadow::find(address, len)
    {
        shadow.refresh(list, address, len);
        if u.kind == ResourceType::Uav {
            shadow.gpu_writes(list, address, len);
        }
    }
    match (view, u.kind) {
        (View::Shader(resource, _), ResourceType::Srv) => {
            if let Some(t) = crate::resource::host_resource(*resource as This)
                && crate::texture::title_written(t)
            {
                crate::texture::upload(list, *resource as This, t);
            }
            crate::list::ensure_state(list, *resource as This, SHADER_RESOURCE);
            #[cfg(feature = "diagnostics")]
            if let Some(t) = crate::resource::host_resource(*resource as This) {
                // SAFETY: a live resource.
                let d = unsafe { t.GetDesc() };
                if d.Flags.0 & 7 == 0 {
                    tracing::debug!(
                        resource = format_args!("{resource:#x}"),
                        ?d,
                        "a shader reads a texture the title fills"
                    );
                }
            }
        }
        (View::Unordered(resource, _, desc), ResourceType::Uav) => {
            #[cfg(feature = "diagnostics")]
            crate::capture::note_target(*resource as This);
            // Written through its twins, copied in after the dispatch.
            if crate::resource::depth_twin(*resource as This).is_some() {
                crate::twin::note(*resource as This, desc.as_ref());
                return;
            }
            crate::list::ensure_state(list, *resource as This, UNORDERED_ACCESS);
        }
        _ => {}
    }
}

/// A view in a few words (diagnostics builds).
#[cfg(feature = "diagnostics")]
fn describe(view: Option<&View>) -> String {
    let texture = |r: This| {
        crate::resource::host_resource(r).map(|t| {
            // SAFETY: a live resource.
            let d = unsafe { t.GetDesc() };
            format!(
                "{}x{} f{} flags {:#x} data {:#x}",
                d.Width, d.Height, d.Format.0, d.Flags.0, crate::resource::data_of(r)
            )
        })
    };
    match view {
        None => "no view".into(),
        Some(v @ View::Shader(r, _)) => texture(*r as This)
            .unwrap_or_else(|| format!("buffer {:?}", crate::descriptor::host::buffer_range(v))),
        Some(v @ View::Unordered(r, ..)) => format!(
            "uav {}",
            texture(*r as This).unwrap_or_else(|| format!(
                "buffer {:?}",
                crate::descriptor::host::buffer_range(v)
            ))
        ),
        Some(v @ View::Placed(u, r, d)) => {
            let is = (!*u && r[0] != 1)
                .then(|| crate::descriptor::host::placed_texture(v))
                .flatten();
            let is = match is {
                Some(View::Shader(t, _)) => format!(
                    " is {t:#x} {}",
                    texture(t as This).unwrap_or_else(|| "no host texture".into())
                ),
                _ => String::new(),
            };
            format!(
                "placed uav={u} dim {} {}x{} f{} layout {:#x}{is} at {:#x}",
                r[0],
                u64::from(r[4]) | u64::from(r[5]) << 32,
                r[6],
                r[8],
                r[11],
                u64::from(d[10]) | u64::from(d[11]) << 32
            )
        }
        // The first four values, as a colour rectangle's constants are.
        Some(View::Constant(address, size))
            if *size >= 16 && crate::texture::committed(*address as *const u8, 16) =>
        {
            // SAFETY: 16 committed bytes of title memory.
            let v = unsafe { (*address as *const [f32; 4]).read_unaligned() };
            let words = crate::shadow::words_text(*address, *size as usize);
            format!("Constant({address:#x}, {size}) {v:?} words {words}")
        }
        Some(v) => format!("{v:?}").chars().take(60).collect(),
    }
}

/// The view heap the last noted command used.
#[cfg(feature = "diagnostics")]
static HEAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// What the slots `indices` of that heap view.
#[cfg(feature = "diagnostics")]
pub(crate) fn report_slots(indices: &[u32]) -> String {
    let heap = HEAP.load(Ordering::Relaxed);
    if heap == 0 {
        return String::new();
    }
    let Some(h) = crate::descriptor::heap(heap as This).host() else {
        return String::new();
    };
    indices.iter().map(|&i| format!(" SLOT {i:#x} {}", h.report_slot(i))).collect()
}

/// The console's own primitive topologies.
const TRIANGLE_FAN: u32 = 6;
const RECTANGLES: u32 = 17;
const LINE_LOOP: u32 = 18;
const QUADS: u32 = 19;
const QUAD_STRIP: u32 = 20;

/// Fixed-function state, straight onto the host list.
fn viewports(list: &HostList, viewports: &[D3D12_VIEWPORT]) {
    // SAFETY: an open list.
    unsafe { list.list.RSSetViewports(viewports) };
}

fn scissors(list: &HostList, rects: &[RECT]) {
    // SAFETY: an open list.
    unsafe { list.list.RSSetScissorRects(rects) };
}

fn blend_factor(list: &HostList, factor: Option<&[f32; 4]>) {
    // SAFETY: an open list.
    unsafe { list.list.OMSetBlendFactor(factor) };
}

fn stencil_ref(list: &HostList, reference: u32) {
    // SAFETY: an open list.
    unsafe { list.list.OMSetStencilRef(reference) };
}

#[cfg(test)]
mod group_limit_tests {
    #[test]
    fn dispatches_past_the_host_limit_do_not_fit() {
        assert!(super::dispatch_fits(65_535, 65_535, 65_535));
        assert!(super::dispatch_fits(0, 0, 0));
        assert!(!super::dispatch_fits(65_536, 1, 1));
        assert!(!super::dispatch_fits(1, u32::MAX, 1));
        assert!(!super::dispatch_fits(1, 1, 65_536));
    }
}
