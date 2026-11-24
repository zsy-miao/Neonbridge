//! The Series barrier groups have the desktop layout with guest COM resource pointers.
use super::*;

fn guest_pointer(
    field: &mem::ManuallyDrop<Option<ID3D12Resource>>,
) -> Result<*mut c_void, HRESULT> {
    field.as_ref().map(Interface::as_raw).ok_or(E_INVALIDARG)
}

fn borrow_resource(field: &mut mem::ManuallyDrop<Option<ID3D12Resource>>, native: &ID3D12Resource) {
    // Desktop D3D12 borrows the COM pointer for the duration of the call.
    unsafe {
        ptr::from_mut(field)
            .cast::<*mut c_void>()
            .write(native.as_raw())
    };
}

// Non-video stages and accesses: no graphics or compute list takes the video bits.
const KNOWN_SYNC: i32 = 0xffff | 0x0080_0000 | 0x0100_0000 | D3D12_BARRIER_SYNC_SPLIT.0;
const KNOWN_ACCESS: i32 = 0x0001_ffff | D3D12_BARRIER_ACCESS_NO_ACCESS.0;
const GRAPHICS_SYNC: i32 = D3D12_BARRIER_SYNC_DRAW.0 | D3D12_BARRIER_SYNC_INDEX_INPUT.0
    | D3D12_BARRIER_SYNC_VERTEX_SHADING.0 | D3D12_BARRIER_SYNC_PIXEL_SHADING.0
    | D3D12_BARRIER_SYNC_DEPTH_STENCIL.0 | D3D12_BARRIER_SYNC_RENDER_TARGET.0
    | D3D12_BARRIER_SYNC_RESOLVE.0;
const GRAPHICS_ACCESS: i32 = D3D12_BARRIER_ACCESS_VERTEX_BUFFER.0 | D3D12_BARRIER_ACCESS_INDEX_BUFFER.0
    | D3D12_BARRIER_ACCESS_RENDER_TARGET.0 | D3D12_BARRIER_ACCESS_DEPTH_STENCIL_WRITE.0
    | D3D12_BARRIER_ACCESS_DEPTH_STENCIL_READ.0 | D3D12_BARRIER_ACCESS_STREAM_OUTPUT.0
    | D3D12_BARRIER_ACCESS_RESOLVE_DEST.0 | D3D12_BARRIER_ACCESS_RESOLVE_SOURCE.0
    | D3D12_BARRIER_ACCESS_SHADING_RATE_SOURCE.0;

/// Keep one half of a barrier inside what the core runtime accepts on this
/// list type: known bits only, compute lists without graphics stages or
/// accesses, and NONE sync paired with NO_ACCESS. An invalid barrier made
/// the host Close fail with E_INVALIDARG.
/// TUXBOX_ENHANCED_FIX=0 records enhanced barriers as the title wrote them.
fn enhanced_fix() -> bool {
    crate::env_toggle!("TUXBOX_ENHANCED_FIX") != Some("0")
}

fn fix_half(sync: &mut D3D12_BARRIER_SYNC, access: &mut D3D12_BARRIER_ACCESS, compute: bool) {
    if !enhanced_fix() { return; }
    let mut s = sync.0 & KNOWN_SYNC;
    let mut a = access.0 & KNOWN_ACCESS;
    if a & D3D12_BARRIER_ACCESS_NO_ACCESS.0 != 0 {
        a = D3D12_BARRIER_ACCESS_NO_ACCESS.0;
    }
    if compute {
        if s & GRAPHICS_SYNC != 0 {
            s = s & !GRAPHICS_SYNC | D3D12_BARRIER_SYNC_ALL.0;
        }
        if a != D3D12_BARRIER_ACCESS_NO_ACCESS.0 {
            a &= !GRAPHICS_ACCESS;
        }
    }
    if s & !D3D12_BARRIER_SYNC_SPLIT.0 == 0 && s != D3D12_BARRIER_SYNC_SPLIT.0 {
        a = D3D12_BARRIER_ACCESS_NO_ACCESS.0;
    }
    // Every access needs a sync stage that can perform it (e.g. RENDER_TARGET
    // access with only PIXEL_SHADING sync is rejected); widen to ALL if not.
    if a != D3D12_BARRIER_ACCESS_NO_ACCESS.0 && s & D3D12_BARRIER_SYNC_ALL.0 == 0 && !sync_covers(s, a) {
        s = (s & D3D12_BARRIER_SYNC_SPLIT.0) | D3D12_BARRIER_SYNC_ALL.0;
    }
    *sync = D3D12_BARRIER_SYNC(s);
    *access = D3D12_BARRIER_ACCESS(a);
}

/// Whether `sync` holds, for each access bit, a stage compatible with it
/// (D3D12 enhanced barrier spec, access/sync compatibility).
fn sync_covers(sync: i32, access: i32) -> bool {
    let shading = D3D12_BARRIER_SYNC_DRAW.0 | D3D12_BARRIER_SYNC_ALL_SHADING.0 | D3D12_BARRIER_SYNC_VERTEX_SHADING.0
        | D3D12_BARRIER_SYNC_PIXEL_SHADING.0 | D3D12_BARRIER_SYNC_COMPUTE_SHADING.0 | D3D12_BARRIER_SYNC_RAYTRACING.0
        | D3D12_BARRIER_SYNC_NON_PIXEL_SHADING.0;
    let table: [(D3D12_BARRIER_ACCESS, i32); 17] = [
        (D3D12_BARRIER_ACCESS_VERTEX_BUFFER, D3D12_BARRIER_SYNC_VERTEX_SHADING.0 | D3D12_BARRIER_SYNC_INDEX_INPUT.0 | D3D12_BARRIER_SYNC_DRAW.0 | D3D12_BARRIER_SYNC_ALL_SHADING.0 | D3D12_BARRIER_SYNC_NON_PIXEL_SHADING.0),
        (D3D12_BARRIER_ACCESS_CONSTANT_BUFFER, shading),
        (D3D12_BARRIER_ACCESS_INDEX_BUFFER, D3D12_BARRIER_SYNC_INDEX_INPUT.0 | D3D12_BARRIER_SYNC_DRAW.0),
        (D3D12_BARRIER_ACCESS_RENDER_TARGET, D3D12_BARRIER_SYNC_RENDER_TARGET.0 | D3D12_BARRIER_SYNC_DRAW.0),
        (D3D12_BARRIER_ACCESS_UNORDERED_ACCESS, shading | D3D12_BARRIER_SYNC_CLEAR_UNORDERED_ACCESS_VIEW.0),
        (D3D12_BARRIER_ACCESS_DEPTH_STENCIL_WRITE, D3D12_BARRIER_SYNC_DEPTH_STENCIL.0 | D3D12_BARRIER_SYNC_DRAW.0),
        (D3D12_BARRIER_ACCESS_DEPTH_STENCIL_READ, D3D12_BARRIER_SYNC_DEPTH_STENCIL.0 | D3D12_BARRIER_SYNC_DRAW.0),
        (D3D12_BARRIER_ACCESS_SHADER_RESOURCE, shading),
        (D3D12_BARRIER_ACCESS_STREAM_OUTPUT, D3D12_BARRIER_SYNC_VERTEX_SHADING.0 | D3D12_BARRIER_SYNC_DRAW.0 | D3D12_BARRIER_SYNC_ALL_SHADING.0 | D3D12_BARRIER_SYNC_NON_PIXEL_SHADING.0),
        (D3D12_BARRIER_ACCESS_INDIRECT_ARGUMENT, D3D12_BARRIER_SYNC_EXECUTE_INDIRECT.0),
        (D3D12_BARRIER_ACCESS_COPY_DEST, D3D12_BARRIER_SYNC_COPY.0),
        (D3D12_BARRIER_ACCESS_COPY_SOURCE, D3D12_BARRIER_SYNC_COPY.0),
        (D3D12_BARRIER_ACCESS_RESOLVE_DEST, D3D12_BARRIER_SYNC_RESOLVE.0),
        (D3D12_BARRIER_ACCESS_RESOLVE_SOURCE, D3D12_BARRIER_SYNC_RESOLVE.0),
        (D3D12_BARRIER_ACCESS_RAYTRACING_ACCELERATION_STRUCTURE_READ, shading | D3D12_BARRIER_SYNC_BUILD_RAYTRACING_ACCELERATION_STRUCTURE.0
            | D3D12_BARRIER_SYNC_COPY_RAYTRACING_ACCELERATION_STRUCTURE.0 | D3D12_BARRIER_SYNC_EMIT_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO.0),
        (D3D12_BARRIER_ACCESS_RAYTRACING_ACCELERATION_STRUCTURE_WRITE, D3D12_BARRIER_SYNC_BUILD_RAYTRACING_ACCELERATION_STRUCTURE.0
            | D3D12_BARRIER_SYNC_COPY_RAYTRACING_ACCELERATION_STRUCTURE.0),
        (D3D12_BARRIER_ACCESS_SHADING_RATE_SOURCE, D3D12_BARRIER_SYNC_PIXEL_SHADING.0 | D3D12_BARRIER_SYNC_ALL_SHADING.0 | D3D12_BARRIER_SYNC_DRAW.0),
    ];
    table.iter().all(|&(bit, stages)| access & bit.0 == 0 || sync & stages != 0)
}

/// Accesses a texture layout permits (D3D12 enhanced barrier spec).
/// `None` = no restriction we enforce.
fn layout_access(layout: D3D12_BARRIER_LAYOUT) -> Option<i32> {
    use D3D12_BARRIER_ACCESS as A;
    let sr = D3D12_BARRIER_ACCESS_SHADER_RESOURCE.0;
    let copy = D3D12_BARRIER_ACCESS_COPY_SOURCE.0 | D3D12_BARRIER_ACCESS_COPY_DEST.0;
    let _ = A(0);
    Some(match layout {
        D3D12_BARRIER_LAYOUT_COMMON => sr | copy,
        D3D12_BARRIER_LAYOUT_GENERIC_READ => sr | D3D12_BARRIER_ACCESS_COPY_SOURCE.0
            | D3D12_BARRIER_ACCESS_RESOLVE_SOURCE.0 | D3D12_BARRIER_ACCESS_SHADING_RATE_SOURCE.0
            | D3D12_BARRIER_ACCESS_DEPTH_STENCIL_READ.0,
        D3D12_BARRIER_LAYOUT_RENDER_TARGET => D3D12_BARRIER_ACCESS_RENDER_TARGET.0,
        D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS => D3D12_BARRIER_ACCESS_UNORDERED_ACCESS.0,
        D3D12_BARRIER_LAYOUT_DEPTH_STENCIL_WRITE => D3D12_BARRIER_ACCESS_DEPTH_STENCIL_WRITE.0 | D3D12_BARRIER_ACCESS_DEPTH_STENCIL_READ.0,
        D3D12_BARRIER_LAYOUT_DEPTH_STENCIL_READ => D3D12_BARRIER_ACCESS_DEPTH_STENCIL_READ.0 | sr | D3D12_BARRIER_ACCESS_COPY_SOURCE.0 | D3D12_BARRIER_ACCESS_RESOLVE_SOURCE.0,
        D3D12_BARRIER_LAYOUT_SHADER_RESOURCE => sr,
        D3D12_BARRIER_LAYOUT_COPY_SOURCE => D3D12_BARRIER_ACCESS_COPY_SOURCE.0,
        D3D12_BARRIER_LAYOUT_COPY_DEST => D3D12_BARRIER_ACCESS_COPY_DEST.0,
        D3D12_BARRIER_LAYOUT_RESOLVE_SOURCE => D3D12_BARRIER_ACCESS_RESOLVE_SOURCE.0,
        D3D12_BARRIER_LAYOUT_RESOLVE_DEST => D3D12_BARRIER_ACCESS_RESOLVE_DEST.0,
        D3D12_BARRIER_LAYOUT_SHADING_RATE_SOURCE => D3D12_BARRIER_ACCESS_SHADING_RATE_SOURCE.0,
        D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COMMON | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COMMON =>
            sr | copy | D3D12_BARRIER_ACCESS_UNORDERED_ACCESS.0,
        D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_GENERIC_READ | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_GENERIC_READ =>
            sr | D3D12_BARRIER_ACCESS_COPY_SOURCE.0 | D3D12_BARRIER_ACCESS_RESOLVE_SOURCE.0 | D3D12_BARRIER_ACCESS_SHADING_RATE_SOURCE.0,
        D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_UNORDERED_ACCESS | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_UNORDERED_ACCESS =>
            D3D12_BARRIER_ACCESS_UNORDERED_ACCESS.0,
        D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_SHADER_RESOURCE | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_SHADER_RESOURCE => sr,
        D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COPY_SOURCE | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COPY_SOURCE => D3D12_BARRIER_ACCESS_COPY_SOURCE.0,
        D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COPY_DEST | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COPY_DEST => D3D12_BARRIER_ACCESS_COPY_DEST.0,
        _ => return None,
    })
}

/// Layouts a compute list cannot name become COMMON there.
fn compute_layout(layout: D3D12_BARRIER_LAYOUT) -> D3D12_BARRIER_LAYOUT {
    match layout.0 {
        2 | 4 | 5 | 9 | 10 | 11 | 18..=23 => D3D12_BARRIER_LAYOUT_COMMON,
        _ => layout,
    }
}

fn fix_texture(item: &mut D3D12_TEXTURE_BARRIER, compute: bool, simultaneous: bool) {
    if !enhanced_fix() { return; }
    let original = (item.SyncBefore.0, item.SyncAfter.0, item.AccessBefore.0, item.AccessAfter.0, item.LayoutBefore.0, item.LayoutAfter.0);
    if compute {
        item.LayoutBefore = compute_layout(item.LayoutBefore);
        item.LayoutAfter = compute_layout(item.LayoutAfter);
    }
    fix_half(&mut item.SyncBefore, &mut item.AccessBefore, compute);
    fix_half(&mut item.SyncAfter, &mut item.AccessAfter, compute);
    for (layout, access, sync) in [
        (item.LayoutBefore, &mut item.AccessBefore, &mut item.SyncBefore),
        (item.LayoutAfter, &mut item.AccessAfter, &mut item.SyncAfter),
    ] {
        if layout == D3D12_BARRIER_LAYOUT_UNDEFINED {
            *access = D3D12_BARRIER_ACCESS_NO_ACCESS;
            *sync = D3D12_BARRIER_SYNC_NONE;
        } else if access.0 != D3D12_BARRIER_ACCESS_NO_ACCESS.0
            && !(simultaneous && layout == D3D12_BARRIER_LAYOUT_COMMON)
            && let Some(allowed) = layout_access(layout)
        {
            access.0 &= allowed;
        }
    }
    let fixed = (item.SyncBefore.0, item.SyncAfter.0, item.AccessBefore.0, item.AccessAfter.0, item.LayoutBefore.0, item.LayoutAfter.0);
    if fixed != original {
        note_rewrite("texture", original, fixed);
    }
}

/// Planes of a host format: depth+stencil and two-plane video formats have 2.
fn plane_count(format: i32) -> u32 {
    match format {
        19..=22 | 44..=47 | 103 | 104 | 110 | 112 | 113 => 2,
        _ => 1,
    }
}

/// Keep the subresource range inside the host resource: Xbox depth+stencil
/// barriers name two planes of what may be a one-plane host format, and
/// out-of-range mips, slices or planes failed the host Close with E_INVALIDARG.
fn fix_range(range: &mut D3D12_BARRIER_SUBRESOURCE_RANGE, desc: &D3D12_RESOURCE_DESC) {
    let mips = u32::from(desc.MipLevels.max(1));
    let slices = if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D { 1 } else { u32::from(desc.DepthOrArraySize.max(1)) };
    let planes = plane_count(desc.Format.0);
    let all = D3D12_BARRIER_SUBRESOURCE_RANGE { IndexOrFirstMipLevel: u32::MAX, ..Default::default() };
    if range.IndexOrFirstMipLevel == u32::MAX {
        *range = all;
        return;
    }
    if range.NumMipLevels == 0 {
        // Single subresource index form.
        *range = if range.IndexOrFirstMipLevel >= mips * slices * planes { all } else {
            D3D12_BARRIER_SUBRESOURCE_RANGE { IndexOrFirstMipLevel: range.IndexOrFirstMipLevel, ..Default::default() }
        };
        return;
    }
    let clamp = |first: u32, count: u32, total: u32| -> Option<(u32, u32)> {
        if first >= total { return None; }
        let count = if count == 0 { total - first } else { count.min(total - first) };
        Some((first, count))
    };
    *range = match (
        clamp(range.IndexOrFirstMipLevel, range.NumMipLevels, mips),
        clamp(range.FirstArraySlice, range.NumArraySlices, slices),
        clamp(range.FirstPlane, range.NumPlanes, planes),
    ) {
        (Some((m, nm)), Some((a, na)), Some((pl, np))) if (m, nm, a, na, pl, np) != (0, mips, 0, slices, 0, planes) =>
            D3D12_BARRIER_SUBRESOURCE_RANGE {
                IndexOrFirstMipLevel: m, NumMipLevels: nm, FirstArraySlice: a,
                NumArraySlices: na, FirstPlane: pl, NumPlanes: np,
            },
        _ => all,
    };
}

/// Layouts the host resource's flags allow: an RT, depth or UAV layout
/// without its ALLOW_* flag (or a shader-resource layout on a
/// DENY_SHADER_RESOURCE image) is invalid on the host. Such a layout becomes
/// COMMON, consistently on both sides, so the layout sequence stays coherent.
fn fix_layout_flags(layout: &mut D3D12_BARRIER_LAYOUT, flags: D3D12_RESOURCE_FLAGS) {
    let has = |f: D3D12_RESOURCE_FLAGS| flags.0 & f.0 != 0;
    let ok = match *layout {
        D3D12_BARRIER_LAYOUT_RENDER_TARGET => has(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET),
        D3D12_BARRIER_LAYOUT_DEPTH_STENCIL_WRITE | D3D12_BARRIER_LAYOUT_DEPTH_STENCIL_READ =>
            has(D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL),
        D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_UNORDERED_ACCESS
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_UNORDERED_ACCESS => has(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS),
        D3D12_BARRIER_LAYOUT_SHADER_RESOURCE | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_SHADER_RESOURCE
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_SHADER_RESOURCE => !has(D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE),
        _ => true,
    };
    if !ok { *layout = D3D12_BARRIER_LAYOUT_COMMON; }
}

/// Accesses the host resource's flags allow.
fn flag_access(flags: D3D12_RESOURCE_FLAGS) -> i32 {
    let mut a = !0;
    if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 == 0 { a &= !D3D12_BARRIER_ACCESS_RENDER_TARGET.0; }
    if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 == 0 {
        a &= !(D3D12_BARRIER_ACCESS_DEPTH_STENCIL_WRITE.0 | D3D12_BARRIER_ACCESS_DEPTH_STENCIL_READ.0);
    }
    if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 { a &= !D3D12_BARRIER_ACCESS_UNORDERED_ACCESS.0; }
    if flags.0 & D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE.0 != 0 { a &= !D3D12_BARRIER_ACCESS_SHADER_RESOURCE.0; }
    a
}

/// Fit a texture barrier to the host resource behind it (range, flags,
/// DISCARD). Runs before fix_texture, which then fits accesses to layouts.
fn fit_host(item: &mut D3D12_TEXTURE_BARRIER, desc: &D3D12_RESOURCE_DESC) {
    if !enhanced_fix() { return; }
    let snapshot = |b: &D3D12_TEXTURE_BARRIER| {
        let r = b.Subresources;
        (r.IndexOrFirstMipLevel, r.NumMipLevels, r.FirstArraySlice, r.NumArraySlices, r.FirstPlane, r.NumPlanes,
         b.LayoutBefore.0, b.LayoutAfter.0, b.Flags.0, b.AccessBefore.0, b.AccessAfter.0)
    };
    let before = snapshot(item);
    fix_range(&mut item.Subresources, desc);
    fix_layout_flags(&mut item.LayoutBefore, desc.Flags);
    fix_layout_flags(&mut item.LayoutAfter, desc.Flags);
    let allowed = flag_access(desc.Flags);
    for access in [&mut item.AccessBefore, &mut item.AccessAfter] {
        if access.0 != D3D12_BARRIER_ACCESS_NO_ACCESS.0 { access.0 &= allowed; }
    }
    // DISCARD is only a hint; the host restricts which layouts take it.
    item.Flags = D3D12_TEXTURE_BARRIER_FLAGS(0);
    let after = snapshot(item);
    if before != after {
        static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let key = format!("{before:x?}");
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() < 24 && !seen.contains(&key) {
            crate::diagnostics::log_line(format_args!(
                "enhanced texture barrier fitted to host (format {} mips {} slices {} flags {:#x}): range/layouts/flags/access {before:x?} -> {after:x?}",
                desc.Format.0, desc.MipLevels, desc.DepthOrArraySize, desc.Flags.0));
            seen.push(key);
        }
    }
}

type Scratch = (D3D12_COMMAND_LIST_TYPE, ID3D12CommandAllocator, ID3D12GraphicsCommandList7);

/// TUXBOX_LIST_RING=1: record each barrier of a group alone on a scratch list
/// and Close it, then log the first 16 the host rejects with all fields.
fn probe(native: &ID3D12GraphicsCommandList7, group: &D3D12_BARRIER_GROUP) {
    if !ring_enabled() { return; }
    use std::cell::RefCell;
    thread_local! {
        static SCRATCH: RefCell<Vec<Scratch>> = const { RefCell::new(Vec::new()) };
    }
    static FAILED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if FAILED.load(Ordering::Relaxed) >= 16 { return; }
    let kind = unsafe { native.GetType() };
    let Some((allocator, scratch)) = SCRATCH.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some((_, a, l)) = cache.iter().find(|(k, ..)| *k == kind) { return Some((a.clone(), l.clone())); }
        let mut device: Option<ID3D12Device4> = None;
        unsafe { native.GetDevice(&mut device) }.ok()?;
        let device = device?;
        let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(kind) }.ok()?;
        let list: ID3D12GraphicsCommandList7 = unsafe { device.CreateCommandList1(0, kind, D3D12_COMMAND_LIST_FLAG_NONE) }.ok()?;
        cache.push((kind, allocator.clone(), list.clone()));
        Some((allocator, list))
    }) else { return };
    for i in 0..group.NumBarriers as usize {
        let single = unsafe {
            match group.Type {
                D3D12_BARRIER_TYPE_GLOBAL => D3D12_BARRIER_GROUP { NumBarriers: 1, Anonymous: D3D12_BARRIER_GROUP_0 { pGlobalBarriers: group.Anonymous.pGlobalBarriers.add(i) }, ..*group },
                D3D12_BARRIER_TYPE_BUFFER => D3D12_BARRIER_GROUP { NumBarriers: 1, Anonymous: D3D12_BARRIER_GROUP_0 { pBufferBarriers: group.Anonymous.pBufferBarriers.add(i) }, ..*group },
                _ => D3D12_BARRIER_GROUP { NumBarriers: 1, Anonymous: D3D12_BARRIER_GROUP_0 { pTextureBarriers: group.Anonymous.pTextureBarriers.add(i) }, ..*group },
            }
        };
        let result = unsafe {
            if allocator.Reset().is_err() || scratch.Reset(&allocator, None).is_err() { return; }
            scratch.Barrier(&[single]);
            scratch.Close()
        };
        let Err(error) = result else { continue };
        if FAILED.fetch_add(1, Ordering::Relaxed) >= 16 { return; }
        let text = unsafe {
            match group.Type {
                D3D12_BARRIER_TYPE_GLOBAL => {
                    let b = &*single.Anonymous.pGlobalBarriers;
                    format!("global sync {:#x}->{:#x} access {:#x}->{:#x}", b.SyncBefore.0, b.SyncAfter.0, b.AccessBefore.0, b.AccessAfter.0)
                }
                D3D12_BARRIER_TYPE_BUFFER => {
                    let b = &*single.Anonymous.pBufferBarriers;
                    format!("buffer sync {:#x}->{:#x} access {:#x}->{:#x} offset {:#x} size {:#x}",
                        b.SyncBefore.0, b.SyncAfter.0, b.AccessBefore.0, b.AccessAfter.0, b.Offset, b.Size)
                }
                _ => {
                    let b = &*single.Anonymous.pTextureBarriers;
                    let desc = b.pResource.as_ref().map(|r| r.GetDesc()).unwrap_or_default();
                    let r = b.Subresources;
                    format!("texture sync {:#x}->{:#x} access {:#x}->{:#x} layout {}->{} range [{:#x} {} {} {} {} {}] flags {:#x} \
                        | resource dim {} format {} {}x{}x{} mips {} flags {:#x} layout {}",
                        b.SyncBefore.0, b.SyncAfter.0, b.AccessBefore.0, b.AccessAfter.0, b.LayoutBefore.0, b.LayoutAfter.0,
                        r.IndexOrFirstMipLevel, r.NumMipLevels, r.FirstArraySlice, r.NumArraySlices, r.FirstPlane, r.NumPlanes, b.Flags.0,
                        desc.Dimension.0, desc.Format.0, desc.Width, desc.Height, desc.DepthOrArraySize, desc.MipLevels, desc.Flags.0, desc.Layout.0)
                }
            }
        };
        crate::diagnostics::log_line(format_args!(
            "barrier probe: host Close {:#010x} on list type {} for {text}", error.code().0, kind.0));
    }
}

fn note_rewrite(kind: &str, original: (i32, i32, i32, i32, i32, i32), fixed: (i32, i32, i32, i32, i32, i32)) {
    static SEEN: std::sync::Mutex<Vec<(i32, i32, i32, i32, i32, i32)>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 24 && !seen.contains(&original) {
        seen.push(original);
        crate::diagnostics::log_line(format_args!(
            "enhanced {kind} barrier sync {:#x}->{:#x} access {:#x}->{:#x} layout {}->{} recorded as sync {:#x}->{:#x} access {:#x}->{:#x} layout {}->{}",
            original.0, original.1, original.2, original.3, original.4, original.5,
            fixed.0, fixed.1, fixed.2, fixed.3, fixed.4, fixed.5,
        ));
    }
}

fn texture_group(api: &ID3D12GraphicsCommandList7, items: &[D3D12_TEXTURE_BARRIER]) {
    if !items.is_empty() {
        let group = D3D12_BARRIER_GROUP {
            Type: D3D12_BARRIER_TYPE_TEXTURE,
            NumBarriers: items.len() as u32,
            Anonymous: D3D12_BARRIER_GROUP_0 { pTextureBarriers: items.as_ptr() },
        };
        probe(api, &group);
        unsafe { api.Barrier(&[group]) };
    }
}

unsafe fn record(
    list: &List,
    native: &ID3D12GraphicsCommandList7,
    groups: &[D3D12_BARRIER_GROUP],
) -> Result<(), HRESULT> {
    let owner = parent(&list.parent);
    let compute = unsafe { list.native.GetType() } != D3D12_COMMAND_LIST_TYPE_DIRECT;
    for group in groups {
        let count = group.NumBarriers as usize;
        if count == 0 || unsafe { group.Anonymous.pGlobalBarriers.is_null() } {
            return Err(E_INVALIDARG);
        }
        match group.Type {
            D3D12_BARRIER_TYPE_GLOBAL => {
                let mut barriers: Vec<D3D12_GLOBAL_BARRIER> =
                    unsafe { slice::from_raw_parts(group.Anonymous.pGlobalBarriers, count) }.to_vec();
                for item in &mut barriers {
                    let original = (item.SyncBefore.0, item.SyncAfter.0, item.AccessBefore.0, item.AccessAfter.0, 0, 0);
                    fix_half(&mut item.SyncBefore, &mut item.AccessBefore, compute);
                    fix_half(&mut item.SyncAfter, &mut item.AccessAfter, compute);
                    let fixed = (item.SyncBefore.0, item.SyncAfter.0, item.AccessBefore.0, item.AccessAfter.0, 0, 0);
                    if fixed != original { note_rewrite("global", original, fixed); }
                }
                let fixed = D3D12_BARRIER_GROUP {
                    Anonymous: D3D12_BARRIER_GROUP_0 { pGlobalBarriers: barriers.as_ptr() },
                    ..*group
                };
                probe(native, &fixed);
                unsafe { native.Barrier(&[fixed]) };
            }
            D3D12_BARRIER_TYPE_BUFFER => {
                let mut barriers = Vec::new();
                barriers
                    .try_reserve_exact(count)
                    .map_err(|_| E_OUTOFMEMORY)?;
                for item in unsafe { slice::from_raw_parts(group.Anonymous.pBufferBarriers, count) }
                {
                    let resource =
                        unsafe { device::resource_proxy(guest_pointer(&item.pResource)?, owner) }
                            .ok_or(E_INVALIDARG)?;
                    let mut item = unsafe { ptr::read(item) };
                    if let Some((delta, len)) = resource.state.buffer_window() {
                        let size = if item.Size == u64::MAX && item.Offset == 0 {
                            len
                        } else {
                            item.Size
                        };
                        if size == 0 || item.Offset.checked_add(size).is_none_or(|end| end > len) {
                            return Err(E_INVALIDARG);
                        }
                        let host_end = delta
                            .checked_add(item.Offset)
                            .and_then(|offset| offset.checked_add(size))
                            .ok_or(E_INVALIDARG)?;
                        if host_end > unsafe { resource.native.GetDesc() }.Width {
                            return Err(E_INVALIDARG);
                        }
                        if item.SyncBefore == D3D12_BARRIER_SYNC_SPLIT
                            || item.SyncAfter == D3D12_BARRIER_SYNC_SPLIT
                            || item.AccessBefore == D3D12_BARRIER_ACCESS_NO_ACCESS
                            || item.AccessAfter == D3D12_BARRIER_ACCESS_NO_ACCESS
                        {
                            return Err(E_NOTIMPL);
                        }
                        // Desktop buffer barriers require the whole resource. A
                        // wider memory dependency also orders the guest window.
                        // Splitting or deactivating it could affect a neighbour.
                        item.Offset = 0;
                        item.Size = u64::MAX;
                    }
                    borrow_resource(&mut item.pResource, &resource.native);
                    let original = (item.SyncBefore.0, item.SyncAfter.0, item.AccessBefore.0, item.AccessAfter.0, 0, 0);
                    fix_half(&mut item.SyncBefore, &mut item.AccessBefore, compute);
                    fix_half(&mut item.SyncAfter, &mut item.AccessAfter, compute);
                    // A buffer has no render target, depth or shading-rate access.
                    let texture_only = D3D12_BARRIER_ACCESS_RENDER_TARGET.0 | D3D12_BARRIER_ACCESS_DEPTH_STENCIL_WRITE.0
                        | D3D12_BARRIER_ACCESS_DEPTH_STENCIL_READ.0 | D3D12_BARRIER_ACCESS_SHADING_RATE_SOURCE.0
                        | D3D12_BARRIER_ACCESS_RESOLVE_DEST.0 | D3D12_BARRIER_ACCESS_RESOLVE_SOURCE.0;
                    for access in [&mut item.AccessBefore, &mut item.AccessAfter] {
                        if enhanced_fix() && access.0 != D3D12_BARRIER_ACCESS_NO_ACCESS.0 { access.0 &= !texture_only; }
                    }
                    let fixed = (item.SyncBefore.0, item.SyncAfter.0, item.AccessBefore.0, item.AccessAfter.0, 0, 0);
                    if fixed != original { note_rewrite("buffer", original, fixed); }
                    barriers.push(item);
                }
                let fixed = D3D12_BARRIER_GROUP {
                    Anonymous: D3D12_BARRIER_GROUP_0 { pBufferBarriers: barriers.as_ptr() },
                    ..*group
                };
                probe(native, &fixed);
                unsafe { native.Barrier(&[fixed]) };
            }
            D3D12_BARRIER_TYPE_TEXTURE => {
                let mut barriers = Vec::new();
                barriers
                    .try_reserve_exact(count)
                    .map_err(|_| E_OUTOFMEMORY)?;
                for item in
                    unsafe { slice::from_raw_parts(group.Anonymous.pTextureBarriers, count) }
                {
                    let resource =
                        unsafe { device::resource_proxy(guest_pointer(&item.pResource)?, owner) }
                            .ok_or(E_INVALIDARG)?;
                    if let Some(pair) = resource.state.depth() {
                        // Copies belong between the preceding and following barriers.
                        texture_group(native, &barriers);
                        barriers.clear();
                        if depth::enhanced_handoff(native, &resource.native, pair, item)
                            .map_err(|error| error.code())?
                        {
                            continue;
                        }
                    }
                    let mut item = unsafe { ptr::read(item) };
                    borrow_resource(&mut item.pResource, &resource.native);
                    if resource.state.unbacked_uav() {
                        // The host depth image has no unordered access; see unbacked_uav.
                        item.LayoutBefore = device::format::host_layout(item.LayoutBefore);
                        item.LayoutAfter = device::format::host_layout(item.LayoutAfter);
                        item.AccessBefore &= !D3D12_BARRIER_ACCESS_UNORDERED_ACCESS;
                        item.AccessAfter &= !D3D12_BARRIER_ACCESS_UNORDERED_ACCESS;
                    }
                    let simultaneous = resource.state.placed().is_some();
                    if simultaneous {
                        // Simultaneous-access textures share COMMON across all uses.
                        if item.LayoutBefore != D3D12_BARRIER_LAYOUT_UNDEFINED {
                            item.LayoutBefore = D3D12_BARRIER_LAYOUT_COMMON;
                        }
                        if item.LayoutAfter != D3D12_BARRIER_LAYOUT_UNDEFINED {
                            item.LayoutAfter = D3D12_BARRIER_LAYOUT_COMMON;
                        }
                    }
                    let desc = unsafe { resource.native.GetDesc() };
                    if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER && enhanced_fix() {
                        // A texture barrier on a host buffer (a placed alias) is
                        // invalid; order its memory with a global barrier instead.
                        static BUFFER: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&BUFFER, || "enhanced texture barrier on a host buffer recorded as a global barrier".into());
                        let mut global = D3D12_GLOBAL_BARRIER {
                            SyncBefore: item.SyncBefore, SyncAfter: item.SyncAfter,
                            AccessBefore: item.AccessBefore, AccessAfter: item.AccessAfter,
                        };
                        fix_half(&mut global.SyncBefore, &mut global.AccessBefore, compute);
                        fix_half(&mut global.SyncAfter, &mut global.AccessAfter, compute);
                        texture_group(native, &barriers);
                        barriers.clear();
                        let group = D3D12_BARRIER_GROUP { Type: D3D12_BARRIER_TYPE_GLOBAL, NumBarriers: 1,
                            Anonymous: D3D12_BARRIER_GROUP_0 { pGlobalBarriers: &global } };
                        probe(native, &group);
                        unsafe { native.Barrier(&[group]) };
                        continue;
                    }
                    fit_host(&mut item, &desc);
                    fix_texture(&mut item, compute, simultaneous);
                    barriers.push(item);
                }
                texture_group(native, &barriers);
            }
            _ => return Err(E_NOTIMPL),
        }
    }
    Ok(())
}

pub(super) unsafe extern "system" fn barrier(
    list: &List,
    count: u32,
    groups: *const D3D12_BARRIER_GROUP,
) {
    let mut state = recording(list);
    if count == 0 || !state.can_record(&list.native) {
        return;
    }
    if groups.is_null() {
        state.error = E_INVALIDARG;
        return;
    }
    let Some(native) = list
        .state
        .enhanced_barriers
        .get_or_init(|| enhanced_barriers(&list.native))
    else {
        state.error = E_NOTIMPL;
        return;
    };
    if let Err(error) =
        unsafe { record(list, native, slice::from_raw_parts(groups, count as usize)) }
    {
        crate::diagnostics::recording_error(format_args!(
            "enhanced resource barrier group ({error:?})"
        ));
        state.error = error;
    }
}
