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
    if !state.can_record(&list.native) {
        return;
    }
    match unsafe { root_proxy(signature, parent(&list.parent)) } {
        Ok(root) => {
            let native = root.map(|r| &r.native);
            state.bindings.graphics_root(native, root.map(|r| &r.state));
            unsafe { list.native.SetGraphicsRootSignature(native) };
        }
        Err(error) => state.error = error,
    }
}

pub(super) unsafe extern "system" fn topology(list: &List, primitive: D3D_PRIMITIVE_TOPOLOGY) {
    let mut state = recording(list);
    if state.can_record(&list.native) {
        let native = if primitive.0 == crate::device::rectangle::LIST {
            D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST
        } else {
            primitive
        };
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
            state.stencil_slot = host.state.stencil_slot;
            state.bindings.null_usage = host.state.null_usage.clone();
            state.bindings.pipeline = Some(host.into());
            state.bindings.draw_trace = host.state.draw_trace.clone();
            unsafe { list.native.SetPipelineState(&host.native) };
        }
        _ => state.error = E_INVALIDARG,
    }
}

pub(super) unsafe extern "system" fn viewports(
    list: &List,
    count: u32,
    views: *const D3D12_VIEWPORT,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if count > D3D12_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE
        || (count != 0 && views.is_null())
    {
        state.error = E_INVALIDARG;
        return;
    }
    unsafe { (list.native.vtable().RSSetViewports)(list.native.as_raw(), count, views) };
}

pub(super) unsafe extern "system" fn scissors(list: &List, count: u32, rects: *const RECT) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if count > D3D12_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE
        || (count != 0 && rects.is_null())
    {
        state.error = E_INVALIDARG;
        return;
    }
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
    if !state.can_record(&list.native) {
        return;
    }
    if count > D3D12_SIMULTANEOUS_RENDER_TARGET_COUNT || (count != 0 && rtvs.is_null()) {
        state.error = E_INVALIDARG;
        return;
    }
    // CPU RTV descriptors are consumed by OMSetRenderTargets immediately.
    // Capture their resources now even if the guest overwrites a slot before drawing.
    if let Err(error) = unsafe {
        crate::device::placement::render_targets(
            parent(&list.parent), count, rtvs, contiguous.as_bool(), &mut state.bound_placed_targets,
        )
    } {
        state.error = error.code();
        return;
    }
    state.dsv_bound = !dsv.is_null();
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
                    state.error = e.code();
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
    if state.can_record(&list.native) {
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
    if !state.can_record(&list.native) {
        return;
    }
    if !matches!(rate, D3D12_SHADING_RATE_1X1 | D3D12_SHADING_RATE_1X2 | D3D12_SHADING_RATE_2X1 |
        D3D12_SHADING_RATE_2X2 | D3D12_SHADING_RATE_2X4 | D3D12_SHADING_RATE_4X2 | D3D12_SHADING_RATE_4X4)
        || (!combiners.is_null() && unsafe { std::slice::from_raw_parts(combiners, 2) }.iter()
            .any(|op| !(0..=4).contains(&op.0))) {
        state.error = E_INVALIDARG;
        return;
    }
    let native = match list.native.cast::<ID3D12GraphicsCommandList5>() {
        Ok(native) => native,
        Err(error) => { state.error = error.code(); return; }
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
        state.error = E_INVALIDARG;
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
    if !state.can_record(&list.native) {
        return;
    }
    // A mask that differs between the four pixels of a quad, such as the
    // checkerboard 0x0000ffffffff0000, has no desktop equivalent: the host has
    // one pipeline sample mask for every pixel. Draw with full coverage. Pixels
    // the title meant to leave untouched are shaded too. Failing the list here
    // would poison the queue and fail the title's Present.
    if mask != u64::MAX {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || {
            format!("SetSampleMaskX quad mask {mask:#018x} ignored; drawing with full coverage")
        });
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
        state.error = E_INVALIDARG;
        return;
    }
    let Some(native) = list.state.mesh.get_or_init(|| list.native.cast().ok()) else {
        state.error = windows::Win32::Foundation::E_NOINTERFACE;
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
    if state.can_record(&list.native) {
        let mut host = unsafe { view.as_ref() }.copied();
        if let Some(v) = &mut host {
            let Some(address) = super::compute::address(list, &mut state, v.BufferLocation, v.SizeInBytes.into()) else { return; };
            v.BufferLocation = address;
        }
        unsafe { list.native.IASetIndexBuffer(host.as_ref().map(core::ptr::from_ref)) };

    }
}

pub(super) unsafe extern "system" fn vertex_buffers(
    list: &List,
    start: u32,
    count: u32,
    views: *const D3D12_VERTEX_BUFFER_VIEW,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if start > D3D12_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT
        || count > D3D12_IA_VERTEX_INPUT_RESOURCE_SLOT_COUNT - start
    {
        state.error = E_INVALIDARG;
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
        let Some(address) = super::compute::address(list, &mut state, v.BufferLocation, v.SizeInBytes.into()) else { return; };
        v.BufferLocation = address;
    }
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
    if state.can_record(&list.native) {
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
    if state.can_record(&list.native) {
        unsafe { (list.native.vtable().OMSetBlendFactor)(list.native.as_raw(), factor) };
    }
}

pub(super) unsafe extern "system" fn stencil_ref(list: &List, reference: u32) {
    let mut state = recording(list);
    if state.can_record(&list.native) {
        unsafe { list.native.OMSetStencilRef(reference) };
    }
}

// ApplyState loads the depth minimum and maximum into XMM1/XMM2 before the
// slot 68 call returning at title RVA 0x57f03ed.
pub(super) unsafe extern "system" fn depth_bounds(list: &List, minimum: f32, maximum: f32) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let native = list
        .state
        .extended
        .get_or_init(|| list.native.cast::<ID3D12GraphicsCommandList1>().ok());
    match native {
        Some(native) => unsafe { native.OMSetDepthBounds(minimum, maximum) },
        None => state.error = windows::Win32::Foundation::E_NOINTERFACE,
    }
}
