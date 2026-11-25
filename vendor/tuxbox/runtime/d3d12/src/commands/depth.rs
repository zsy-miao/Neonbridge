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
    crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::ClearsDepth);
    let mut recording = recording(list);
    if !recording.can_record_graphics(&list.native) {
        return;
    }
    if handle.ptr == 0
        || flags.0 == 0
        || flags.0 & !(D3D12_CLEAR_FLAG_DEPTH | D3D12_CLEAR_FLAG_STENCIL).0 != 0
        || (count != 0 && rects.is_null())
    {
        { recording.error = E_INVALIDARG; recording.error_at = (file!(), line!()); }
        return;
    }
    unsafe { crate::device::placement::touch_target(parent(&list.parent), handle.ptr) };
    let depth = if flags.0 & D3D12_CLEAR_FLAG_DEPTH.0 != 0 { clear_depth(depth) } else { depth };
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

/// leonida 673dd15d (GPL-3.0, credit to sodove) clamps a depth clear into
/// [0, 1] because its Vulkan backend has no VK_EXT_depth_range_unrestricted;
/// D3D12 takes the title's value as is, so the clamp is off by default.
/// TUXBOX_DEPTH_CLEAR_CLAMP=1 clamps (NaN to 0) as the earlier misread port did.
fn clear_depth(depth: f32) -> f32 {
    if (0.0..=1.0).contains(&depth) || crate::env_toggle!("TUXBOX_DEPTH_CLEAR_CLAMP") != Some("1") {
        return depth;
    }
    let host = if depth.is_nan() { 0.0 } else { depth.clamp(0.0, 1.0) };
    static ONCE: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&ONCE, || format!(
        "depth clear {depth} is outside [0, 1]: cleared to {host} (logged once; TUXBOX_DEPTH_CLEAR_CLAMP=1)"));
    host
}

/// The host descriptor behind a UAV clear's CPU handle;
/// TUXBOX_UAV_CLEAR_XLATE=0 passes the title handle through unchanged.
fn uav_clear_cpu(list: &List, cpu: D3D12_CPU_DESCRIPTOR_HANDLE) -> D3D12_CPU_DESCRIPTOR_HANDLE {
    if crate::env_toggle!("TUXBOX_UAV_CLEAR_XLATE") == Some("0") {
        return cpu;
    }
    unsafe { crate::device::host_cpu_descriptor(parent(&list.parent), cpu) }
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
    crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::ClearsUav);
    let mut recording = recording(list);
    if !recording.can_record(&list.native) {
        return;
    }
    let Some(resource) = (unsafe { resource_proxy(resource, parent(&list.parent)) }) else {
        { recording.error = E_INVALIDARG; recording.error_at = (file!(), line!()); }
        return;
    };
    if gpu.ptr == 0 || cpu.ptr == 0 || values.is_null() || (count != 0 && rects.is_null()) {
        { recording.error = E_INVALIDARG; recording.error_at = (file!(), line!()); }
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
                { recording.error = error.code(); recording.error_at = (file!(), line!()); }
                return;
            }
        };
        (
            handle,
            if arena.contains(cpu.ptr as u64) {
                arena.cpu(cpu.ptr)
            } else {
                uav_clear_cpu(list, cpu)
            },
        )
    } else {
        (gpu, uav_clear_cpu(list, cpu))
    };
    if let Err(error) = recording.prepare_copy(resource.state.placed(), None) {
        { recording.error = error; recording.error_at = (file!(), line!()); }
        return;
    }
    let alias = match unsafe { crate::device::clear_buffer_alias(parent(&list.parent), cpu.ptr, resource) } {
        Ok(alias) => alias,
        Err(error) => { { recording.error = error.code(); recording.error_at = (file!(), line!()); } return; }
    };
    if let Some(target) = &alias
        && let Err(error) = super::raytracing::retain(&recording, &[target])
    {
        { recording.error = error.code(); recording.error_at = (file!(), line!()); }
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
    subresource: u32,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    to_buffer: bool,
) {
    let texture = CopyLocation {
        resource: image.as_raw(),
        kind: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        data: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: subresource,
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

/// The depth-plane subresources a guest barrier's subresource names, and the
/// stencil ones: ALL names every one of both planes.
fn named(pair: &Pair, subresource: u32) -> (Vec<u32>, Vec<u32>) {
    let n = pair.count();
    let stencil_end = if pair.stencil { 2 * n } else { n };
    if subresource == D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES {
        ((0..n).collect(), (n..stencil_end).collect())
    } else if subresource < n {
        (vec![subresource], Vec::new())
    } else if subresource < stencil_end {
        (Vec::new(), vec![subresource])
    } else {
        (Vec::new(), Vec::new())
    }
}

/// Copies the named depth-plane subresources between the depth texture and
/// its twin through the transfer buffer, both sides moved to their copy
/// states and back. `entering`: depth texture into the twin.
/// The twin, its layouts and its first depth-texture subresource for a
/// depth-texture subresource: the depth plane's, or the stencil plane's.
fn twin_of(pair: &Pair, subresource: u32) -> Option<(&ID3D12Resource, u32, (D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_PLACED_SUBRESOURCE_FOOTPRINT))> {
    let n = pair.count();
    if subresource < n {
        Some((&pair.image, subresource, pair.planes[subresource as usize]))
    } else {
        let image = pair.stencil_image.as_ref()?;
        Some((image, subresource - n, *pair.stencil_planes.get((subresource - n) as usize)?))
    }
}

fn copy_subresources(
    list: &ID3D12GraphicsCommandList,
    depth: &ID3D12Resource,
    pair: &Pair,
    subresources: &[u32],
    entering: bool,
    depth_from: D3D12_RESOURCE_STATES,
    depth_to: D3D12_RESOURCE_STATES,
) {
    let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    let src = D3D12_RESOURCE_STATE_COPY_SOURCE;
    let dst = D3D12_RESOURCE_STATE_COPY_DEST;
    for &i in subresources {
        let Some((twin, t, (depth_fp, image_fp))) = twin_of(pair, i) else { continue };
        if entering {
            transition(list, depth, i, depth_from, src);
            transition(list, twin, t, uav, dst);
            copy_plane(list, pair, depth, i, depth_fp, true);
        } else {
            transition(list, twin, t, uav, src);
            transition(list, depth, i, depth_from, dst);
            copy_plane(list, pair, twin, t, image_fp, true);
        }
        transition(list, &pair.transfer, 0, dst, src);
        if entering {
            copy_plane(list, pair, twin, t, image_fp, false);
            transition(list, twin, t, dst, uav);
            transition(list, depth, i, src, depth_to);
        } else {
            copy_plane(list, pair, depth, i, depth_fp, false);
            transition(list, twin, t, src, uav);
            transition(list, depth, i, dst, depth_to);
        }
        transition(list, &pair.transfer, 0, src, dst);
    }
}

pub(super) unsafe fn handoff(
    list: &ID3D12GraphicsCommandList,
    depth: &ID3D12Resource,
    pair: &Pair,
    barrier: &TransitionBarrier,
) {
    let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    let src = D3D12_RESOURCE_STATE_COPY_SOURCE;
    let entering = barrier.after == uav;
    if crate::device::depth::twins() {
        let _ = src;
        let (planes, stencil) = named(pair, barrier.subresource);
        // There is no unordered stencil view: park those subresources in
        // COMMON while the logical resource is in unordered access.
        // Both planes rest in COMMON while their twins hold them, so a
        // texture made in unordered access starts in one state for both.
        let rest = D3D12_RESOURCE_STATE_COMMON;
        let (from, to) = if entering { (barrier.before, rest) } else { (rest, barrier.after) };
        if pair.stencil_image.is_some() {
            copy_subresources(list, depth, pair, &stencil, entering, from, to);
        } else {
            for s in stencil {
                transition(list, depth, s, from, to);
            }
        }
        copy_subresources(list, depth, pair, &planes, entering, from, to);
        return;
    }
    let dst = D3D12_RESOURCE_STATE_COPY_DEST;
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
    let (depth_plane, image_plane) = pair.planes[0];
    if entering {
        transition(list, depth, 0, barrier.before, src);
        transition(list, &pair.image, 0, uav, dst);
        copy_plane(list, pair, depth, 0, depth_plane, true);
    } else {
        transition(list, &pair.image, 0, uav, src);
        transition(list, depth, 0, src, dst);
        copy_plane(list, pair, &pair.image, 0, image_plane, true);
    }
    transition(list, &pair.transfer, 0, dst, src);
    if entering {
        copy_plane(list, pair, &pair.image, 0, image_plane, false);
        transition(list, &pair.image, 0, dst, uav);
    } else {
        copy_plane(list, pair, depth, 0, depth_plane, false);
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

/// TUXBOX_DEPTH_TWINS: the depth-plane and stencil-plane subresource
/// indices an enhanced barrier's range names, for any mips and slices.
fn selected_subresources(pair: &Pair, range: &D3D12_BARRIER_SUBRESOURCE_RANGE) -> windows::core::Result<(Vec<u32>, Vec<u32>)> {
    let mips = u32::from(pair.guest.MipLevels.max(1));
    let slices = u32::from(pair.guest.DepthOrArraySize.max(1));
    let n = mips * slices;
    let planes = if pair.stencil { 2 } else { 1 };
    let mut out = (Vec::new(), Vec::new());
    let mut push = |index: u32| {
        if index < n {
            out.0.push(index);
        } else if index < n * planes {
            out.1.push(index);
        }
    };
    if range.NumMipLevels == 0 {
        if range.IndexOrFirstMipLevel == u32::MAX {
            (0..n * planes).for_each(&mut push);
        } else if range.IndexOrFirstMipLevel < n * planes {
            push(range.IndexOrFirstMipLevel);
        } else {
            return Err(E_INVALIDARG.into());
        }
    } else {
        let (m0, mc) = (range.IndexOrFirstMipLevel, range.NumMipLevels);
        let (s0, sc) = (range.FirstArraySlice, range.NumArraySlices.max(1));
        let (p0, pc) = (range.FirstPlane, range.NumPlanes.max(1));
        if m0.checked_add(mc).is_none_or(|e| e > mips)
            || s0.checked_add(sc).is_none_or(|e| e > slices)
            || p0.checked_add(pc).is_none_or(|e| e > planes)
        {
            return Err(E_INVALIDARG.into());
        }
        for p in p0..p0 + pc {
            for sl in s0..s0 + sc {
                for m in m0..m0 + mc {
                    push(m + sl * mips + p * n);
                }
            }
        }
    }
    Ok(out)
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
    let twins = crate::device::depth::twins();
    let (float_list, stencil_list) = if twins { selected_subresources(pair, &b.Subresources)? } else { (vec![0], vec![1]) };
    let (float, stencil) = if twins {
        (!float_list.is_empty(), !stencil_list.is_empty())
    } else {
        selected_planes(&b.Subresources)?
    };
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
    let (float_list, stencil_list) = if twins && pair.stencil_image.is_some() {
        // The stencil plane has its own twin: its subresources are copied
        // with the depth plane's.
        let mut all = float_list;
        all.extend(stencil_list);
        (all, Vec::new())
    } else {
        (float_list, stencil_list)
    };
    let (float, stencil) = (float || (twins && !float_list.is_empty()), stencil && !stencil_list.is_empty());
    if stencil && from_uav != to_uav {
        for &s in &stencil_list {
            texture_plane(
                api,
                depth,
                s,
                if from_uav { common } else { before },
                if to_uav { common } else { after },
                b.Flags,
            );
        }
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
            let rest = if twins { common } else { source };
            for &i in &float_list {
                texture_plane(
                    api,
                    depth,
                    i,
                    if from_uav { rest } else { before },
                    if to_uav { rest } else { after },
                    b.Flags,
                );
            }
        } else if twins {
            // The depth plane rests in COMMON while the twin holds it.
            for &i in &float_list {
                let Some((twin, t, (depth_fp, image_fp))) = twin_of(pair, i) else { continue };
                let ((read, read_sub, read_fp), (write, write_sub, write_fp), image_copy_state) = if to_uav {
                    texture_plane(api, depth, i, before, source, none);
                    ((depth, i, depth_fp), (twin, t, image_fp), dst)
                } else {
                    texture_plane(api, depth, i, common, dest, none);
                    ((twin, t, image_fp), (depth, i, depth_fp), src)
                };
                transition(list, twin, t, uav, image_copy_state);
                copy_plane(list, pair, read, read_sub, read_fp, true);
                transition(list, &pair.transfer, 0, dst, src);
                copy_plane(list, pair, write, write_sub, write_fp, false);
                transition(list, &pair.transfer, 0, src, dst);
                transition(list, twin, t, image_copy_state, uav);
                if from_uav {
                    texture_plane(api, depth, i, dest, after, none);
                } else {
                    texture_plane(api, depth, i, source, common, none);
                }
            }
        } else {
            let (read, read_plane, write, write_plane, image_copy_state) = if to_uav {
                texture_plane(api, depth, 0, before, source, none);
                (depth, pair.planes[0].0, &pair.image, pair.planes[0].1, dst)
            } else {
                texture_plane(api, depth, 0, source, dest, none);
                (&pair.image, pair.planes[0].1, depth, pair.planes[0].0, src)
            };
            transition(list, &pair.image, 0, uav, image_copy_state);
            copy_plane(list, pair, read, 0, read_plane, true);
            transition(list, &pair.transfer, 0, dst, src);
            copy_plane(list, pair, write, 0, write_plane, false);
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
