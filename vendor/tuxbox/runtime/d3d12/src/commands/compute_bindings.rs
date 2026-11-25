//! Save the guest bindings displaced by an internal compute pass. Graphics
//! root arguments other than tables and IA/OM/RS state survive the pass.
use crate::device::compute::{ComputeTable, Pipeline as GuestPipeline, RootSource, SceneRoot, SceneSource};
use std::collections::BTreeMap;
use std::sync::Arc;
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Result};

#[derive(Default)]
pub(super) struct Bindings {
    pub(super) pipeline: Option<Pipeline>,
    pub(super) draw_trace: Option<Arc<crate::diagnostics::RasterDraw>>,
    /// The bound pipeline's heap access reflection (TUXBOX_SHADOW_UAV_REMARK).
    pub(super) heap_access: Option<Arc<crate::device::shader_cache::HeapAccess>>,
    heaps: Vec<Option<ID3D12DescriptorHeap>>,
    compute: Option<ID3D12RootSignature>,
    layout: Option<RootSource>,
    graphics: Option<ID3D12RootSignature>,
    graphics_layout: Option<RootSource>,
    compute_args: BTreeMap<u32, Argument>,
    graphics_buffers: BTreeMap<u32, BufferView>,
    graphics_constants: BTreeMap<(u32,u32),u32>,
    graphics_tables: BTreeMap<u32, DescriptorTable>,
    /// Root views bound as one-view tables (root_tables), by (compute, slot).
    /// The BufferView beside each keeps the address (scene detection).
    view_tables: BTreeMap<(bool, u32), D3D12_GPU_DESCRIPTOR_HANDLE>,
    /// Every root view bound on a tabled parameter with its import, so a heap
    /// change can write it into the new heap (rebind_views).
    view_sources: BTreeMap<(bool, u32), (BufferView, Option<ID3D12Resource>)>,
    /// SetSampleMaskX's quad mask (None: none set), for translated pixel
    /// shaders that declare it (quad_mask).
    pub(super) quad_mask: Option<u64>,
    /// Title addresses of compute root SRVs, by slot (bvh_raw).
    root_guests: BTreeMap<u32, u64>,
    /// Title addresses of every root CBV/SRV/UAV by (compute, slot), for the
    /// table words a stage reads as data (table_words).
    root_vas: BTreeMap<(bool, u32), u64>,
}

// A private root descriptor captures its value when recording. A guest
// descriptor table may change until submission, especially under root v1.0.
#[derive(Clone, Copy, PartialEq)]
enum SceneLocation {
    Table(D3D12_CPU_DESCRIPTOR_HANDLE),
    Address(u64),
}

impl SceneLocation {
    unsafe fn resolve(self, owner: &IUnknown) -> Result<(ID3D12Resource, u64)> {
        unsafe {
            match self {
                Self::Table(cpu) => crate::device::blas::inline_scene(owner, cpu),
                Self::Address(va) => crate::device::blas::inline_scene_address(owner, va),
            }
        }
    }
}

#[derive(PartialEq)]
pub(super) struct SceneCheck {
    location: SceneLocation,
    pub(super) scene: (ID3D12Resource, u64),
    pub(super) geometry: Option<[(ID3D12Resource, u64); 2]>,
}

impl SceneCheck {
    pub(super) fn recheck_on_submit(&self) -> bool {
        matches!(self.location, SceneLocation::Table(_)) || self.geometry.is_some()
    }

    pub(super) fn validate(&self, list: &super::List) -> Result<()> {
        let owner = list.parent.as_ref().ok_or(E_INVALIDARG)?;
        if unsafe { self.location.resolve(owner)? } != self.scene {
            return Err(E_INVALIDARG.into());
        }
        if let Some(geometry) = &self.geometry
            && unsafe { crate::device::blas::inline_geometry(owner, self.scene.1)? } != *geometry
        {
            return Err(E_INVALIDARG.into());
        }
        Ok(())
    }
}

pub(super) enum Pipeline {
    Regular(
        ID3D12PipelineState,
        Option<SceneRoot>,
        Option<Arc<ComputeTable>>,
        Option<Arc<ComputeTable>>,
        Option<Arc<crate::device::null_kinds::NullKinds>>,
    ),
    Ray(ID3D12StateObject),
}

#[derive(Clone, Copy)]
pub(super) enum BufferView {
    Cbv(u64),
    Srv(u64),
    Uav(u64),
}

/// TUXBOX_VIEW_TABLE_REBIND=0: a tabled root view with no slot is bound as a
/// root descriptor and a heap change drops tabled views, as before.
fn view_rebind() -> bool {
    crate::env_toggle!("TUXBOX_VIEW_TABLE_REBIND") != Some("0")
}

impl BufferView {
    pub(super) unsafe fn apply<const COMPUTE: bool>(self, list: &ID3D12GraphicsCommandList, slot: u32) {
        match self {
            Self::Cbv(0) => crate::diagnostics::null_va("root CBV", format_args!("slot {slot}, compute {COMPUTE}")),
            Self::Srv(0) => crate::diagnostics::null_va("root SRV", format_args!("slot {slot}, compute {COMPUTE}")),
            Self::Uav(0) => crate::diagnostics::null_va("root UAV", format_args!("slot {slot}, compute {COMPUTE}")),
            _ => {}
        }
        unsafe {
            match (COMPUTE, self) {
                (true, Self::Cbv(va)) => list.SetComputeRootConstantBufferView(slot, va),
                (true, Self::Srv(va)) => list.SetComputeRootShaderResourceView(slot, va),
                (true, Self::Uav(va)) => list.SetComputeRootUnorderedAccessView(slot, va),
                (false, Self::Cbv(va)) => list.SetGraphicsRootConstantBufferView(slot, va),
                (false, Self::Srv(va)) => list.SetGraphicsRootShaderResourceView(slot, va),
                (false, Self::Uav(va)) => list.SetGraphicsRootUnorderedAccessView(slot, va),
            }
        }
    }
}

unsafe fn root_table<const COMPUTE: bool>(list: &ID3D12GraphicsCommandList, slot: u32, handle: D3D12_GPU_DESCRIPTOR_HANDLE) {
    unsafe {
        if COMPUTE { list.SetComputeRootDescriptorTable(slot, handle) } else { list.SetGraphicsRootDescriptorTable(slot, handle) }
    }
}

#[derive(Clone, Copy)]
pub(super) struct DescriptorTable {
    native: D3D12_GPU_DESCRIPTOR_HANDLE,
    logical: D3D12_GPU_DESCRIPTOR_HANDLE,
}

pub(super) enum Argument {
    Table(DescriptorTable),
    Buffer(BufferView),
    Constants(BTreeMap<u32, u32>),
}

impl From<&GuestPipeline> for Pipeline {
    fn from(p: &GuestPipeline) -> Self {
        let (native, data, _) = crate::device::compute::current(p);
        Self::Regular(native.clone(), data.scene_root.clone(), data.reflected.clone(), data.raster_table.clone(), data.null_kinds.clone())
    }
}

impl Bindings {
    pub(super) fn new(initial: Option<&GuestPipeline>) -> Self {
        Self {
            pipeline: initial.map(Pipeline::from),
            draw_trace: initial.and_then(|p| crate::device::compute::current(p).1.draw_trace.clone()),
            ..Default::default()
        }
    }

    pub(super) fn compute_root(
        &mut self,
        root: Option<&ID3D12RootSignature>,
        layout: Option<&RootSource>,
    ) {
        if self.compute.as_ref() != root {
            self.compute_args.clear();
            self.view_tables.retain(|&(compute, _), _| !compute);
            self.view_sources.retain(|&(compute, _), _| !compute);
            self.layout = layout.cloned();
            self.compute = root.cloned();
        }
    }

    pub(super) fn graphics_root(&mut self, root: Option<&ID3D12RootSignature>, layout: Option<&RootSource>) {
        if self.graphics.as_ref() != root {
            self.graphics_layout = layout.cloned();
            self.graphics_tables.clear();
            self.graphics_constants.clear();
            self.graphics_buffers.clear();
            self.view_tables.retain(|&(compute, _), _| compute);
            self.view_sources.retain(|&(compute, _), _| compute);
            self.graphics = root.cloned();
        }
    }

    pub(super) fn heaps(&mut self, heaps: &[Option<ID3D12DescriptorHeap>]) {
        if self.heaps.len() != heaps.len() || heaps.iter().any(|heap| !self.heaps.contains(heap)) {
            self.compute_args
                .retain(|_, arg| !matches!(arg, Argument::Table(_)));
            self.graphics_tables.clear();
            self.view_tables.clear();
            self.heaps.clear();
            self.heaps.extend_from_slice(heaps);
        }
    }

    pub(super) fn table<const COMPUTE: bool>(
        &mut self,
        slot: u32,
        base: D3D12_GPU_DESCRIPTOR_HANDLE,
        logical: D3D12_GPU_DESCRIPTOR_HANDLE,
    ) {
        let table = DescriptorTable { native: base, logical };
        if COMPUTE {
            self.arg(slot, Argument::Table(table));
        } else {
            self.graphics_tables.insert(slot, table);
        }
    }

    pub(super) fn buffer<const COMPUTE: bool>(&mut self, slot: u32, view: BufferView) {
        if COMPUTE {
            self.compute_args.insert(slot, Argument::Buffer(view));
        } else {
            self.graphics_buffers.insert(slot, view);
        }
    }

    /// Binds a title root view: on a parameter the root signature made a
    /// one-view table (device::root_tables, VIctory-fxceless GPL-3.0
    /// bounded_root_srvs), a raw view of `buffer` from the view's address to
    /// its end in a private slot of the bound view heap; else (or with no
    /// slot, logged once) the root descriptor as before.
    pub(super) fn bind_buffer<const COMPUTE: bool>(
        &mut self, list: &ID3D12GraphicsCommandList, slot: u32, view: BufferView, buffer: Option<&ID3D12Resource>,
    ) {
        self.view_tables.remove(&(COMPUTE, slot));
        self.view_sources.remove(&(COMPUTE, slot));
        let tabled = matches!(view, BufferView::Srv(_) | BufferView::Uav(_)) && self.view_tabled::<COMPUTE>(slot);
        if tabled && view_rebind() {
            self.view_sources.insert((COMPUTE, slot), (view, buffer.cloned()));
        }
        let table = match view {
            BufferView::Srv(va) | BufferView::Uav(va) if self.view_tabled::<COMPUTE>(slot) => {
                let uav = matches!(view, BufferView::Uav(_));
                let heap = self.heaps.iter().flatten()
                    .find(|h| unsafe { h.GetDesc() }.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
                let handle = heap.and_then(|h| crate::device::root_tables::view_slot(h, buffer, va, uav));
                if handle.is_none() {
                    static NONE: std::sync::Once = std::sync::Once::new();
                    crate::diagnostics::log_once(&NONE, || format!(
                        "root view table: no private slot in the bound view heap (slot {slot}, compute {COMPUTE}); bound as a root descriptor"));
                }
                handle
            }
            _ => None,
        };
        match table {
            Some(handle) => {
                unsafe { root_table::<COMPUTE>(list, slot, handle) };
                self.view_tables.insert((COMPUTE, slot), handle);
            }
            // A root descriptor call on a parameter the signature made a table
            // writes a GPU address where a descriptor handle belongs, and the
            // next draw reads through it (test157: the first list hung on its
            // first draw). It is left unbound until a heap gives it a slot.
            None if tabled && view_rebind() => {}
            None => unsafe { view.apply::<COMPUTE>(list, slot) },
        }
        self.buffer::<COMPUTE>(slot, view);
    }

    pub(super) fn note_root_va<const COMPUTE: bool>(&mut self, slot: u32, guest: u64) {
        if guest == 0 { self.root_vas.remove(&(COMPUTE, slot)); } else { self.root_vas.insert((COMPUTE, slot), guest); }
    }

    pub(super) fn note_guest<const COMPUTE: bool>(&mut self, slot: u32, guest: Option<u64>) {
        if COMPUTE {
            match guest {
                Some(va) => { self.root_guests.insert(slot, va); }
                None => { self.root_guests.remove(&slot); }
            }
        }
    }

    /// After SetDescriptorHeaps: root views outlive a heap change on the host,
    /// so each tabled one is written into the new heap's private slots and
    /// bound again (the title never rebinds them).
    pub(super) fn rebind_views(&mut self, list: &ID3D12GraphicsCommandList) {
        let sources: Vec<_> = self.view_sources.iter().map(|(&k, (v, b))| (k, *v, b.clone())).collect();
        for ((compute, slot), view, buffer) in sources {
            if compute {
                self.bind_buffer::<true>(list, slot, view, buffer.as_ref());
            } else {
                self.bind_buffer::<false>(list, slot, view, buffer.as_ref());
            }
        }
    }

    fn view_tabled<const COMPUTE: bool>(&self, slot: u32) -> bool {
        crate::device::root_tables::enabled()
            && (if COMPUTE { &self.compute } else { &self.graphics }).as_ref()
                .and_then(crate::device::root_tables::tabled)
                .is_some_and(|t| t.contains(&slot))
    }

    /// Replays a saved root view: its table when it was bound as one.
    unsafe fn replay_buffer<const COMPUTE: bool>(&self, list: &ID3D12GraphicsCommandList, slot: u32, view: BufferView) {
        match self.view_tables.get(&(COMPUTE, slot)) {
            Some(&handle) => unsafe { root_table::<COMPUTE>(list, slot, handle) },
            None if view_rebind() && self.view_sources.contains_key(&(COMPUTE, slot)) => {}
            None => unsafe { view.apply::<COMPUTE>(list, slot) },
        }
    }

    fn arg(&mut self, slot: u32, arg: Argument) {
        self.compute_args.insert(slot, arg);
    }

    pub(super) fn constants(&mut self, slot: u32, offset: u32, words: &[u32]) {
        let arg = self
            .compute_args
            .entry(slot)
            .or_insert_with(|| Argument::Constants(Default::default()));
        if !matches!(arg, Argument::Constants(_)) {
            *arg = Argument::Constants(Default::default());
        }
        if let Argument::Constants(values) = arg {
            values.extend((offset..).zip(words.iter().copied()));
        }
    }

    unsafe fn apply_compute_args(
        &self, host: &ID3D12GraphicsCommandList, overrides: &[Option<D3D12_GPU_DESCRIPTOR_HANDLE>],
    ) {
        unsafe {
            for (&slot, arg) in &self.compute_args {
                match arg {
                    Argument::Table(table) => {
                        let base = overrides.get(slot as usize).copied().flatten().unwrap_or(table.native);
                        host.SetComputeRootDescriptorTable(slot, base);
                    }
                    Argument::Buffer(view) => self.replay_buffer::<true>(host, slot, *view),
                    Argument::Constants(words) => {
                        for (&index, &value) in words {
                            host.SetComputeRoot32BitConstant(slot, value, index);
                        }
                    }
                }
            }
        }
    }

    /// The bound pipeline's null descriptor kinds per table slot.
    fn null_kinds(&self) -> Option<&crate::device::null_kinds::NullKinds> {
        match &self.pipeline {
            Some(Pipeline::Regular(_, _, _, _, kinds)) => kinds.as_deref(),
            _ => None,
        }
    }

    pub(super) fn inline_ranges<const COMPUTE: bool>(&self, slot: usize) -> &[std::ops::Range<u32>] {
        let root = if COMPUTE { &self.layout } else { &self.graphics_layout };
        root.as_ref().map_or(&[], |root| root.inline_ranges(slot))
    }

    pub(super) fn paged_layout<const COMPUTE: bool>(&self) -> Option<&[Option<u32>]> {
        let root = if COMPUTE { &self.layout } else { &self.graphics_layout };
        root.as_ref()?.paged_tables.as_deref()
    }

    /// The host addresses of the root UAVs a dispatch of the bound compute
    /// root and pipeline can write, or None when it may write through
    /// anything else: no root layout, a directly indexed root, a reflected
    /// or scene pipeline, a descriptor table with UAV ranges bound, or heap
    /// writes in its reflection (see `stale_writes` in commands.rs).
    pub(super) fn root_uav_writes(&self) -> Option<Vec<u64>> {
        let layout = self.layout.as_ref()?;
        // The root bounds what any pipeline on it writes (a pending one too):
        // root UAVs and UAV table ranges, unless it is directly indexed or a
        // table is unbounded (uav_tables lists only finite ranges).
        if layout.direct_resources || layout.paged_tables.is_none() || self.heap_access.is_some() || self.reflected_table::<true>().is_some() || self.scene_root().is_some() {
            return None;
        }
        if layout.uav_tables.iter().any(|(slot, _)| matches!(self.compute_args.get(slot), Some(Argument::Table(_)))) {
            return None;
        }
        Some(self.compute_args.values().filter_map(|a| match a {
            Argument::Buffer(BufferView::Uav(va)) => Some(*va),
            _ => None,
        }).collect())
    }

    /// (directly indexed, every table bounded) of the bound root, if any.
    pub(super) fn paged_shape<const COMPUTE: bool>(&self) -> Option<(bool, bool)> {
        let root = if COMPUTE { &self.layout } else { &self.graphics_layout };
        root.as_ref().map(|r| (r.direct_resources, r.paged_tables.is_some()))
    }

    pub(super) fn supports_paged_draw<const COMPUTE: bool>(&self) -> bool {
        let root = if COMPUTE { &self.layout } else { &self.graphics_layout };
        root.as_ref().is_some_and(|r| !r.direct_resources && r.paged_tables.is_some())
    }

    pub(super) fn graphics_constants(&mut self, slot:u32, first:u32, words:&[u32]) {
        self.graphics_constants.extend((first..).zip(words).map(|(offset, &value)| ((slot, offset),value)));
    }
    pub(super) fn graphics_constant(&self, slot: u32, offset: u32) -> Option<u32> {
        self.graphics_constants.get(&(slot, offset)).copied()
    }
    unsafe fn apply_graphics_arguments(&self, list:&ID3D12GraphicsCommandList) {
        for (&slot, &view) in &self.graphics_buffers {
            unsafe { self.replay_buffer::<false>(list, slot, view); }
        }
        for (&(slot, offset), &value) in &self.graphics_constants {
            unsafe { list.SetGraphicsRoot32BitConstant(slot, value, offset); }
        }
    }

    pub(super) fn reflected_table<const COMPUTE: bool>(&self) -> Option<&ComputeTable> {
        match &self.pipeline {
            Some(Pipeline::Regular(_, _, compute, raster, _)) => if COMPUTE { compute.as_deref() } else { raster.as_deref() },
            _ => None,
        }
    }

    pub(super) fn with_reflected_table<const COMPUTE: bool>(
        &self, table: &ComputeTable, guest: &super::List,
        arena: Option<&crate::device::descriptor_paging::Arena>,
        heap: Option<&super::descriptors::ResourceHeap>, reads: &mut Vec<std::ops::Range<usize>>,
        resources: &crate::device::MediaResources, dispatch: impl FnOnce(),
    ) -> Result<()> {
        let layout = if COMPUTE { &self.layout } else { &self.graphics_layout };
        if !layout.as_ref().is_some_and(|l| l.matches(&table.original)) {
            return Err(E_INVALIDARG.into());
        }
        let mut storage = [None; 64]; // no root signature can consume more than 64 DWORDs
        let bases = storage.get_mut(..table.lengths.len()).ok_or(E_INVALIDARG)?;
        for (index, &length) in table.lengths.iter().enumerate().filter(|(_, len)| **len != 0) {
            let slot = index as u32;
            let bound = if COMPUTE {
                let Some(Argument::Table(table)) = self.compute_args.get(&slot) else { return Err(E_INVALIDARG.into()); };
                table
            } else { self.graphics_tables.get(&slot).ok_or(E_INVALIDARG)? };
            bases[index] = Some(match arena.filter(|a| a.contains(bound.logical.ptr)) {
                Some(arena) => arena.gpu_table(bound.logical.ptr, length, table.layout.inline_ranges(index), self.null_kinds().map(|k| (k, index)))?,
                None => bound.native,
            });
        }
        let heap = heap.ok_or(E_INVALIDARG)?;
        // SRV-as-UAV piece 4 (TUXBOX_SRV_AS_UAV=1): a root made for a program's
        // stored-to SRV slots binds a private copy of each such table with
        // UAVs of the slots' resources there (device::srv_views::fill_table).
        if arena.is_none()
            && let Some(slots) = crate::device::srv_views::table_slots(&table.native)
            && let Some(native_heap) = self.heaps.iter().flatten()
                .find(|h| unsafe { h.GetDesc() }.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
        {
            for (index, &length) in table.lengths.iter().enumerate().filter(|(_, len)| **len != 0) {
                let Some(base) = bases[index] else { continue };
                let Some(src) = heap.table(base.ptr, &(0..length)) else { continue };
                if let Some(gpu) = crate::device::srv_views::fill_table(native_heap, src.start, length, index as u32, &slots) {
                    bases[index] = Some(D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu });
                }
            }
        }
        let index_map = if let Some(binding) = table.index_map {
            // Global indices originate in shader data. Any resident SRV can
            // sample placed textures and needs the usual pre-submit upload.
            super::descriptors::include_range(reads, heap.cpu.clone());
            Some((binding.slot, heap.index_map(guest, arena, resources)?))
        } else {
            if table.layout.direct_resources {
                super::descriptors::include_range(reads, heap.cpu.clone());
            }
            for (slot, range) in &table.accessed_views {
                let base = bases[*slot].ok_or(E_INVALIDARG)?;
                super::descriptors::include_range(reads, heap.table(base.ptr, range).ok_or(E_INVALIDARG)?);
            }
            None
        };
        let desc_table = match table.desc_table {
            Some(slot) => {
                let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
                Some((slot, unsafe { crate::device::desc_table_address(owner)? }))
            }
            None => None,
        };
        // The typed view format table of the bound shader-visible heap: the
        // paged arena's visible heap, else the heap itself (view_formats).
        let view_formats = match table.view_formats {
            Some(slot) => {
                let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
                let stride = heap.table_stride();
                let (start, count) = match arena {
                    Some(arena) => {
                        let visible = arena.visible();
                        (unsafe { visible.GetCPUDescriptorHandleForHeapStart() }.ptr, unsafe { visible.GetDesc() }.NumDescriptors)
                    }
                    None => (heap.cpu.start, heap.table_slots()),
                };
                let (buffer, address) = unsafe { crate::device::view_formats_address(owner, start, count, stride)? };
                let held_buffer = ID3D12Pageable::from(buffer);
                let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                if !held.contains(&held_buffer) {
                    held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                    held.push(held_buffer);
                }
                Some((slot, address))
            }
            None => None,
        };
        // The buffer view address table of the same heap (slot_views).
        let slot_views = match table.slot_views {
            Some(slot) => {
                let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
                let stride = heap.table_stride();
                let (start, count) = match arena {
                    Some(arena) => {
                        let visible = arena.visible();
                        (unsafe { visible.GetCPUDescriptorHandleForHeapStart() }.ptr, unsafe { visible.GetDesc() }.NumDescriptors)
                    }
                    None => (heap.cpu.start, heap.table_slots()),
                };
                let (buffer, address) = unsafe { crate::device::slot_views_address(owner, start, count, stride)? };
                let held_buffer = ID3D12Pageable::from(buffer);
                let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                if !held.contains(&held_buffer) {
                    held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                    held.push(held_buffer);
                }
                Some((slot, address))
            }
            None => None,
        };
        // The console descriptor words of the same heap (slot_words).
        let slot_words = match table.slot_words {
            Some(slot) => {
                let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
                let stride = heap.table_stride();
                let (start, count) = match arena {
                    Some(arena) => {
                        let visible = arena.visible();
                        (unsafe { visible.GetCPUDescriptorHandleForHeapStart() }.ptr, unsafe { visible.GetDesc() }.NumDescriptors)
                    }
                    None => (heap.cpu.start, heap.table_slots()),
                };
                let (buffer, address) = unsafe { crate::device::slot_words_address(owner, start, count, stride)? };
                let held_buffer = ID3D12Pageable::from(buffer);
                let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                if !held.contains(&held_buffer) {
                    held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                    held.push(held_buffer);
                }
                Some((slot, address))
            }
            None => None,
        };
        // Page directory (mem_dir, MESIA-Unified memory.rs ready() and
        // draw.rs:948-961): make the pages this dispatch names ready (root
        // views, address-like root constants, last faults), then bind.
        let mem_dir = match table.mem_dir {
            Some(slot) if COMPUTE => {
                let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
                let mut device = None::<ID3D12Device>;
                unsafe { guest.native.GetDevice(&mut device)? };
                let device = device.ok_or(E_INVALIDARG)?;
                let constants: Vec<u32> = self.compute_args.values()
                    .filter_map(|a| match a { Argument::Constants(c) => Some(c.values().copied()), _ => None })
                    .flatten().collect();
                let named = crate::device::mem_dir::named(self.root_guests.values().copied(), &constants);
                crate::device::mem_dir::take_faults(owner, &device);
                crate::device::mem_dir::ready(owner, &device, &named);
                let visible = if arena.is_some() { None } else {
                    self.heaps.iter().flatten().find(|h| unsafe { h.GetDesc() }.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                };
                crate::device::mem_dir::binding(&device, visible).map(|(dir, faults, base)| (slot, dir, faults, base))
            }
            _ => None,
        };
        // Write marks (view_marks): the same heap's view-mark table and the
        // bitmap shaders mark their writes in.
        let view_marks = match table.view_marks {
            Some(slot) => {
                let stride = heap.table_stride();
                let (start, count) = match arena {
                    Some(arena) => {
                        let visible = arena.visible();
                        (unsafe { visible.GetCPUDescriptorHandleForHeapStart() }.ptr, unsafe { visible.GetDesc() }.NumDescriptors)
                    }
                    None => (heap.cpu.start, heap.table_slots()),
                };
                let mut device = None::<ID3D12Device>;
                unsafe { guest.native.GetDevice(&mut device)? };
                let device = device.ok_or(E_INVALIDARG)?;
                let (buffer, address) = crate::device::view_marks::table_address(&device, start, count, stride)?;
                let bitmap = crate::device::view_marks::bitmap_address(&device).ok_or(E_INVALIDARG)?;
                let held_buffer = ID3D12Pageable::from(buffer);
                let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                if !held.contains(&held_buffer) {
                    held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                    held.push(held_buffer);
                }
                Some((slot, address, bitmap))
            }
            None => None,
        };
        // Table words (table_words): each declared guest root parameter's title memory.
        let mut word_views = Vec::new();
        if let Some((first, params)) = &table.table_words {
            let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
            for (k, param) in params.iter().enumerate() {
                // A descriptor-table parameter points at heap cells, which
                // hold host records, not console words: it reads the encoded
                // words of its slots (slot_words, 32 bytes a slot like the
                // console's descriptors) from the table's first slot.
                let tabled = bases.get(*param as usize).copied().flatten().and_then(|base| {
                    let stride = heap.table_stride();
                    let (start, count, gpu) = match arena {
                        Some(arena) => {
                            let visible = arena.visible();
                            (unsafe { visible.GetCPUDescriptorHandleForHeapStart() }.ptr, unsafe { visible.GetDesc() }.NumDescriptors,
                             unsafe { visible.GetGPUDescriptorHandleForHeapStart() }.ptr)
                        }
                        None => (heap.cpu.start, heap.table_slots(), heap.gpu_start()),
                    };
                    let slot = base.ptr.checked_sub(gpu)? / stride as u64;
                    let (buffer, address) = unsafe { crate::device::slot_words_address(owner, start, count, stride) }.ok()?;
                    Some((buffer, address + slot * size_of::<crate::device::slot_words::Words>() as u64))
                });
                let resolved = tabled.or_else(|| self.root_vas.get(&(COMPUTE, *param))
                    .and_then(|&va| unsafe { crate::device::table_words_address(owner, va) }));
                let address = match resolved {
                    Some((buffer, address)) => {
                        let held_buffer = ID3D12Pageable::from(buffer);
                        let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                        if !held.contains(&held_buffer) {
                            held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                            held.push(held_buffer);
                        }
                        address
                    }
                    None => super::compute::null_sink(guest).unwrap_or(0),
                };
                if address != 0 {
                    word_views.push((first + k as u32, address));
                }
            }
            static ONCE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&ONCE, || format!(
                "table words bound for guest root parameters {params:?} (t<p> space32765; TUXBOX_TABLE_WORDS=0 leaves them out)"));
        }
        // BVH raw root views (bvh_raw): each guest root SRV's title memory.
        let mut bvh_views = Vec::new();
        if COMPUTE && let Some((first, slots)) = &table.bvh_raw {
            let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
            for (k, slot) in slots.iter().enumerate() {
                let resolved = self.root_guests.get(slot)
                    .and_then(|&va| unsafe { crate::device::bvh_root_address(owner, va) });
                let address = match resolved {
                    Some((buffer, address)) => {
                        let held_buffer = ID3D12Pageable::from(buffer);
                        let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                        if !held.contains(&held_buffer) {
                            held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                            held.push(held_buffer);
                        }
                        address
                    }
                    None => super::compute::null_sink(guest).unwrap_or(0),
                };
                if address != 0 {
                    bvh_views.push((first + k as u32, address));
                }
            }
        }
        // Root raw view strides (root_strides): by register key.
        let mut stride_table = None;
        if COMPUTE && let Some((slot, guests)) = &table.root_strides {
            let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
            let keys: Vec<(u32, u32)> = guests.iter()
                .map(|&(param, key)| (key, self.root_guests.get(&param)
                    .map_or(0, |&va| crate::device::slot_views::stride_at(va))))
                .collect();
            if let Some((buffer, address)) = crate::device::root_strides::table(owner, &keys) {
                let held_buffer = ID3D12Pageable::from(buffer);
                let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                if !held.contains(&held_buffer) {
                    held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                    held.push(held_buffer);
                }
                stride_table = Some((*slot, address));
            }
        }
        // Hoisted table parameters (mesh FSSM 0x100): the title address of
        // the buffer view in the bound table's first slot, as a root SRV.
        let mut hoisted_views = Vec::new();
        if !COMPUTE {
            for &slot in &table.hoisted {
                let base = bases.get(slot as usize).copied().flatten().or_else(|| self.graphics_tables.get(&slot).map(|b| b.native));
                let cpu = base.and_then(|b| heap.table(b.ptr, &(0..1)));
                let entry = cpu.as_ref().and_then(|cpu| crate::device::slot_views::get(cpu.start));
                {
                    // Confirm (Coder4): the table and its slot 0 view for the first draws.
                    static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                    if SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
                        crate::diagnostics::log_line(format_args!(
                            "mesh hoisted SRV: param {slot} table gpu {:#x} cpu {:#x?}, slot 0 view [addr lo, hi, len, stride] {entry:x?}",
                            base.map_or(0, |b| b.ptr), cpu.as_ref().map(|c| c.start)));
                    }
                }
                let title = entry
                    .map(|e| u64::from(e[0]) | u64::from(e[1]) << 32)
                    .filter(|&va| va != 0);
                let owner = guest.parent.as_ref().ok_or(E_INVALIDARG)?;
                let resolved = title.and_then(|va| unsafe { crate::device::table_words_address(owner, va) });
                let address = match resolved {
                    Some((buffer, address)) => {
                        let held_buffer = ID3D12Pageable::from(buffer);
                        let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
                        if !held.contains(&held_buffer) {
                            held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                            held.push(held_buffer);
                        }
                        address
                    }
                    None => {
                        static UNRESOLVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                        let n = UNRESOLVED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        if n.is_power_of_two() {
                            crate::diagnostics::log_line(format_args!(
                                "mesh hoisted SRV: root parameter {slot} has no buffer view in its table's first slot (title address {title:?}); bound null ({n} so far; TUXBOX_MESH_HOISTED_SRV=0 off)"));
                        }
                        0
                    }
                };
                hoisted_views.push((slot, address));
            }
        }
        let list = &guest.native;
        unsafe {
            if COMPUTE {
                list.SetComputeRootSignature(&table.native);
                // Changing root signatures invalidates the native arguments. Replay
                // constants and descriptors, then substitute each reflected table.
                self.apply_compute_args(list, bases);
                if let Some((slot, address)) = index_map {
                    list.SetComputeRootShaderResourceView(slot, address);
                    static ONCE: std::sync::Once = std::sync::Once::new();
                    crate::diagnostics::log_once(&ONCE, || format!("bound resource descriptor index map at compute root slot {slot}"));
                }
                if let Some((slot, address)) = desc_table {
                    list.SetComputeRootShaderResourceView(slot, address);
                }
                if let Some((slot, address)) = view_formats {
                    list.SetComputeRootShaderResourceView(slot, address);
                }
                if let Some((slot, address)) = slot_views {
                    list.SetComputeRootShaderResourceView(slot, address);
                }
                if let Some((slot, address)) = slot_words {
                    list.SetComputeRootShaderResourceView(slot, address);
                }
                for &(slot, address) in &word_views {
                    list.SetComputeRootShaderResourceView(slot, address);
                }
                if let Some((slot, table, bitmap)) = view_marks {
                    list.SetComputeRootShaderResourceView(slot, table);
                    list.SetComputeRootUnorderedAccessView(slot + 1, bitmap);
                }
                for &(slot, address) in &bvh_views {
                    list.SetComputeRootShaderResourceView(slot, address);
                }
                if let Some((slot, dir, faults, base)) = mem_dir {
                    list.SetComputeRootShaderResourceView(slot, dir);
                    list.SetComputeRootUnorderedAccessView(slot + 1, faults);
                    list.SetComputeRoot32BitConstant(slot + 2, base, 0);
                }
                if let Some((slot, address)) = stride_table {
                    list.SetComputeRootShaderResourceView(slot, address);
                }
                dispatch();
                list.SetComputeRootSignature(self.compute.as_ref());
                self.apply_compute_args(list, &[]);
            } else {
                list.SetGraphicsRootSignature(&table.native);
                self.apply_graphics_arguments(list);
                for (&slot, bound) in &self.graphics_tables {
                    if table.hoisted.contains(&slot) {
                        continue;
                    }
                    let base = bases.get(slot as usize).copied().flatten().unwrap_or(bound.native);
                    list.SetGraphicsRootDescriptorTable(slot, base);
                }
                for &(slot, address) in &hoisted_views {
                    list.SetGraphicsRootShaderResourceView(slot, address);
                }
                if let Some((slot, address)) = index_map {
                    list.SetGraphicsRootShaderResourceView(slot, address);
                    static IDENTITY: std::sync::Once = std::sync::Once::new();
                    static PAGED: std::sync::Once = std::sync::Once::new();
                    let (site, kind) = if arena.is_some() { (&PAGED, "paged") } else { (&IDENTITY, "identity") };
                    crate::diagnostics::log_once(site, || {
                        format!("bound {kind} resource descriptor index map at graphics root slot {slot}")
                    });
                }
                if let Some((slot, address)) = desc_table {
                    list.SetGraphicsRootShaderResourceView(slot, address);
                }
                if let Some((slot, address)) = view_formats {
                    list.SetGraphicsRootShaderResourceView(slot, address);
                }
                if let Some((slot, address)) = slot_views {
                    list.SetGraphicsRootShaderResourceView(slot, address);
                }
                if let Some((slot, address)) = slot_words {
                    list.SetGraphicsRootShaderResourceView(slot, address);
                }
                for &(slot, address) in &word_views {
                    list.SetGraphicsRootShaderResourceView(slot, address);
                }
                if let Some((slot, table, bitmap)) = view_marks {
                    list.SetGraphicsRootShaderResourceView(slot, table);
                    list.SetGraphicsRootUnorderedAccessView(slot + 1, bitmap);
                }
                if let Some(slot) = table.quad_mask {
                    let words = crate::device::quad_mask::words(self.quad_mask);
                    list.SetGraphicsRoot32BitConstants(slot, 2, words.as_ptr().cast(), 0);
                }
                dispatch();
                list.SetGraphicsRootSignature(self.graphics.as_ref());
                self.apply_graphics_arguments(list);
                for (&slot, &handle) in &self.graphics_tables { list.SetGraphicsRootDescriptorTable(slot, handle.native); }
            }
        }
        Ok(())
    }

    pub(super) fn scene_root(&self) -> Option<&SceneRoot> {
        match self.pipeline.as_ref() {
            Some(Pipeline::Regular(_, root, _, _, _)) => root.as_ref(),
            _ => None,
        }
    }

    pub(super) fn scene_resource(&self, list: &super::List) -> Result<SceneCheck> {
        let scene = self.scene_root().ok_or(E_INVALIDARG)?;
        if !self
            .layout
            .as_ref()
            .is_some_and(|layout| layout.matches(&scene.original))
        {
            return Err(E_INVALIDARG.into());
        }
        let owner = list.parent.as_ref().ok_or(E_INVALIDARG)?;
        let location = match scene.source {
            SceneSource::Table { slot, offset } => {
                SceneLocation::Table(self.table_entry(slot, offset)?)
            }
            SceneSource::RootSrv(slot) => {
                let Some(Argument::Buffer(BufferView::Srv(address))) = self.compute_args.get(&slot) else {
                    return Err(E_INVALIDARG.into());
                };
                SceneLocation::Address(*address)
            }
        };
        let backing = unsafe { location.resolve(owner)? };
        let geometry = if scene.program.triangle_geometry {
            Some(unsafe { crate::device::blas::inline_geometry(owner, backing.1)? })
        } else {
            None
        };
        Ok(SceneCheck {
            location,
            scene: backing,
            geometry,
        })
    }

    pub(super) fn refresh_inline_tables<const COMPUTE: bool>(
        &self, arena: Option<&crate::device::descriptor_paging::Arena>,
    ) -> Result<()> {
        let Some(arena) = arena.filter(|arena| arena.uses_inline_records()) else {
            return Ok(());
        };
        let Some(lengths) = self.paged_layout::<COMPUTE>() else { return Ok(()); };
        for (slot, length) in lengths.iter().enumerate() {
            let Some(length) = length else { continue; };
            let slot = slot as u32;
            let bound = if COMPUTE {
                match self.compute_args.get(&slot) {
                    Some(Argument::Table(bound)) => Some(bound),
                    _ => None,
                }
            } else {
                self.graphics_tables.get(&slot)
            };
            if let Some(bound) = bound.filter(|bound| arena.contains(bound.logical.ptr)) {
                arena.gpu_table(bound.logical.ptr, *length, self.inline_ranges::<COMPUTE>(slot as usize), self.null_kinds().map(|k| (k, slot as usize)))?;
            }
        }
        Ok(())
    }

    pub(super) fn raster_inputs(&self, heap: Option<&super::descriptors::ResourceHeap>, arena: Option<&crate::device::descriptor_paging::Arena>, display: bool) -> String {
        use crate::diagnostics::display;
        if !display::trace_views() { return String::new(); }
        let mut views = Vec::new();
        let mut buffers = Vec::new();
        let mut describe = |slot: u32, offsets: &std::ops::Range<u32>| {
            let Some(heap) = heap else { return; };
            let Some(base) = self.graphics_tables.get(&slot) else { return; };
            let native = match arena.filter(|arena| arena.contains(base.logical.ptr)) {
                Some(arena) => {
                    let length = self.reflected_table::<false>().and_then(|table| table.lengths.get(slot as usize).copied())
                        .or_else(|| self.paged_layout::<false>()?.get(slot as usize).copied().flatten());
                    let Some(handle) = length.and_then(|length| arena.resident_gpu(base.logical.ptr, length)) else { return; };
                    handle
                },
                None => base.native,
            };
            if let Some(cpu) = heap.table(native.ptr, offsets) {
                views.extend(display::texture_views(cpu.clone(), heap.table_stride()).into_iter().map(|view| (slot, view)));
                buffers.extend(display::buffer_views(cpu, heap.table_stride()).into_iter().map(|view| (slot, view)));
            }
        };
        if let Some(table) = self.reflected_table::<false>() {
            for (slot, offsets) in &table.accessed_views {
                describe(*slot as u32, offsets);
            }
        } else if let Some(ranges) = self.graphics_layout.as_ref().and_then(|layout| layout.placed_tables.as_ref()) {
            for (slot, offsets) in ranges.iter() {
                describe(*slot, offsets);
            }
        }
        if buffers.is_empty() {
            format!(" display={display} texture views {views:?}")
        } else {
            format!(" display={display} texture views {views:?} buffer views {buffers:?}")
        }
    }

    pub(super) fn sample_tables<const COMPUTE: bool>(
        &self,
        heap: &super::descriptors::ResourceHeap,
        sampled: &mut Vec<std::ops::Range<usize>>,
    ) -> Option<()> {
        let layout = if COMPUTE { self.layout.as_ref() } else { self.graphics_layout.as_ref() }?;
        for (slot, range) in layout.placed_tables.as_ref()?.iter() {
            let base = if COMPUTE {
                let Argument::Table(base) = self.compute_args.get(slot)? else { return None };
                base
            } else {
                self.graphics_tables.get(slot)?
            };
            super::descriptors::include_range(sampled, heap.table(base.native.ptr, range)?);
        }
        Some(())
    }

    /// CPU descriptor ranges of the UAV table ranges the bound layout declares
    /// (TUXBOX_SHADOW_UAV_REMARK): the views this draw or dispatch may write.
    pub(super) fn uav_tables<const COMPUTE: bool>(
        &self,
        heap: &super::descriptors::ResourceHeap,
        written: &mut Vec<std::ops::Range<usize>>,
    ) {
        let Some(layout) = (if COMPUTE { self.layout.as_ref() } else { self.graphics_layout.as_ref() }) else { return };
        for (slot, range) in layout.uav_tables.iter() {
            let base = if COMPUTE {
                let Some(Argument::Table(base)) = self.compute_args.get(slot) else { continue };
                base
            } else {
                let Some(base) = self.graphics_tables.get(slot) else { continue };
                base
            };
            if let Some(cpu) = heap.table(base.native.ptr, range) {
                super::descriptors::include_range(written, cpu);
            }
        }
    }

    /// CPU handles of the heap descriptors the bound program writes through
    /// direct heap indexing, from its reflection (`<key>.heap`): a literal
    /// index, a root constant plus add, or a table entry. A per-lane or root
    /// CBV source takes the whole heap.
    pub(super) fn heap_writes<const COMPUTE: bool>(
        &self,
        heap: &super::descriptors::ResourceHeap,
        written: &mut Vec<std::ops::Range<usize>>,
        fallback: &mut Vec<std::ops::Range<usize>>,
        touched: &mut Vec<std::ops::Range<usize>>,
        dynamic: &mut Vec<std::ops::Range<usize>>,
    ) {
        use crate::device::shader_cache::HeapSource;
        let Some(access) = self.heap_access.as_ref() else { return };
        let slot = |index: i64| -> Option<std::ops::Range<usize>> {
            let index = u32::try_from(index).ok()?;
            heap.table(heap.gpu_start(), &(index..index.checked_add(1)?))
        };
        // Every exact source (read or written) also goes to `touched`, the
        // list's sampled heap ranges placement::submit hands the shadow
        // eviction (Coder5's touch_views): direct heap reads were invisible to it.
        for source in access.reads.iter().chain(&access.writes) {
            if matches!(source, HeapSource::Dynamic | HeapSource::Cbv(..)) {
                super::descriptors::include_range(dynamic, heap.cpu.clone());
            }
            let range = match *source {
                HeapSource::Const(index) => slot(i64::from(index)),
                HeapSource::Root(param, dword, add) => {
                    let value = if COMPUTE {
                        match self.compute_args.get(&param) { Some(Argument::Constants(words)) => words.get(&dword).copied(), _ => None }
                    } else {
                        self.graphics_constants.get(&(param, dword)).copied()
                    };
                    value.and_then(|v| slot(i64::from(v) + add))
                }
                _ => None,
            };
            if let Some(range) = range {
                super::descriptors::include_range(touched, range);
            }
        }
        for source in &access.writes {
            let range = match *source {
                HeapSource::Const(index) => slot(i64::from(index)),
                HeapSource::Root(param, dword, add) => {
                    let value = if COMPUTE {
                        match self.compute_args.get(&param) {
                            Some(Argument::Constants(words)) => words.get(&dword).copied(),
                            _ => None,
                        }
                    } else {
                        self.graphics_constants.get(&(param, dword)).copied()
                    };
                    value.and_then(|v| slot(i64::from(v) + add))
                }
                HeapSource::Table(param, byte) => {
                    let base = if COMPUTE {
                        match self.compute_args.get(&param) { Some(Argument::Table(base)) => Some(base), _ => None }
                    } else {
                        self.graphics_tables.get(&param)
                    };
                    base.and_then(|base| heap.table(base.native.ptr, &(byte / 32..byte / 32 + 1)))
                }
                HeapSource::Cbv(..) | HeapSource::Dynamic => {
                    static SAID: std::sync::Once = std::sync::Once::new();
                    crate::diagnostics::log_once(&SAID, || "shadow uav re-mark: a program writes the heap through a per-lane or root CBV index; its draws and dispatches re-mark the heap's shadow UAVs a capped share per write-back (TUXBOX_SHADOW_REMARK_FALLBACK_MB)".to_string());
                    super::descriptors::include_range(fallback, heap.cpu.clone());
                    None
                }
            };
            if let Some(range) = range {
                super::descriptors::include_range(written, range);
            }
        }
    }

    fn table_entry(&self, slot: u32, table_offset: u32) -> Result<D3D12_CPU_DESCRIPTOR_HANDLE> {
        let Some(Argument::Table(base)) = self.compute_args.get(&slot) else {
            return Err(E_INVALIDARG.into());
        };
        let heap = self
            .heaps
            .iter()
            .flatten()
            .find(|heap| unsafe { heap.GetDesc().Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV })
            .ok_or(E_INVALIDARG)?;
        let mut native = None::<ID3D12Device>;
        unsafe { heap.GetDevice(&mut native)? };
        let stride = u64::from(unsafe {
            native
                .ok_or(E_INVALIDARG)?
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
        });
        let start = unsafe { heap.GetGPUDescriptorHandleForHeapStart().ptr };
        let offset = base.native.ptr.checked_sub(start).ok_or(E_INVALIDARG)?;
        if stride == 0 || !offset.is_multiple_of(stride) {
            return Err(E_INVALIDARG.into());
        }
        let slot = (offset / stride)
            .checked_add(u64::from(table_offset))
            .ok_or(E_INVALIDARG)?;
        if slot >= u64::from(unsafe { heap.GetDesc().NumDescriptors }) {
            return Err(E_INVALIDARG.into());
        }
        let cpu_start = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };
        let offset = slot
            .checked_mul(stride)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(E_INVALIDARG)?;
        let ptr = cpu_start.ptr.checked_add(offset).ok_or(E_INVALIDARG)?;
        Ok(D3D12_CPU_DESCRIPTOR_HANDLE { ptr })
    }

    pub(super) unsafe fn dispatch_scene(
        &self,
        host: &ID3D12GraphicsCommandList,
        root: &SceneRoot,
        va: u64,
        geometry: Option<[u64; 2]>,
        dispatch: impl FnOnce(),
    ) {
        unsafe {
            host.SetComputeRootSignature(&root.native);
            self.apply_compute_args(host, &[]);
            host.SetComputeRootShaderResourceView(root.appended, va);
            if let Some([instances, normals]) = geometry {
                host.SetComputeRootShaderResourceView(root.appended + 1, instances);
                host.SetComputeRootShaderResourceView(root.appended + 2, normals);
            }
            dispatch();
            host.SetComputeRootSignature(self.compute.as_ref());
            self.apply_compute_args(host, &[]);
        }
    }

    /// Every binding on a new host list (split): heaps, both root
    /// signatures with their arguments, and the pipeline.
    pub(super) unsafe fn restore_all(&self, host: &ID3D12GraphicsCommandList) {
        unsafe {
            if !self.heaps.is_empty() {
                host.SetDescriptorHeaps(&self.heaps);
            }
            if self.compute.is_some() {
                host.SetComputeRootSignature(self.compute.as_ref());
                self.apply_compute_args(host, &[]);
            }
            if host.GetType() == D3D12_COMMAND_LIST_TYPE_DIRECT && self.graphics.is_some() {
                host.SetGraphicsRootSignature(self.graphics.as_ref());
                self.apply_graphics_arguments(host);
                for (&slot, &base) in &self.graphics_tables {
                    host.SetGraphicsRootDescriptorTable(slot, base.native);
                }
            }
            match self.pipeline.as_ref() {
                Some(Pipeline::Regular(pso, ..)) => host.SetPipelineState(pso),
                Some(Pipeline::Ray(pso)) => {
                    if let Ok(host) = windows::core::Interface::cast::<ID3D12GraphicsCommandList4>(host) {
                        host.SetPipelineState1(pso);
                    }
                }
                None => {}
            }
        }
    }

    pub(super) unsafe fn restore(&self, host: &ID3D12GraphicsCommandList4) {
        unsafe {
            host.SetDescriptorHeaps(&self.heaps);
            host.SetComputeRootSignature(self.compute.as_ref());
            self.apply_compute_args(host, &[]);
            for (&slot, &base) in &self.graphics_tables {
                host.SetGraphicsRootDescriptorTable(slot, base.native);
            }
            match self.pipeline.as_ref() {
                Some(Pipeline::Regular(pso, _, _, _, _)) => host.SetPipelineState(pso),
                Some(Pipeline::Ray(pso)) => host.SetPipelineState1(pso),
                None => {}
            }
        }
    }
}
