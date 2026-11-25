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
    unsafe { crate::device::placement::touch_target(parent(&list.parent), handle.ptr, &mut recording.target_heaps) };
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
            for &s in &stencil {
                transition(list, depth, s, from, to);
            }
        }
        if pair.deep {
            // A 24-bit plane: only its states move here. The twin is not
            // given the plane on entry (fxceless's shortcut, counted); on
            // exit the plane is drawn from the twin when the list closes.
            for &p in &planes {
                transition(list, depth, p, from, to);
            }
            if entering {
                LATE[3].fetch_add(planes.len() as u64, Ordering::Relaxed);
            } else {
                let both: Vec<u32> = planes.iter().chain(&stencil).copied().collect();
                defer_plane(list, depth, pair, &both, to);
            }
            return;
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
    if pair.deep {
        // Not ported: the 24-bit plane's close-time pass runs on legacy
        // barriers only (`late_pass`); enhanced layouts are refused as any
        // other unsupported depth barrier is.
        crate::diagnostics::log_line(format_args!("enhanced barrier on a 24-bit depth twin: not supported"));
        return Err(E_NOTIMPL.into());
    }
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

// ---------------------------------------------------------------------------
// The 24-bit depth plane's pass (TUXBOX_DEPTH_TWINS=1, console D16S8 on a host
// D24S8 texture). Ported from VIctory-fxceless victory-gpu host.rs (GPL-3.0,
// credited): `record_plane_pass`, `PLANE_STEPS`, `TwinUse::late`,
// `twins_to_depth` and `settle`. No copy turns the twin's 16-bit steps into
// 24-bit depth, so when the title takes the texture out of unordered access
// the depth plane is left as it was and drawn from the twin when the list
// closes; a pass mid-list would replace the title's bound pipeline, root
// signature and targets. As in fxceless, the list's own uses between the
// barrier and its close see the plane as it was (counted in LATE[1]), and the
// twin is not given the plane when the texture enters unordered access
// (fxceless's shortcut: in the title only the twin's pass writes that plane).
// Per list, each such texture's subresource states are settled from the
// title's barriers so the pass moves them from where the title left them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, atomic::{AtomicU32, AtomicU64, Ordering}};

/// [passes drawn, uses that saw the plane before its pass, subresources
/// skipped with no known state, twin entries not given the plane].
pub(crate) static LATE: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

struct Late {
    depth: ID3D12Resource,
    image: ID3D12Resource,
    /// Host state per subresource of both planes, as the title left it in
    /// this list (`None`: not named since the texture left unordered access).
    states: Vec<Option<D3D12_RESOURCE_STATES>>,
}

/// Per host list (its raw pointer): the 24-bit planes its close must draw.
fn pending() -> std::sync::MutexGuard<'static, HashMap<usize, Vec<Late>>> {
    static PENDING: OnceLock<Mutex<HashMap<usize, Vec<Late>>>> = OnceLock::new();
    PENDING.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Leaves the deep pair's depth plane where the title asked, the twin
/// holding it: notes the pass for the list's close.
fn defer_plane(list: &ID3D12GraphicsCommandList, depth: &ID3D12Resource, pair: &Pair, subresources: &[u32], state: D3D12_RESOURCE_STATES) {
    let mut pending = pending();
    let lates = pending.entry(list.as_raw() as usize).or_default();
    let at = match lates.iter().position(|l| l.depth.as_raw() == depth.as_raw()) {
        Some(at) => at,
        None => {
            lates.push(Late { depth: depth.clone(), image: pair.image.clone(), states: vec![None; 2 * pair.count() as usize] });
            lates.len() - 1
        }
    };
    for &s in subresources {
        if let Some(slot) = lates[at].states.get_mut(s as usize) {
            *slot = Some(state);
        }
    }
}

/// commands.rs barrier hook: the title moves `depth`'s `subresource` to host
/// state `after` outside unordered access. A pending pass follows it
/// (fxceless `settle`) and counts the use that sees the plane early.
pub(super) fn note_state(list: &ID3D12GraphicsCommandList, depth: &ID3D12Resource, subresource: u32, after: D3D12_RESOURCE_STATES) {
    let mut pending = pending();
    let Some(late) = pending
        .get_mut(&(list.as_raw() as usize))
        .and_then(|l| l.iter_mut().find(|l| l.depth.as_raw() == depth.as_raw()))
    else {
        return;
    };
    LATE[1].fetch_add(1, Ordering::Relaxed);
    if subresource == D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES {
        late.states.iter_mut().for_each(|s| *s = Some(after));
    } else if let Some(slot) = late.states.get_mut(subresource as usize) {
        *slot = Some(after);
    }
}

/// commands.rs reset hook: a reset list drops what it had pending.
pub(super) fn forget(list: &ID3D12GraphicsCommandList) {
    if crate::device::depth::twins() {
        pending().remove(&(list.as_raw() as usize));
    }
}

const PLANE_SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0), visibility = SHADER_VISIBILITY_PIXEL)"
Texture2DArray<uint> twin : register(t0);
float4 corner(uint i : SV_VertexID) : SV_Position {
    return float4(float(i & 1) * 4.0 - 1.0, float(i >> 1) * 4.0 - 1.0, 0.0, 1.0);
}
// A 16-bit twin holds whole steps of 1/65535; the depth written is the step
// times the float nearest 1/65535 (0x37800080), as the title's tiled lighting
// computes the depth it then tests equal against (fxceless PLANE_STEPS).
[RootSignature(RS)]
float plane(float4 p : SV_Position) : SV_Depth {
    return float(twin.Load(int4(int2(p.xy), 0, 0))) * asfloat(0x37800080);
}
"#;

/// Views of twins (a shader-visible ring) and depth views (a CPU ring), as
/// fxceless PLANE_VIEWS: a view lasts until its list ran, and one or two
/// passes a frame are far from coming round.
const PLANE_VIEWS: u32 = 256;

struct PlanePass {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    targets: ID3D12DescriptorHeap,
    view_step: u32,
    target_step: u32,
    next: AtomicU32,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for PlanePass {}
// SAFETY: as above.
unsafe impl Sync for PlanePass {}

fn compile_plane(entry: windows::core::PCSTR, target: windows::core::PCSTR) -> windows::core::Result<Vec<u8>> {
    use windows::Win32::Foundation::{E_NOTIMPL, FreeLibrary};
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
    use windows::core::{HRESULT, s, w};
    type Compile = unsafe extern "system" fn(
        *const c_void, usize, *const u8, *const c_void, *const c_void,
        *const u8, *const u8, u32, u32, *mut *mut c_void, *mut *mut c_void,
    ) -> HRESULT;
    let library = unsafe { LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)? };
    let result = (|| unsafe {
        let entry_point = GetProcAddress(library, s!("D3DCompile")).ok_or(E_NOTIMPL)?;
        let compile: Compile = core::mem::transmute(entry_point);
        let mut raw = ptr::null_mut();
        compile(
            PLANE_SOURCE.as_ptr().cast(), PLANE_SOURCE.len(), s!("plane").as_ptr(), ptr::null(), ptr::null(),
            entry.as_ptr(), target.as_ptr(), 0, 0, &mut raw, ptr::null_mut(),
        )
        .ok()?;
        let blob = ID3DBlob::from_raw(raw);
        Ok(core::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()).to_vec())
    })();
    let _ = unsafe { FreeLibrary(library) };
    result
}

fn make_pass(device: &ID3D12Device) -> windows::core::Result<PlanePass> {
    use core::mem::ManuallyDrop;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_SAMPLE_DESC};
    use windows::core::s;
    let vs = compile_plane(s!("corner"), s!("vs_5_1"))?;
    let ps = compile_plane(s!("plane"), s!("ps_5_1"))?;
    let code = |bytes: &[u8]| D3D12_SHADER_BYTECODE { pShaderBytecode: bytes.as_ptr().cast(), BytecodeLength: bytes.len() };
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &ps)?;
        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: ManuallyDrop::new(Some(root.clone())),
            VS: code(&vs),
            PS: code(&ps),
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            },
            // Every texel is written; stencil stays off and untouched.
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                ..Default::default()
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            DSVFormat: DXGI_FORMAT_D24_UNORM_S8_UINT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let pipeline = device.CreateGraphicsPipelineState(&desc);
        ManuallyDrop::drop(&mut desc.pRootSignature);
        let heap = |kind, flags| {
            device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: kind,
                NumDescriptors: PLANE_VIEWS,
                Flags: flags,
                NodeMask: 0,
            })
        };
        Ok(PlanePass {
            root,
            pipeline: pipeline?,
            views: heap(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE)?,
            targets: heap(D3D12_DESCRIPTOR_HEAP_TYPE_DSV, D3D12_DESCRIPTOR_HEAP_FLAG_NONE)?,
            view_step: device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            target_step: device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_DSV),
            next: AtomicU32::new(0),
        })
    }
}

fn plane_pass(device: &ID3D12Device) -> Option<&'static PlanePass> {
    static PASS: OnceLock<Option<PlanePass>> = OnceLock::new();
    PASS.get_or_init(|| match make_pass(device) {
        Ok(pass) => Some(pass),
        Err(error) => {
            crate::diagnostics::log_line(format_args!(
                "no pass to draw a 24-bit depth plane from its twin ({error}): such planes stay as they are"
            ));
            None
        }
    })
    .as_ref()
}

/// commands.rs close hook, before the host list closes: draws each pending
/// 24-bit depth plane from its twin, each mip and slice, and puts every
/// subresource back in the state the title left it (fxceless
/// `twins_to_depth`). Nothing of the title's is recorded after it, so the
/// pass leaves its own heap, pipeline and target bound.
pub(super) fn late_pass(list: &ID3D12GraphicsCommandList) {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_FORMAT_R16_UINT};
    if !crate::device::depth::twins() {
        return;
    }
    let Some(lates) = pending().remove(&(list.as_raw() as usize)) else { return };
    let mut device: Option<ID3D12Device> = None;
    if unsafe { list.GetDevice(&mut device) }.is_err() {
        return;
    }
    let Some(device) = device else { return };
    let Some(pass) = plane_pass(&device) else { return };
    let write = D3D12_RESOURCE_STATE_DEPTH_WRITE;
    let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
    let read = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
    for late in &lates {
        let of = unsafe { late.depth.GetDesc() };
        let mips = u32::from(of.MipLevels.max(1));
        let count = mips * u32::from(of.DepthOrArraySize.max(1));
        for i in 0..count {
            let (Some(Some(depth_state)), Some(Some(stencil_state))) =
                (late.states.get(i as usize).copied(), late.states.get((count + i) as usize).copied())
            else {
                LATE[2].fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let (mip, slice) = (i % mips, i / mips);
            let slot = pass.next.fetch_add(1, Ordering::Relaxed) % PLANE_VIEWS;
            let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R16_UINT,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2DARRAY,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2DArray: D3D12_TEX2D_ARRAY_SRV {
                        MostDetailedMip: mip,
                        MipLevels: 1,
                        FirstArraySlice: slice,
                        ArraySize: 1,
                        ..Default::default()
                    },
                },
            };
            let target = D3D12_DEPTH_STENCIL_VIEW_DESC {
                Format: DXGI_FORMAT_D24_UNORM_S8_UINT,
                ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2DARRAY,
                Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 {
                    Texture2DArray: D3D12_TEX2D_ARRAY_DSV { MipSlice: mip, FirstArraySlice: slice, ArraySize: 1 },
                },
                ..Default::default()
            };
            let (width, height) = ((of.Width >> mip).max(1) as u32, (of.Height >> mip).max(1));
            unsafe {
                let cpu = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: pass.views.GetCPUDescriptorHandleForHeapStart().ptr + (slot * pass.view_step) as usize,
                };
                let gpu = D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: pass.views.GetGPUDescriptorHandleForHeapStart().ptr + u64::from(slot * pass.view_step),
                };
                let dsv = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: pass.targets.GetCPUDescriptorHandleForHeapStart().ptr + (slot * pass.target_step) as usize,
                };
                device.CreateShaderResourceView(&late.image, Some(&view), cpu);
                device.CreateDepthStencilView(&late.depth, Some(&target), dsv);
                transition(list, &late.depth, i, depth_state, write);
                transition(list, &late.depth, count + i, stencil_state, write);
                transition(list, &late.image, i, uav, read);
                list.SetDescriptorHeaps(&[Some(pass.views.clone())]);
                list.SetGraphicsRootSignature(&pass.root);
                list.SetPipelineState(&pass.pipeline);
                list.SetGraphicsRootDescriptorTable(0, gpu);
                list.OMSetRenderTargets(0, None, false, Some(&dsv));
                list.RSSetViewports(&[D3D12_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: width as f32,
                    Height: height as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                }]);
                list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 }]);
                list.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY(4));
                list.DrawInstanced(3, 1, 0, 0);
                transition(list, &late.image, i, read, uav);
                transition(list, &late.depth, i, write, depth_state);
                transition(list, &late.depth, count + i, write, stencil_state);
            }
            LATE[0].fetch_add(1, Ordering::Relaxed);
        }
    }
}
