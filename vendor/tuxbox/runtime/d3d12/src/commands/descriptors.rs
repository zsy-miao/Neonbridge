//! Descriptor heap binding and compute/graphics root tables in the Xbox list.
use super::{List, parent, recording};
use crate::device;
use core::{ffi::c_void, ptr, slice};
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use core::ops::Range;

pub(super) struct ResourceHeap {
    pub(super) cpu: Range<usize>,
    gpu: u64,
    stride: usize,
    // The guest heap owns its descriptor arena and placed-view metadata.
    guest: ID3D12Pageable,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct ScalarDescriptor {
    pub(super) index: u32,
    pub(super) cpu: usize,
}

impl ResourceHeap {
    pub(super) fn scalar_descriptor(
        &self, index: u32, arena: Option<&device::descriptor_paging::Arena>,
    ) -> Option<ScalarDescriptor> {
        let offset = (index as usize).checked_mul(self.stride)?;
        let cpu = if let Some(arena) = arena {
            let logical = arena.base().checked_add(offset)?;
            if !arena.contains(logical as u64) { return None; }
            arena.cpu(logical).ptr
        } else {
            let handle = self.cpu.start.checked_add(offset)?;
            if !self.cpu.contains(&handle) { return None; }
            handle
        };
        Some(ScalarDescriptor { index, cpu })
    }

    pub(super) fn index_map(
        &self, list: &List, arena: Option<&device::descriptor_paging::Arena>,
        resources: &device::MediaResources,
    ) -> windows::core::Result<u64> {
        let parent = list.parent.as_ref().ok_or(E_INVALIDARG)?;
        // All command list proxies own a reference to their creating Device.
        let map = unsafe { device::descriptor_paging::index_map(parent, arena)? };
        let address = unsafe { map.GetGPUVirtualAddress() };
        if address == 0 { return Err(E_INVALIDARG.into()); }
        let mut held = resources.lock().unwrap_or_else(|e| e.into_inner());
        // SetDescriptorHeaps and command-list Reset can precede GPU execution.
        // The allocator keeps the map, shader-visible heap and its aliases alive.
        for object in [&self.guest, &ID3D12Pageable::from(map)] {
            if !held.contains(object) {
                held.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
                held.push(object.clone());
            }
        }
        Ok(address)
    }

    pub(super) fn table_stride(&self) -> usize { self.stride }

    pub(super) fn table(&self, base: u64, slots: &Range<u32>) -> Option<Range<usize>> {
        let offset = usize::try_from(base.checked_sub(self.gpu)?).ok()?;
        if self.stride == 0 || !offset.is_multiple_of(self.stride) { return None; }
        let base = self.cpu.start.checked_add(offset)?;
        let address = |slot: u32| base.checked_add((slot as usize).checked_mul(self.stride)?);
        let range = address(slots.start)?..address(slots.end)?;
        (range.end <= self.cpu.end).then_some(range)
    }
}

/// Keep submitted ranges sorted and coalesced even if a table is rebound.
pub(super) fn include_range(sampled: &mut Vec<Range<usize>>, range: Range<usize>) {
    let first = sampled.partition_point(|prior| prior.end < range.start);
    let count = sampled[first..].partition_point(|prior| prior.start <= range.end);
    if count == 0 {
        sampled.insert(first, range);
    } else {
        sampled[first].start = sampled[first].start.min(range.start);
        sampled[first].end = sampled[first + count - 1].end.max(range.end);
        sampled.drain(first + 1..first + count);
    }
}

pub(super) unsafe extern "system" fn bind_heaps(
    list: &List,
    count: u32,
    heaps: *const *mut c_void,
) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) {
        return;
    }
    if count > 2 || (count != 0 && heaps.is_null()) {
        state.error = E_INVALIDARG;
        return;
    }
    let guest = if count == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(heaps, count as usize) }
    };
    let mut native = [ptr::null_mut(); 2];
    let mut retained = [None, None];
    let mut previous = None;
    let mut resource_heap = None;
    let mut descriptor_arena = None;
    for (index, &object) in guest.iter().enumerate() {
        let Some(heap) = (unsafe { device::descriptor_heap(object, parent(&list.parent)) }) else {
            state.error = E_INVALIDARG;
            return;
        };
        let desc = &heap.state.desc;
        if desc.Flags != D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE || previous == Some(desc.Type) {
            state.error = E_INVALIDARG;
            return;
        }
        if let Some(views) = &heap.state.placed_views {
            descriptor_arena = heap.state.arena.clone();
            resource_heap = Some(ResourceHeap {
                // descriptor_heap validated this guest pageable and its device.
                guest: unsafe { ID3D12Pageable::from_raw_borrowed(&object) }.unwrap().clone(),
                cpu: views.range.clone(),
                gpu: unsafe { heap.native.GetGPUDescriptorHandleForHeapStart() }.ptr,
                stride: views.range.len() / unsafe { heap.native.GetDesc() }.NumDescriptors.max(1) as usize,
            });
        }
        previous = Some(desc.Type);
        native[index] = heap.native.as_raw();
        retained[index] = Some(heap.native.clone());
    }
    state.resource_heap = resource_heap;
    state.descriptor_arena = descriptor_arena;
    state.bindings.heaps(&retained[..count as usize]);
    unsafe {
        (list.native.vtable().SetDescriptorHeaps)(list.native.as_raw(), count, native.as_ptr())
    };
}

pub(super) unsafe extern "system" fn table<const COMPUTE: bool>(
    list: &List,
    index: u32,
    base: D3D12_GPU_DESCRIPTOR_HANDLE,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let logical = base;
    let base = if let Some(arena) = &state.descriptor_arena && arena.contains(base.ptr) {
        let Some(Some(length)) = state.bindings.paged_layout::<COMPUTE>().and_then(|t| t.get(index as usize)) else {
            state.error = windows::Win32::Foundation::E_NOTIMPL;
            return;
        };
        // The selected PSO may supply a more precise descriptor layout.
        // Defer reading guest records until draw or dispatch.
        match arena.gpu_table(base.ptr, *length, &[]) {
            Ok(handle) => handle,
            Err(error) => { state.error = error.code(); return; }
        }
    } else { base };
    state.bindings.table::<COMPUTE>(index, base, logical);
    if COMPUTE {
        unsafe { list.native.SetComputeRootDescriptorTable(index, base) };
    } else {
        unsafe { list.native.SetGraphicsRootDescriptorTable(index, base) };
    }
}
