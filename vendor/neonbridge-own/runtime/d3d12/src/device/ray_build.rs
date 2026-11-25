//! The Xbox sizing call takes the same 24-byte inputs and three 64-bit sizes as DXR.
use super::Device;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

const _: () = assert!(
    size_of::<D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS>() == 24
        && size_of::<D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO>() == 24
);

/// The caller supplies NumDescs readable geometry descriptors or pointers for a BLAS.
/// A prebuild query can omit the eventual GPU buffers.
pub(crate) unsafe fn valid(inputs: &D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS) -> bool {
    let n = inputs.NumDescs as usize;
    if !matches!(
        inputs.DescsLayout,
        D3D12_ELEMENTS_LAYOUT_ARRAY | D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS
    ) {
        return false;
    }
    match inputs.Type {
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL => true,
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL => {
            let supported = |geometry: &D3D12_RAYTRACING_GEOMETRY_DESC| {
                matches!(
                    geometry.Type,
                    D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES
                        | D3D12_RAYTRACING_GEOMETRY_TYPE_PROCEDURAL_PRIMITIVE_AABBS
                )
            };
            if n == 0 {
                return true;
            }
            if inputs.DescsLayout == D3D12_ELEMENTS_LAYOUT_ARRAY {
                let ptr = unsafe { inputs.Anonymous.pGeometryDescs };
                !ptr.is_null()
                    && unsafe { core::slice::from_raw_parts(ptr, n) }
                        .iter()
                        .all(supported)
            } else {
                let ptr = unsafe { inputs.Anonymous.ppGeometryDescs };
                !ptr.is_null()
                    && unsafe { core::slice::from_raw_parts(ptr, n) }
                        .iter()
                        .all(|p| unsafe { p.as_ref() }.is_some_and(supported))
            }
        }
        _ => false,
    }
}

pub(super) unsafe extern "system" fn prebuild(
    device: &Device,
    inputs: *const D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS,
    out: *mut D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO,
) {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    *out = Default::default();
    if let Some(inputs) = unsafe { inputs.as_ref() }
        && unsafe { valid(inputs) }
        && let Ok(host) = device.native.cast::<ID3D12Device5>()
    {
        unsafe { host.GetRaytracingAccelerationStructurePrebuildInfo(inputs, out) };
    }
}
