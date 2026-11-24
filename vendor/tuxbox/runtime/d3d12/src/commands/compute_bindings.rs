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
    heaps: Vec<Option<ID3D12DescriptorHeap>>,
    compute: Option<ID3D12RootSignature>,
    layout: Option<RootSource>,
    graphics: Option<ID3D12RootSignature>,
    graphics_layout: Option<RootSource>,
    compute_args: BTreeMap<u32, Argument>,
    graphics_buffers: BTreeMap<u32, BufferView>,
    graphics_constants: BTreeMap<(u32,u32),u32>,
    graphics_tables: BTreeMap<u32, DescriptorTable>,
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
    Regular(ID3D12PipelineState, Option<SceneRoot>, Option<Arc<ComputeTable>>, Option<Arc<ComputeTable>>),
    Ray(ID3D12StateObject),
}

#[derive(Clone, Copy)]
pub(super) enum BufferView {
    Cbv(u64),
    Srv(u64),
    Uav(u64),
}

impl BufferView {
    pub(super) unsafe fn apply<const COMPUTE: bool>(self, list: &ID3D12GraphicsCommandList, slot: u32) {
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
        Self::Regular(p.native.clone(), p.state.scene_root.clone(), p.state.reflected.clone(), p.state.raster_table.clone())
    }
}

impl Bindings {
    pub(super) fn new(initial: Option<&GuestPipeline>) -> Self {
        Self {
            pipeline: initial.map(Pipeline::from),
            draw_trace: initial.and_then(|p| p.state.draw_trace.clone()),
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
            self.graphics = root.cloned();
        }
    }

    pub(super) fn heaps(&mut self, heaps: &[Option<ID3D12DescriptorHeap>]) {
        if self.heaps.len() != heaps.len() || heaps.iter().any(|heap| !self.heaps.contains(heap)) {
            self.compute_args
                .retain(|_, arg| !matches!(arg, Argument::Table(_)));
            self.graphics_tables.clear();
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
                    Argument::Buffer(view) => view.apply::<true>(host, slot),
                    Argument::Constants(words) => {
                        for (&index, &value) in words {
                            host.SetComputeRoot32BitConstant(slot, value, index);
                        }
                    }
                }
            }
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
            unsafe { view.apply::<false>(list, slot); }
        }
        for (&(slot, offset), &value) in &self.graphics_constants {
            unsafe { list.SetGraphicsRoot32BitConstant(slot, value, offset); }
        }
    }

    pub(super) fn reflected_table<const COMPUTE: bool>(&self) -> Option<&ComputeTable> {
        match &self.pipeline {
            Some(Pipeline::Regular(_, _, compute, raster)) => if COMPUTE { compute.as_deref() } else { raster.as_deref() },
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
                Some(arena) => arena.gpu_table(bound.logical.ptr, length, table.layout.inline_ranges(index))?,
                None => bound.native,
            });
        }
        let heap = heap.ok_or(E_INVALIDARG)?;
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
        let list = &guest.native;
        unsafe {
            if COMPUTE {
                list.SetComputeRootSignature(&table.native);
                // Changing root signatures invalidates the native arguments. Replay
                // constants and descriptors, then substitute each reflected table.
                self.apply_compute_args(list, bases);
                dispatch();
                list.SetComputeRootSignature(self.compute.as_ref());
                self.apply_compute_args(list, &[]);
            } else {
                list.SetGraphicsRootSignature(&table.native);
                self.apply_graphics_arguments(list);
                for (&slot, bound) in &self.graphics_tables {
                    let base = bases.get(slot as usize).copied().flatten().unwrap_or(bound.native);
                    list.SetGraphicsRootDescriptorTable(slot, base);
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
            Some(Pipeline::Regular(_, root, _, _)) => root.as_ref(),
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
                arena.gpu_table(bound.logical.ptr, *length, self.inline_ranges::<COMPUTE>(slot as usize))?;
            }
        }
        Ok(())
    }

    pub(super) fn raster_inputs(&self, heap: Option<&super::descriptors::ResourceHeap>, arena: Option<&crate::device::descriptor_paging::Arena>, display: bool) -> String {
        use crate::diagnostics::display;
        if !display::trace_views() { return String::new(); }
        let mut views = Vec::new();
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
                views.extend(display::texture_views(cpu, heap.table_stride()).into_iter().map(|view| (slot, view)));
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
        format!(" display={display} texture views {views:?}")
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

    pub(super) unsafe fn restore(&self, host: &ID3D12GraphicsCommandList4) {
        unsafe {
            host.SetDescriptorHeaps(&self.heaps);
            host.SetComputeRootSignature(self.compute.as_ref());
            self.apply_compute_args(host, &[]);
            for (&slot, &base) in &self.graphics_tables {
                host.SetGraphicsRootDescriptorTable(slot, base.native);
            }
            match self.pipeline.as_ref() {
                Some(Pipeline::Regular(pso, _, _, _)) => host.SetPipelineState(pso),
                Some(Pipeline::Ray(pso)) => host.SetPipelineState1(pso),
                None => {}
            }
        }
    }
}
