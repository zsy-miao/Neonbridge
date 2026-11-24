use super::{Device, Out, compute, identity, rectangle, wrap_pageable};
use crate::proxy::check_output;
use core::{ffi::c_void, mem::ManuallyDrop};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT, Interface};

// Frosty's device slot 60 takes a graphics descriptor and an extension array
// before the IID/output pair (retail call at RVA 0x5759a60).
pub(super) unsafe extern "system" fn create_pipeline(
    this: &Device,
    desc: *const D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    extension_count: u32,
    _extensions: *const c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let checked = unsafe { check_output::<ID3D12PipelineState>(iid, out) };
    if checked.is_err() {
        return checked;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if extension_count != 0 || desc.CachedPSO.CachedBlobSizeInBytes != 0 {
        return E_NOTIMPL;
    }
    if desc.VS.pShaderBytecode.is_null() || desc.VS.BytecodeLength == 0 {
        return E_INVALIDARG;
    }
    let guest_root = desc
        .pRootSignature
        .as_ref()
        .map_or(core::ptr::null_mut(), Interface::as_raw);
    let root = match unsafe { compute::root_signature(guest_root, identity(this).as_raw()) } {
        Ok(root) => root,
        Err(error) => return error,
    };
    // The descriptor borrows its shader blobs, layout arrays and root signature
    // for this synchronous call. Replace its guest pointer without taking a
    // reference on the guest object. Release only the new native reference.
    let mut native_desc = unsafe { core::ptr::read(desc) };
    native_desc.pRootSignature = ManuallyDrop::new(root.cloned());
    super::format::host_targets(&mut native_desc.RTVFormats, &mut native_desc.DSVFormat);
    let native = if native_desc.PrimitiveTopologyType.0 == rectangle::PIPELINE_TYPE {
        crate::big_stack::run(|| unsafe { rectangle::create_pipeline(&this.native, &mut native_desc) })
    } else {
        crate::big_stack::run(|| unsafe {
            this.native
                .CreateGraphicsPipelineState::<ID3D12PipelineState>(&native_desc)
        })
    };
    unsafe { ManuallyDrop::drop(&mut native_desc.pRootSignature) };
    unsafe {
        wrap_pageable(
            native,
            &compute::PIPELINE,
            this,
            compute::PipelineData::default(),
            out,
        )
    }
}
