//! Graphics-list methods whose slot is the same on every Xbox generation.
//! Slots after EndEvent are filled from retail traces and are not renumbered here.

use core::ffi::c_void;

use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B4G4R4A4_UNORM, DXGI_FORMAT_UNKNOWN,
};
use windows::core::{HRESULT, Interface};

use super::{List, ListState, Recording, parent, recording};

pub(super) unsafe extern "system" fn clear_state(list: &List, pipeline: *mut c_void) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    // Bundles and copy lists have no graphics state to restore.
    if !matches!(
        unsafe { list.native.GetType() },
        D3D12_COMMAND_LIST_TYPE_DIRECT | D3D12_COMMAND_LIST_TYPE_COMPUTE
    ) {
        state.error = E_INVALIDARG;
        return;
    }
    // A null pipeline is the cleared state. An unknown object is not.
    let pipeline = match unsafe { crate::device::compute::pipeline(pipeline, parent(&list.parent)) } {
        Ok(pipeline) => pipeline,
        Err(error) => {
            state.error = error;
            return;
        }
    };
    // Draw and dispatch replay these bindings. Clear them with the host state,
    // or the next draw puts the old tables back.
    state.bindings = super::compute_bindings::Bindings::new(pipeline);
    state.stencil_slot = pipeline.and_then(|pipeline| pipeline.state.stencil_slot);
    if let Some(targets) = &mut state.stencil_targets {
        targets.unbind();
    }
    state.dsv_bound = false;
    state.display_bound = false;
    state.trace_target = None;
    state.bound_placed_targets.clear();
    state.resource_heap = None;
    state.descriptor_arena = None;
    // Earlier draws in this recording still reference their paged GPU handles,
    // so descriptor_leases stays pinned until Reset() or Drop.
    // Desktop ClearState takes no pipeline. The guest argument is bound after
    // the reset so a following draw sees that pipeline.
    unsafe { list.native.ClearState(None::<&ID3D12PipelineState>) };
    if let Some(pipeline) = pipeline {
        unsafe { list.native.SetPipelineState(&pipeline.native) };
    }
}

pub(super) unsafe extern "system" fn copy_texture_region(
    list: &List,
    dst: *const super::CopyLocation,
    x: u32,
    y: u32,
    z: u32,
    src: *const super::CopyLocation,
    region: *const D3D12_BOX,
) {
    unsafe { super::copy_texture(list, dst, x, y, z, src, region, 0) }
}

pub(super) unsafe extern "system" fn copy_whole_resource(
    list: &List,
    dst: *mut c_void,
    src: *mut c_void,
) {
    unsafe { super::copy_resource(list, dst, src, 0) }
}

fn tile_copy_flags(flags: D3D12_TILE_COPY_FLAGS) -> Result<(), HRESULT> {
    let known = D3D12_TILE_COPY_FLAG_NO_HAZARD.0
        | D3D12_TILE_COPY_FLAG_LINEAR_BUFFER_TO_SWIZZLED_TILED_RESOURCE.0
        | D3D12_TILE_COPY_FLAG_SWIZZLED_TILED_RESOURCE_TO_LINEAR_BUFFER.0;
    if flags.0 & !known != 0 {
        return Err(E_NOTIMPL);
    }
    let both = D3D12_TILE_COPY_FLAG_LINEAR_BUFFER_TO_SWIZZLED_TILED_RESOURCE.0
        | D3D12_TILE_COPY_FLAG_SWIZZLED_TILED_RESOURCE_TO_LINEAR_BUFFER.0;
    if flags.0 & both == both {
        return Err(E_INVALIDARG);
    }
    Ok(())
}

fn tile_count(size: &D3D12_TILE_REGION_SIZE) -> Result<u64, HRESULT> {
    if size.UseBox.as_bool() {
        if size.Width == 0 || size.Height == 0 || size.Depth == 0 {
            return Err(E_INVALIDARG);
        }
        return u64::from(size.Width)
            .checked_mul(u64::from(size.Height))
            .and_then(|tiles| tiles.checked_mul(u64::from(size.Depth)))
            .ok_or(E_INVALIDARG);
    }
    if size.NumTiles == 0 {
        return Err(E_INVALIDARG);
    }
    Ok(u64::from(size.NumTiles))
}

fn reserved_resource(resource: &ID3D12Resource) -> Result<bool, HRESULT> {
    let mut properties = D3D12_HEAP_PROPERTIES::default();
    // A reserved resource has no heap. GetHeapProperties reports that as E_INVALIDARG.
    match unsafe { resource.GetHeapProperties(Some(core::ptr::from_mut(&mut properties)), None) } {
        Err(error) if error.code() == E_INVALIDARG => Ok(true),
        Ok(()) => Ok(false),
        Err(error) => Err(error.code()),
    }
}

pub(super) unsafe extern "system" fn copy_tiles(
    list: &List,
    tiled: *mut c_void,
    coordinate: *const D3D12_TILED_RESOURCE_COORDINATE,
    region: *const D3D12_TILE_REGION_SIZE,
    buffer: *mut c_void,
    offset: u64,
    flags: D3D12_TILE_COPY_FLAGS,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if let Err(error) = tile_copy_flags(flags) {
        if error == E_NOTIMPL {
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || format!("CopyTiles flags {:#x}", flags.0));
        }
        state.error = error;
        return;
    }
    let Some(region) = (unsafe { region.as_ref() }) else {
        state.error = E_INVALIDARG;
        return;
    };
    if coordinate.is_null() {
        state.error = E_INVALIDARG;
        return;
    }
    let Ok(tiles) = tile_count(region) else {
        state.error = E_INVALIDARG;
        return;
    };
    let Some(bytes) = tiles.checked_mul(u64::from(D3D12_TILED_RESOURCE_TILE_SIZE_IN_BYTES)) else {
        state.error = E_INVALIDARG;
        return;
    };
    if !offset.is_multiple_of(u64::from(D3D12_TILED_RESOURCE_TILE_SIZE_IN_BYTES)) {
        state.error = E_INVALIDARG;
        return;
    }
    let owner = parent(&list.parent);
    let (Some(tiled), Some(buffer)) = (
        unsafe { crate::device::resource_proxy(tiled, owner) },
        unsafe { crate::device::resource_proxy(buffer, owner) },
    ) else {
        state.error = E_INVALIDARG;
        return;
    };
    let Some(offset) = super::compute::buffer_offset(buffer, offset, bytes) else {
        state.error = E_INVALIDARG;
        return;
    };
    match reserved_resource(&tiled.native) {
        Ok(true) => {}
        Ok(false) => {
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || "CopyTiles resource is not reserved".to_string());
            state.error = E_INVALIDARG;
            return;
        }
        Err(error) => {
            state.error = error;
            return;
        }
    }
    if let Err(error) = state.prepare_copy(tiled.state.placed(), buffer.state.placed()) {
        state.error = error;
        return;
    }
    unsafe {
        list.native.CopyTiles(
            &tiled.native,
            coordinate,
            region,
            &buffer.native,
            offset,
            flags,
        )
    };
}

fn resolve_format(format: DXGI_FORMAT) -> bool {
    format == DXGI_FORMAT_UNKNOWN || (1..=DXGI_FORMAT_B4G4R4A4_UNORM.0).contains(&format.0)
}

pub(super) unsafe extern "system" fn resolve_subresource(
    list: &List,
    dst: *mut c_void,
    dst_subresource: u32,
    src: *mut c_void,
    src_subresource: u32,
    format: DXGI_FORMAT,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if !resolve_format(format) {
        state.error = E_INVALIDARG;
        return;
    }
    let owner = parent(&list.parent);
    let (Some(dst), Some(src)) = (
        unsafe { crate::device::resource_proxy(dst, owner) },
        unsafe { crate::device::resource_proxy(src, owner) },
    ) else {
        state.error = E_INVALIDARG;
        return;
    };
    let (destination, source) = unsafe { (dst.native.GetDesc(), src.native.GetDesc()) };
    if dst.native == src.native
        || destination.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER
        || source.Dimension != destination.Dimension
        || destination.Width != source.Width
        || destination.Height != source.Height
        || source.SampleDesc.Count <= 1
        || destination.SampleDesc.Count != 1
    {
        state.error = E_INVALIDARG;
        return;
    }
    if let Err(error) = state.prepare_copy(dst.state.placed(), src.state.placed()) {
        state.error = error;
        return;
    }
    unsafe {
        list.native
            .ResolveSubresource(&dst.native, dst_subresource, &src.native, src_subresource, format)
    };
}

fn predication_op(operation: D3D12_PREDICATION_OP) -> Result<(), HRESULT> {
    if matches!(
        operation,
        D3D12_PREDICATION_OP_EQUAL_ZERO | D3D12_PREDICATION_OP_NOT_EQUAL_ZERO
    ) {
        Ok(())
    } else {
        Err(E_INVALIDARG)
    }
}

pub(super) unsafe extern "system" fn set_predication(
    list: &List,
    buffer: *mut c_void,
    offset: u64,
    operation: D3D12_PREDICATION_OP,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if let Err(error) = predication_op(operation) {
        state.error = error;
        return;
    }
    if buffer.is_null() {
        unsafe { list.native.SetPredication(None::<&ID3D12Resource>, offset, operation) };
        return;
    }
    let Some(buffer) = (unsafe { crate::device::resource_proxy(buffer, parent(&list.parent)) }) else {
        state.error = E_INVALIDARG;
        return;
    };
    // The predicate is one 64-bit value.
    if !offset.is_multiple_of(8) {
        state.error = E_INVALIDARG;
        return;
    }
    let Some(offset) = super::compute::buffer_offset(buffer, offset, 8) else {
        state.error = E_INVALIDARG;
        return;
    };
    unsafe { list.native.SetPredication(&buffer.native, offset, operation) };
}

fn stream_output_range(start: u32, count: u32) -> Result<(), HRESULT> {
    if start > D3D12_SO_BUFFER_SLOT_COUNT || count > D3D12_SO_BUFFER_SLOT_COUNT - start {
        Err(E_INVALIDARG)
    } else {
        Ok(())
    }
}

pub(super) unsafe extern "system" fn stream_output(
    list: &List,
    start: u32,
    count: u32,
    views: *const D3D12_STREAM_OUTPUT_BUFFER_VIEW,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if let Err(error) = stream_output_range(start, count) {
        state.error = error;
        return;
    }
    if views.is_null() || count == 0 {
        unsafe { (list.native.vtable().SOSetTargets)(list.native.as_raw(), start, count, views) };
        return;
    }
    let mut translated = [D3D12_STREAM_OUTPUT_BUFFER_VIEW::default(); D3D12_SO_BUFFER_SLOT_COUNT as usize];
    let host = &mut translated[..count as usize];
    host.copy_from_slice(unsafe { core::slice::from_raw_parts(views, count as usize) });
    for view in host.iter_mut() {
        if view.BufferLocation == 0 && view.SizeInBytes == 0 && view.BufferFilledSizeLocation == 0 {
            continue;
        }
        if view.BufferLocation == 0
            || view.SizeInBytes == 0
            || !view.BufferLocation.is_multiple_of(4)
        {
            state.error = E_INVALIDARG;
            return;
        }
        let Some(location) = super::compute::address(list, &mut state, view.BufferLocation, view.SizeInBytes) else {
            return;
        };
        view.BufferLocation = location;
        if view.BufferFilledSizeLocation != 0 {
            if !view.BufferFilledSizeLocation.is_multiple_of(4) {
                state.error = E_INVALIDARG;
                return;
            }
            let Some(filled) = super::compute::address(list, &mut state, view.BufferFilledSizeLocation, 4) else {
                return;
            };
            view.BufferFilledSizeLocation = filled;
        }
    }
    unsafe { (list.native.vtable().SOSetTargets)(list.native.as_raw(), start, count, host.as_ptr()) };
}

fn adopt_bundle(parent_state: &mut Recording, bundle: &Recording) -> Result<(), HRESULT> {
    // Queue splits and raytracing replay are applied to the submitted list. A bundle is not submitted on its own, so those recordings
    // cannot be replayed by ExecuteBundle yet.
    if !bundle.prefix.segments.is_empty() || !bundle.scene_commands.is_empty() {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || {
            format!(
                "ExecuteBundle has {} waits, {} scene commands",
                bundle.prefix.segments.len(),
                bundle.scene_commands.len()
            )
        });
        return Err(E_NOTIMPL);
    }
    parent_state
        .sampled_heaps
        .try_reserve(bundle.sampled_heaps.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    for texture in &bundle.placed_reads {
        Recording::track_texture(&mut parent_state.placed_reads, texture)?;
    }
    for texture in &bundle.placed_writes {
        Recording::track_texture(&mut parent_state.placed_writes, texture)?;
    }
    for range in &bundle.sampled_heaps {
        super::descriptors::include_range(&mut parent_state.sampled_heaps, range.clone());
    }
    for arena in &bundle.indexed_arenas {
        if parent_state.indexed_arenas.iter().any(|used| std::sync::Arc::ptr_eq(used, arena)) {
            continue;
        }
        parent_state.indexed_arenas.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
        parent_state.indexed_arenas.push(std::sync::Arc::clone(arena));
    }
    parent_state
        .descriptor_leases
        .extend_from(&bundle.descriptor_leases)?;
    parent_state
        .scalar_descriptors
        .try_reserve(bundle.scalar_descriptors.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    parent_state.scalar_descriptors.extend(bundle.scalar_descriptors.iter().copied());
    parent_state
        .trace_samples
        .try_reserve(bundle.trace_samples.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    parent_state.trace_samples.extend(bundle.trace_samples.iter().cloned());
    Ok(())
}

pub(super) unsafe extern "system" fn execute_bundle(list: &List, bundle: *mut c_void) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if unsafe { list.native.GetType() } != D3D12_COMMAND_LIST_TYPE_DIRECT {
        state.error = E_INVALIDARG;
        return;
    }
    let Some(bundle) = (unsafe { super::guest_list(bundle, parent(&list.parent)) }) else {
        state.error = E_INVALIDARG;
        return;
    };
    if core::ptr::eq(list, bundle) {
        state.error = E_INVALIDARG;
        return;
    }
    // Lock the caller first, then the bundle. The two lists are distinct.
    let bundle_state = recording(bundle);
    if unsafe { bundle.native.GetType() } != D3D12_COMMAND_LIST_TYPE_BUNDLE
        || !matches!(&bundle_state.phase, ListState::Closed)
        || bundle_state.error.is_err()
    {
        state.error = E_INVALIDARG;
        return;
    }
    if let Err(error) = adopt_bundle(&mut state, &bundle_state) {
        state.error = error;
        return;
    }
    unsafe { list.native.ExecuteBundle(&bundle.native) };
}

fn event_data(data: *const c_void, size: u32) -> Result<(), HRESULT> {
    if size != 0 && data.is_null() {
        Err(E_INVALIDARG)
    } else {
        Ok(())
    }
}

pub(super) unsafe extern "system" fn set_marker(
    list: &List,
    _metadata: u32,
    data: *const c_void,
    size: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if let Err(error) = event_data(data, size) {
        state.error = error;
    }
}

pub(super) unsafe extern "system" fn begin_event(
    list: &List,
    _metadata: u32,
    data: *const c_void,
    size: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if let Err(error) = event_data(data, size) {
        state.error = error;
    }
}

pub(super) unsafe extern "system" fn end_event(list: &List) {
    let mut state = recording(list);
    let _ = state.can_record(&list.native);
}

#[cfg(test)]
#[path = "../tests/commands/standard.rs"]
mod tests;
