//! Frosty binds a state object at list slot 120, then supplies the 104-byte
//! dispatch at slot 121 (retail RVAs 0x57b812e and 0x57b8187).
use super::{List, parent, recording};
use core::ffi::c_void;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;

pub(super) unsafe extern "system" fn pipeline(list: &List, object: *mut c_void) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let Some(host) = &list.state.raytracing else {
        { state.error = E_NOTIMPL; state.error_at = (file!(), line!()); }
        return;
    };
    let Some(native) = (unsafe { crate::device::ray_state(object, parent(&list.parent)) }) else {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    };
    state.bindings.pipeline = Some(super::compute_bindings::Pipeline::Ray(native.clone()));
    state.bindings.draw_trace = None;
    state.pipeline_pending = false;
    state.pending_pipeline = 0;
    unsafe {
        host.SetPipelineState1(native);
    }
}

const _: () = assert!(size_of::<D3D12_DISPATCH_RAYS_DESC>() == 104);

/// TUXBOX_RAYS=0 skips every DispatchRays, direct and indirect (an A/B for
/// device-hung runs; pairs with TUXBOX_AS_BUILDS=0). Logged once.
pub(super) fn skip_rays() -> bool {
    let skip = crate::env_toggle!("TUXBOX_RAYS") == Some("0");
    if skip {
        static ONCE: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&ONCE, || "DispatchRays skipped (TUXBOX_RAYS=0)".to_owned());
    }
    skip
}

pub(super) unsafe extern "system" fn dispatch(list: &List, desc: *const D3D12_DISPATCH_RAYS_DESC) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let Some(host) = &list.state.raytracing else {
        { state.error = E_NOTIMPL; state.error_at = (file!(), line!()); }
        return;
    };
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    };
    let table_alignment = u64::from(D3D12_RAYTRACING_SHADER_TABLE_BYTE_ALIGNMENT);
    let record_alignment = u64::from(D3D12_RAYTRACING_SHADER_RECORD_BYTE_ALIGNMENT);
    let raygen = desc.RayGenerationShaderRecord;
    let threads = u64::from(desc.Width)
        .saturating_mul(desc.Height.into())
        .saturating_mul(desc.Depth.into());
    if threads > D3D12_RAYTRACING_MAX_RAY_GENERATION_SHADER_THREADS.into()
        || !raygen.StartAddress.is_multiple_of(table_alignment)
        || [
            desc.MissShaderTable,
            desc.HitGroupTable,
            desc.CallableShaderTable,
        ]
        .iter()
        .any(|t| {
            t.SizeInBytes != 0
                && (!t.StartAddress.is_multiple_of(table_alignment)
                    || !t.StrideInBytes.is_multiple_of(record_alignment)
                    || t.StrideInBytes > u64::from(D3D12_RAYTRACING_MAX_SHADER_RECORD_STRIDE))
        })
    {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    if raygen.StartAddress == 0 && raygen.SizeInBytes != 0 {
        crate::diagnostics::null_va("ray generation record", format_args!("{} bytes, {}x{}x{}", raygen.SizeInBytes, desc.Width, desc.Height, desc.Depth));
    }
    for (name, t) in [("miss table", desc.MissShaderTable), ("hit group table", desc.HitGroupTable), ("callable table", desc.CallableShaderTable)] {
        if t.StartAddress == 0 && t.SizeInBytes != 0 {
            crate::diagnostics::null_va(name, format_args!("{} bytes, stride {}", t.SizeInBytes, t.StrideInBytes));
        }
    }
    if skip_rays() {
        return;
    }
    state.sample_heap();
    unsafe {
        host.DispatchRays(desc);
    }
}
