//! Hand off the float depth plane between depth testing and unordered access
//! without moving or reinterpreting stencil. All commands use the guest list.
use super::{CopyLocation, List, TransitionBarrier, parent, recording};
use crate::device::{depth::Pair, resource_proxy};
use core::{ffi::c_void, ptr};
use windows::Win32::Foundation::{E_INVALIDARG, RECT};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

pub(super) unsafe extern "system" fn clear(
    list: &List,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    flags: D3D12_CLEAR_FLAGS,
    depth: f32,
    stencil: u8,
    count: u32,
    rects: *const RECT,
) {
    let mut recording = recording(list);
    if !recording.can_record(&list.native) {
        return;
    }
    if handle.ptr == 0
        || flags.0 == 0
        || flags.0 & !(D3D12_CLEAR_FLAG_DEPTH | D3D12_CLEAR_FLAG_STENCIL).0 != 0
        || (count != 0 && rects.is_null())
    {
        recording.error = E_INVALIDARG;
        return;
    }
    unsafe {
        (list.native.vtable().ClearDepthStencilView)(
            list.native.as_raw(),
            handle,
            flags,
            depth,
            stencil,
            count,
            rects,
        )
    };
}

pub(super) unsafe extern "system" fn clear_uav<const FLOAT: bool>(
    list: &List,
    gpu: D3D12_GPU_DESCRIPTOR_HANDLE,
    cpu: D3D12_CPU_DESCRIPTOR_HANDLE,
    resource: *mut c_void,
    values: *const u32,
    count: u32,
    rects: *const RECT,
) {
    let mut recording = recording(list);
    if !recording.can_record(&list.native) {
        return;
    }
    let Some(resource) = (unsafe { resource_proxy(resource, parent(&list.parent)) }) else {
        recording.error = E_INVALIDARG;
        return;
    };
    if gpu.ptr == 0 || cpu.ptr == 0 || values.is_null() || (count != 0 && rects.is_null()) {
        recording.error = E_INVALIDARG;
        return;
    }
    if resource.state.unbacked_uav() {
        // Its UAV descriptor is null on the host; the clear has nothing to write.
        static SKIPPED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&SKIPPED, || "skipped UAV clear of a Scarlett depth image".to_owned());
        return;
    }
    let (gpu, cpu) = if let Some(arena) = &recording.descriptor_arena
        && arena.contains(gpu.ptr)
    {
        let handle = match arena.gpu(gpu.ptr, 1) {
            Ok(handle) => handle,
            Err(error) => {
                recording.error = error.code();
                return;
            }
        };
        (
            handle,
            if arena.contains(cpu.ptr as u64) {
                arena.cpu(cpu.ptr)
            } else {
                cpu
            },
        )
    } else {
        (gpu, cpu)
    };
    if let Err(error) = recording.prepare_copy(resource.state.placed(), None) {
        recording.error = error;
        return;
    }
    let alias = match unsafe { crate::device::clear_buffer_alias(parent(&list.parent), cpu.ptr, resource) } {
        Ok(alias) => alias,
        Err(error) => { recording.error = error.code(); return; }
    };
    if let Some(target) = &alias
        && let Err(error) = super::raytracing::retain(&recording, &[target])
    {
        recording.error = error.code();
        return;
    }
    let target = alias.as_ref().unwrap_or_else(|| resource.state.depth()
        .map_or(&resource.native, |pair| &pair.image)).as_raw();
    let native = &list.native;
    unsafe {
        if FLOAT {
            (native.vtable().ClearUnorderedAccessViewFloat)(
                native.as_raw(),
                gpu,
                cpu,
                target,
                values.cast(),
                count,
                rects,
            )
        } else {
            (native.vtable().ClearUnorderedAccessViewUint)(
                native.as_raw(),
                gpu,
                cpu,
                target,
                values,
                count,
                rects,
            )
        }
    }
}

fn transition(
    list: &ID3D12GraphicsCommandList,
    resource: &ID3D12Resource,
    subresource: u32,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    if before != after {
        let host = TransitionBarrier {
            kind: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            resource: resource.as_raw(),
            subresource,
            before,
            after,
        };
        unsafe { (list.vtable().ResourceBarrier)(list.as_raw(), 1, ptr::from_ref(&host).cast()) };
    }
}

fn copy_plane(
    list: &ID3D12GraphicsCommandList,
    pair: &Pair,
    image: &ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    to_buffer: bool,
) {
    let texture = CopyLocation {
        resource: image.as_raw(),
        kind: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        data: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let buffer = CopyLocation {
        resource: pair.transfer.as_raw(),
        kind: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        data: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let (dst, src) = if to_buffer {
        (&buffer, &texture)
    } else {
        (&texture, &buffer)
    };
    unsafe {
        (list.vtable().CopyTextureRegion)(
            list.as_raw(),
            ptr::from_ref(dst).cast(),
            0,
            0,
            0,
            ptr::from_ref(src).cast(),
            ptr::null(),
        )
    };
}

pub(super) unsafe fn handoff(
    list: &ID3D12GraphicsCommandList,
    depth: &ID3D12Resource,
    pair: &Pair,
    barrier: &TransitionBarrier,
) {
    let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    let src = D3D12_RESOURCE_STATE_COPY_SOURCE;
    let dst = D3D12_RESOURCE_STATE_COPY_DEST;
    let entering = barrier.after == uav;
    if barrier.subresource != 0 {
        // There is no unordered stencil view. Park that hardware plane in
        // COMMON until the logical resource leaves unordered access.
        let common = D3D12_RESOURCE_STATE_COMMON;
        let (before, after) = if entering {
            (barrier.before, common)
        } else {
            (common, barrier.after)
        };
        transition(list, depth, 1, before, after);
        if barrier.subresource == 1 {
            return;
        }
    }
    if entering {
        transition(list, depth, 0, barrier.before, src);
        transition(list, &pair.image, 0, uav, dst);
        copy_plane(list, pair, depth, pair.depth_plane, true);
    } else {
        transition(list, &pair.image, 0, uav, src);
        transition(list, depth, 0, src, dst);
        copy_plane(list, pair, &pair.image, pair.image_plane, true);
    }
    transition(list, &pair.transfer, 0, dst, src);
    if entering {
        copy_plane(list, pair, &pair.image, pair.image_plane, false);
        transition(list, &pair.image, 0, dst, uav);
    } else {
        copy_plane(list, pair, depth, pair.depth_plane, false);
        transition(list, &pair.image, 0, src, uav);
        transition(list, depth, 0, dst, barrier.after);
    }
    transition(list, &pair.transfer, 0, src, dst);
}

// The primary uses the guest's enhanced layouts. The private float image and
// transfer buffer stay under the legacy barrier API for their whole lifetime.
#[derive(Clone, Copy)]
struct Scope(
    D3D12_BARRIER_SYNC,
    D3D12_BARRIER_ACCESS,
    D3D12_BARRIER_LAYOUT,
);

fn texture_plane(
    api: &ID3D12GraphicsCommandList7,
    res: &ID3D12Resource,
    plane: u32,
    before: Scope,
    after: Scope,
    flags: D3D12_TEXTURE_BARRIER_FLAGS,
) {
    let barrier = D3D12_TEXTURE_BARRIER {
        SyncBefore: before.0,
        SyncAfter: after.0,
        AccessBefore: before.1,
        AccessAfter: after.1,
        LayoutBefore: before.2,
        LayoutAfter: after.2,
        pResource: core::mem::ManuallyDrop::new(None),
        Subresources: D3D12_BARRIER_SUBRESOURCE_RANGE {
            IndexOrFirstMipLevel: plane,
            ..Default::default()
        },
        Flags: flags,
    };
    let mut barrier = barrier;
    unsafe {
        ptr::from_mut(&mut barrier.pResource)
            .cast::<*mut c_void>()
            .write(res.as_raw());
        api.Barrier(&[D3D12_BARRIER_GROUP {
            Type: D3D12_BARRIER_TYPE_TEXTURE,
            NumBarriers: 1,
            Anonymous: D3D12_BARRIER_GROUP_0 {
                pTextureBarriers: &barrier,
            },
        }]);
    }
}

fn unordered(layout: D3D12_BARRIER_LAYOUT) -> bool {
    matches!(
        layout,
        D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_UNORDERED_ACCESS
            | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_UNORDERED_ACCESS
    )
}

fn selected_planes(range: &D3D12_BARRIER_SUBRESOURCE_RANGE) -> windows::core::Result<(bool, bool)> {
    let (first, count) = match (range.NumMipLevels, range.IndexOrFirstMipLevel) {
        (0, u32::MAX) => (0, 2),
        (0, index @ 0..=1) => (index, 1),
        (1, 0) if range.FirstArraySlice == 0 && range.NumArraySlices == 1 => {
            (range.FirstPlane, range.NumPlanes)
        }
        _ => return Err(E_INVALIDARG.into()),
    };
    if count == 0 || first.checked_add(count).is_none_or(|end| end > 2) {
        return Err(E_INVALIDARG.into());
    }
    Ok((first == 0, first + count == 2))
}

pub(super) fn enhanced_handoff(
    api: &ID3D12GraphicsCommandList7,
    depth: &ID3D12Resource,
    pair: &Pair,
    b: &D3D12_TEXTURE_BARRIER,
) -> windows::core::Result<bool> {
    use windows::Win32::Foundation::E_NOTIMPL;
    let before = Scope(b.SyncBefore, b.AccessBefore, b.LayoutBefore);
    let after = Scope(b.SyncAfter, b.AccessAfter, b.LayoutAfter);
    let from_uav = unordered(before.2);
    let to_uav = unordered(after.2);
    if (!from_uav && before.1.contains(D3D12_BARRIER_ACCESS_UNORDERED_ACCESS))
        || (!to_uav && after.1.contains(D3D12_BARRIER_ACCESS_UNORDERED_ACCESS))
    {
        // A queue-common layout allowing both UAV and SRV accesses cannot keep
        // the two independent surfaces coherent within a single dispatch.
        return Err(E_NOTIMPL.into());
    }
    if !from_uav && !to_uav {
        return Ok(false);
    }
    if before.0 == D3D12_BARRIER_SYNC_SPLIT
        || after.0 == D3D12_BARRIER_SYNC_SPLIT
        || (b.Flags != D3D12_TEXTURE_BARRIER_FLAG_NONE
            && b.Flags != D3D12_TEXTURE_BARRIER_FLAG_DISCARD)
    {
        return Err(E_NOTIMPL.into());
    }
    let (float, stencil) = selected_planes(&b.Subresources)?;
    let none = D3D12_TEXTURE_BARRIER_FLAG_NONE;
    let common = Scope(
        D3D12_BARRIER_SYNC_ALL,
        D3D12_BARRIER_ACCESS_COMMON,
        D3D12_BARRIER_LAYOUT_COMMON,
    );
    let source = Scope(
        D3D12_BARRIER_SYNC_COPY,
        D3D12_BARRIER_ACCESS_COPY_SOURCE,
        D3D12_BARRIER_LAYOUT_COPY_SOURCE,
    );
    let dest = Scope(
        D3D12_BARRIER_SYNC_COPY,
        D3D12_BARRIER_ACCESS_COPY_DEST,
        D3D12_BARRIER_LAYOUT_COPY_DEST,
    );
    // The guest's sync scopes may name work unrelated to the two private
    // resources. Keep the execution dependency across the injected copies.
    let sync = D3D12_GLOBAL_BARRIER {
        SyncBefore: D3D12_BARRIER_SYNC_ALL,
        SyncAfter: D3D12_BARRIER_SYNC_ALL,
        AccessBefore: D3D12_BARRIER_ACCESS_COMMON,
        AccessAfter: D3D12_BARRIER_ACCESS_COMMON,
    };
    let global = D3D12_BARRIER_GROUP {
        Type: D3D12_BARRIER_TYPE_GLOBAL,
        NumBarriers: 1,
        Anonymous: D3D12_BARRIER_GROUP_0 {
            pGlobalBarriers: &sync,
        },
    };
    unsafe {
        api.Barrier(&[global]);
    }
    if stencil && from_uav != to_uav {
        texture_plane(
            api,
            depth,
            1,
            if from_uav { common } else { before },
            if to_uav { common } else { after },
            b.Flags,
        );
    }
    if float && from_uav != to_uav {
        let list: &ID3D12GraphicsCommandList = api;
        let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        let src = D3D12_RESOURCE_STATE_COPY_SOURCE;
        let dst = D3D12_RESOURCE_STATE_COPY_DEST;
        if b.Flags == D3D12_TEXTURE_BARRIER_FLAG_DISCARD
            || before.2 == D3D12_BARRIER_LAYOUT_UNDEFINED
            || after.2 == D3D12_BARRIER_LAYOUT_UNDEFINED
        {
            texture_plane(
                api,
                depth,
                0,
                if from_uav { source } else { before },
                if to_uav { source } else { after },
                b.Flags,
            );
        } else {
            let (read, read_plane, write, write_plane, image_copy_state) = if to_uav {
                texture_plane(api, depth, 0, before, source, none);
                (depth, pair.depth_plane, &pair.image, pair.image_plane, dst)
            } else {
                texture_plane(api, depth, 0, source, dest, none);
                (&pair.image, pair.image_plane, depth, pair.depth_plane, src)
            };
            transition(list, &pair.image, 0, uav, image_copy_state);
            copy_plane(list, pair, read, read_plane, true);
            transition(list, &pair.transfer, 0, dst, src);
            copy_plane(list, pair, write, write_plane, false);
            transition(list, &pair.transfer, 0, src, dst);
            transition(list, &pair.image, 0, image_copy_state, uav);
            if from_uav {
                texture_plane(api, depth, 0, dest, after, none);
            }
        }
    }
    unsafe {
        api.Barrier(&[global]);
    }
    Ok(true)
}
