use super::{List, parent, recording};
use crate::device::compute::root_proxy;
use core::ffi::c_void;
use windows::Win32::Foundation::{E_INVALIDARG, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{BOOL, Interface};

pub(super) unsafe extern "system" fn root(list: &List, signature: *mut c_void) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    match unsafe { root_proxy(signature, parent(&list.parent)) } {
        Ok(root) => {
            let native = root.map(|r| &r.native);
            state.bindings.graphics_root(native, root.map(|r| &r.state));
            state.unresolved_roots[0] = 0;
            unsafe { list.native.SetGraphicsRootSignature(native) };
        }
        Err(error) => state.error = error,
    }
}

pub(super) unsafe extern "system" fn topology(list: &List, primitive: D3D_PRIMITIVE_TOPOLOGY) {
    let mut state = recording(list);
    if state.can_record_graphics(&list.native) {
        let native = if primitive.0 == crate::device::rectangle::LIST {
            D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST
        } else {
            primitive
        };
        state.segments.replay.topology(native);
        unsafe { list.native.IASetPrimitiveTopology(native) };
    }
}

pub(super) unsafe extern "system" fn pipeline(list: &List, guest: *mut c_void) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    match unsafe { crate::device::compute::pipeline(guest, parent(&list.parent)) } {
        Ok(Some(host)) => {
            // A pipeline made without its translation binds its background
            // rebuild once a worker has published it.
            let (mut native, mut data, mut pending) = crate::device::compute::current(host);
            if pending && crate::device::compute::wait_pending_compute(host) {
                (native, data, pending) = crate::device::compute::current(host);
            }
            state.pipeline_pending = pending;
            if crate::diagnostics::capture::active() {
                state.capture_pipeline = data.key.clone();
            }
            state.stencil_slot = data.stencil_slot;
            state.null_pipeline = crate::diagnostics::display::is_null_pipeline(native.as_raw());
            state.bindings.pipeline = Some(host.into());
            state.bindings.draw_trace = data.draw_trace.clone();
            unsafe { list.native.SetPipelineState(native) };
        }
        _ => {
            crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::BindsRefused);
            { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        }
    }
}

pub(super) unsafe extern "system" fn viewports(
    list: &List,
    count: u32,
    views: *const D3D12_VIEWPORT,
) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    if count > D3D12_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE
        || (count != 0 && views.is_null())
    {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    unsafe { state.segments.replay.viewports(count, views) };
    unsafe { (list.native.vtable().RSSetViewports)(list.native.as_raw(), count, views) };
}

pub(super) unsafe extern "system" fn scissors(list: &List, count: u32, rects: *const RECT) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    if count > D3D12_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE
        || (count != 0 && rects.is_null())
    {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    unsafe { state.segments.replay.scissors(count, rects) };
    unsafe { (list.native.vtable().RSSetScissorRects)(list.native.as_raw(), count, rects) };
}

// At RVA 0x5790ccf the title passes the count and handle array in RDX/R8,
// the range flag in R9, and a pointer to an optional DSV on the stack.
pub(super) unsafe extern "system" fn targets(
    list: &List,
    count: u32,
    rtvs: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    contiguous: BOOL,
    dsv: *const D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    if count > D3D12_SIMULTANEOUS_RENDER_TARGET_COUNT || (count != 0 && rtvs.is_null()) {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    // CPU RTV descriptors are consumed by OMSetRenderTargets immediately.
    // Capture their resources now even if the guest overwrites a slot before drawing.
    if let Err(error) = unsafe {
        crate::device::placement::render_targets(
            parent(&list.parent), count, rtvs, contiguous.as_bool(), &mut state.bound_placed_targets,
        )
    } {
        { state.error = error.code(); state.error_at = (file!(), line!()); }
        return;
    }
    state.dsv_bound = !dsv.is_null();
    if let Some(h) = unsafe { dsv.as_ref() } {
        unsafe { crate::device::placement::touch_target(parent(&list.parent), h.ptr) };
    }
    if crate::diagnostics::display::capture_draws() {
        state.trace_target = if count == 1 {
            crate::diagnostics::display::draw_target(unsafe { (*rtvs).ptr })
        } else { None };
    }
    if crate::diagnostics::display::enabled() {
        // Contiguous ranges: the display target is bound alone in practice.
        let handles: Vec<usize> = (0..count as usize)
            .map(|i| unsafe { (*rtvs.add(if contiguous.as_bool() { 0 } else { i })).ptr })
            .collect();
        state.display_bound = crate::diagnostics::display::bound(&handles);
    }
    let binding = unsafe { dsv.as_ref() }
        .and_then(|h| unsafe { crate::device::stencil::binding(parent(&list.parent), h.ptr) });
    if let Some((native, surface)) = binding {
        if state.stencil_targets.is_none() {
            match super::stencil::Targets::new(native) {
                Ok(targets) => state.stencil_targets = Some(targets),
                Err(e) => {
                    { state.error = e.code(); state.error_at = (file!(), line!()); }
                    return;
                }
            }
        }
        if let Some(targets) = &mut state.stencil_targets {
            unsafe {
                targets.snapshot(surface, count, rtvs, contiguous.as_bool(), *dsv);
            }
        }
    } else if let Some(targets) = &mut state.stencil_targets {
        targets.unbind();
    }
    unsafe { state.segments.replay.targets(count, rtvs, contiguous.as_bool(), dsv) };
    unsafe {
        (list.native.vtable().OMSetRenderTargets)(
            list.native.as_raw(),
            count,
            rtvs,
            contiguous,
            dsv,
        )
    };
}

pub(super) unsafe extern "system" fn draw(
    list: &List,
    vertices: u32,
    instances: u32,
    start_vertex: u32,
    start_instance: u32,
) {
    let mut state = recording(list);
    if state.can_record_graphics(&list.native) {
        state.draw(list, vertices != 0 && instances != 0, || unsafe {
            list.native
                .DrawInstanced(vertices, instances, start_vertex, start_instance);
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || format!("recorded DrawInstanced list={list:p} vertices={vertices} instances={instances}"));
        });
    }
}

// Series slot 126 takes a base rate and two D3D12_SHADING_RATE_COMBINERs.
pub(super) unsafe extern "system" fn shading_rate(
    list: &List,
    rate: D3D12_SHADING_RATE,
    combiners: *const D3D12_SHADING_RATE_COMBINER,
) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    if !matches!(rate, D3D12_SHADING_RATE_1X1 | D3D12_SHADING_RATE_1X2 | D3D12_SHADING_RATE_2X1 |
        D3D12_SHADING_RATE_2X2 | D3D12_SHADING_RATE_2X4 | D3D12_SHADING_RATE_4X2 | D3D12_SHADING_RATE_4X4)
        || (!combiners.is_null() && unsafe { std::slice::from_raw_parts(combiners, 2) }.iter()
            .any(|op| !(0..=4).contains(&op.0))) {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    let native = match list.native.cast::<ID3D12GraphicsCommandList5>() {
        Ok(native) => native,
        Err(error) => { { state.error = error.code(); state.error_at = (file!(), line!()); } return; }
    };
    unsafe { native.RSSetShadingRate(rate, (!combiners.is_null()).then_some(combiners)) };
}

// SET_CONTEXT_REG DB_RENDER_OVERRIDE.FORCE_SHADER_Z_ORDER tells the DB to
// obey the bound shader's Z_ORDER. The translated PS carries the observable
// ordering in DXIL, including earlydepthstencil for early-depth color shaders.
// Desktop D3D12 always honors this shader contract; it has no dynamic override.
pub(super) unsafe extern "system" fn force_shader_depth_order(list: &List, enable: u32) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if enable > 1 || unsafe { list.native.GetType() } != D3D12_COMMAND_LIST_TYPE_DIRECT {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    static MODES: [std::sync::Once; 2] = [const { std::sync::Once::new() }; 2];
    crate::diagnostics::log_once(&MODES[enable as usize], || {
        format!("SetPixelShaderDepthForceZOrderX({enable}): bound pixel shader controls depth ordering")
    });
}

// SetSampleMaskX programs an additional 16-bit coverage mask per pixel in a
// 2x2 quad. Enabling every sample preserves the coverage of the bound PSO.
pub(super) unsafe extern "system" fn sample_mask(list: &List, mask: u64) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    if mask != u64::MAX {
        crate::diagnostics::recording_error(format_args!("SetSampleMaskX quad mask {mask:#018x}"));
        { state.error = windows::Win32::Foundation::E_NOTIMPL; state.error_at = (file!(), line!()); }
    }
}

// FD3D12CommandContext::RHIDispatchMeshShader calls slot 127 at Frosty
// RVA 0x578f241 with X/Y/Z group counts in the standard register order.
pub(super) unsafe extern "system" fn dispatch_mesh(list: &List, x: u32, y: u32, z: u32) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if x.max(y).max(z) > D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION
        || u64::from(x) * u64::from(y) * u64::from(z) > 1 << 22
        || !matches!(
            unsafe { list.native.GetType() },
            D3D12_COMMAND_LIST_TYPE_DIRECT | D3D12_COMMAND_LIST_TYPE_BUNDLE
        )
    {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    let Some(native) = list.state.mesh.get_or_init(|| list.native.cast().ok()) else {
        { state.error = windows::Win32::Foundation::E_NOINTERFACE; state.error_at = (file!(), line!()); }
        return;
    };
    state.draw(list, x != 0 && y != 0 && z != 0, || unsafe { native.DispatchMesh(x, y, z) });
    static FIRST_MESH: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST_MESH, || {
        format!("recorded Xbox slot 127 DispatchMesh ({x}, {y}, {z})")
    });
}

// Frosty's RHIDrawIndexedPrimitive chooses DXGI_FORMAT_R16_UINT or R32_UINT
// at RVA 0x578ec32 and sends the packed GPU address, size and format to slot 47.
pub(super) unsafe extern "system" fn index_buffer(
    list: &List,
    view: *const D3D12_INDEX_BUFFER_VIEW,
) {
    let mut state = recording(list);
    if state.can_record_graphics(&list.native) {
        let mut host = unsafe { view.as_ref() }.copied();
        if let Some(v) = &mut host {
            let Some(address) = super::compute::address(list, &mut state, v.BufferLocation, v.SizeInBytes.into()) else { return; };
            v.BufferLocation = address;
            if address == 0 && v.SizeInBytes != 0 {
                crate::diagnostics::null_va("index buffer", format_args!("{} bytes, format {}", v.SizeInBytes, v.Format.0));
            }
        }
        state.segments.replay.index(host);
        unsafe { list.native.IASetIndexBuffer(host.as_ref().map(core::ptr::from_ref)) };

    }
}

/// TUXBOX_NULL_UNBACKED_STREAMS=0: a vertex stream with no backing passes its
/// guest address through (and an import failure fails the list), as before.
/// On: it is bound as an empty stream and logged (the bounded vertex-stream
/// binding of Xbox2Windows fc547dd7, re-derived; a stream crossing placements
/// is still served by one import covering it).
fn null_unbacked_streams() -> bool {
    crate::env_toggle!("TUXBOX_NULL_UNBACKED_STREAMS") != Some("0")
}

fn note_null_stream(address: u64, bytes: u32, error: Option<windows::core::HRESULT>) {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "vertex stream {address:#x} ({bytes} bytes) has no backing ({error:?}): bound empty ({n} so far; TUXBOX_NULL_UNBACKED_STREAMS=0 passes it through)"));
    }
}

/// IASetVertexBuffers(start, count, NULL) clears those slots with explicit
/// empty views (TUXBOX_NULL_VIEWS_UNBIND=0 forwards the null pointer;
/// VIctory 31ce30f, re-derived).
fn null_views_unbind() -> bool {
    crate::env_toggle!("TUXBOX_NULL_VIEWS_UNBIND") != Some("0")
}

pub(super) unsafe extern "system" fn vertex_buffers(
    list: &List,
    start: u32,
    count: u32,
    views: *const D3D12_VERTEX_BUFFER_VIEW,
) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    if start > D3D12_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT
        || count > D3D12_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT - start
    {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    if views.is_null() && count != 0 && null_views_unbind() {
        // A null view array unbinds the slots: bind empty views explicitly
        // rather than trusting the driver with a null pointer.
        let empty = [D3D12_VERTEX_BUFFER_VIEW::default(); D3D12_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT as usize];
        state.segments.replay.vertex(start, &empty[..count as usize]);
        unsafe { list.native.IASetVertexBuffers(start, Some(&empty[..count as usize])) };
        return;
    }
    if views.is_null() || count == 0 {
        unsafe { (list.native.vtable().IASetVertexBuffers)(list.native.as_raw(), start, count, views) };
        return;
    }
    let mut translated = [D3D12_VERTEX_BUFFER_VIEW::default(); D3D12_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT as usize];
    let host = &mut translated[..count as usize];
    host.copy_from_slice(unsafe { core::slice::from_raw_parts(views, count as usize) });
    for v in host.iter_mut() {
        if null_unbacked_streams() && v.BufferLocation != 0 {
            // A stream no title buffer or committed page backs would hand the
            // guest VA to the GPU as a host address; bind it empty instead.
            let backed = unsafe { crate::device::command_buffer_address(super::parent(&list.parent), v.BufferLocation, u64::from(v.SizeInBytes).max(1)) };
            match backed {
                Ok((Some(buffer), va)) => {
                    if let Err(e) = super::raytracing::retain(&mut state, &[&buffer]) { { state.error = e.code(); state.error_at = (file!(), line!()); } return; }
                    v.BufferLocation = va;
                }
                Ok((None, _)) | Err(_) => {
                    note_null_stream(v.BufferLocation, v.SizeInBytes, backed.err().map(|e| e.code()));
                    *v = D3D12_VERTEX_BUFFER_VIEW::default();
                }
            }
            continue;
        }
        let Some(address) = super::compute::address(list, &mut state, v.BufferLocation, v.SizeInBytes.into()) else { return; };
        v.BufferLocation = address;
    }
    if let Some(v) = host.iter().find(|v| v.BufferLocation == 0 && v.SizeInBytes != 0) {
        crate::diagnostics::null_va("vertex buffer", format_args!("{} bytes, stride {}", v.SizeInBytes, v.StrideInBytes));
    }
    state.segments.replay.vertex(start, host);
    unsafe { list.native.IASetVertexBuffers(start, Some(host)) };

}

// Slot 13 at Frosty RVA 0x578ecf6 uses the desktop five-argument ordering.
pub(super) unsafe extern "system" fn draw_indexed(
    list: &List,
    indices: u32,
    instances: u32,
    first: u32,
    base_vertex: i32,
    first_instance: u32,
) {
    let mut state = recording(list);
    if state.can_record_graphics(&list.native) {
        state.draw(list, indices != 0 && instances != 0, || unsafe {
            list.native
                .DrawIndexedInstanced(indices, instances, first, base_vertex, first_instance);
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || format!("recorded DrawIndexedInstanced list={list:p} indices={indices} instances={instances}"));
        });
    }
}

// Frosty's ApplyState records these output-merger values at RVAs 0x57f0356 and 0x57f0398.
pub(super) unsafe extern "system" fn blend_factor(list: &List, factor: *const f32) {
    let mut state = recording(list);
    if state.can_record_graphics(&list.native) {
        unsafe { state.segments.replay.blend(factor) };
        unsafe { (list.native.vtable().OMSetBlendFactor)(list.native.as_raw(), factor) };
    }
}

pub(super) unsafe extern "system" fn stencil_ref(list: &List, reference: u32) {
    let mut state = recording(list);
    if state.can_record_graphics(&list.native) {
        state.segments.replay.stencil(reference);
        unsafe { list.native.OMSetStencilRef(reference) };
    }
}

// ApplyState loads the depth minimum and maximum into XMM1/XMM2 before the
// slot 68 call returning at title RVA 0x57f03ed.
pub(super) unsafe extern "system" fn depth_bounds(list: &List, minimum: f32, maximum: f32) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    let native = list
        .state
        .extended
        .get_or_init(|| list.native.cast::<ID3D12GraphicsCommandList1>().ok());
    match native {
        Some(native) => {
            state.segments.replay.depth_bounds(minimum, maximum);
            unsafe { native.OMSetDepthBounds(minimum, maximum) }
        }
        None => state.error = windows::Win32::Foundation::E_NOINTERFACE,
    }
}
