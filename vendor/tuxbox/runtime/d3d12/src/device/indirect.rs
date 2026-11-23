//! Xbox command signature descriptors and their guest COM identity.
use super::{Device, Out, compute, identity};
use crate::proxy::{Proxy, Slots, allocate, check_output, child_slots, unwrap};
use core::ffi::c_void;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT, Interface};

pub(crate) struct Action {
    pub(crate) stride: u32,
    pub(crate) kind: D3D12_INDIRECT_ARGUMENT_TYPE,
}
pub(crate) type Signature = Proxy<ID3D12CommandSignature, Action>;
static SIGNATURE: Slots<8> = child_slots::<ID3D12CommandSignature, Action, 8>();

// The guest holds this reference and the list's device throughout recording.
pub(crate) unsafe fn signature<'a>(
    guest: *mut c_void,
    device: *mut c_void,
) -> Option<&'a Signature> {
    unsafe { unwrap(guest, device, &SIGNATURE) }
}

pub(super) unsafe extern "system" fn create_signature(
    this: &Device,
    desc: *const D3D12_COMMAND_SIGNATURE_DESC,
    guest_root: *mut c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12CommandSignature>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.NumArgumentDescs == 0
        || desc.pArgumentDescs.is_null()
        || !desc.ByteStride.is_multiple_of(4)
    {
        return E_INVALIDARG;
    }
    if desc.NumArgumentDescs != 1 {
        return E_NOTIMPL;
    }
    // Only Type is initialized for these single-action signatures.
    let guest_kind = unsafe { (*desc.pArgumentDescs).Type };
    let (kind, record_size) = match (guest_kind, desc.ByteStride) {
        (D3D12_INDIRECT_ARGUMENT_TYPE_DRAW, _) => (guest_kind, size_of::<D3D12_DRAW_ARGUMENTS>()),
        (D3D12_INDIRECT_ARGUMENT_TYPE_DRAW_INDEXED, _) => {
            (guest_kind, size_of::<D3D12_DRAW_INDEXED_ARGUMENTS>())
        }
        (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH, _) | (D3D12_INDIRECT_ARGUMENT_TYPE(105), 12) => (
            D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH,
            size_of::<D3D12_DISPATCH_ARGUMENTS>(),
        ),
        (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH, _) => (guest_kind, size_of::<D3D12_DISPATCH_MESH_ARGUMENTS>()),
        (D3D12_INDIRECT_ARGUMENT_TYPE(107), 16) => (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH, 16),
        (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS, _) => {
            (guest_kind, size_of::<D3D12_DISPATCH_RAYS_DESC>())
        }
        _ => return E_NOTIMPL,
    };
    if (desc.ByteStride as usize) < record_size {
        return E_INVALIDARG;
    }
    let parent = identity(this);
    if !guest_root.is_null() {
        // Frosty supplies its global root for a 104-byte ray dispatch. The
        // host signature has no root-changing arguments and takes a null root.
        if guest_kind != D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS || desc.ByteStride != 104 {
            return E_INVALIDARG;
        }
        if let Err(error) = unsafe { compute::root_signature(guest_root, parent.as_raw()) } {
            return error;
        }
    }
    let argument = D3D12_INDIRECT_ARGUMENT_DESC {
        Type: kind,
        ..Default::default()
    };
    let host = D3D12_COMMAND_SIGNATURE_DESC {
        pArgumentDescs: &argument,
        ..*desc
    };
    let mut native = None;
    // The native call reads only these stack descriptors, synchronously.
    if let Err(error) = unsafe { this.native.CreateCommandSignature(&host, None, &mut native) } {
        return error.code();
    }
    let Some(native): Option<ID3D12CommandSignature> = native else {
        return E_FAIL;
    };
    unsafe {
        allocate(
            &SIGNATURE,
            &[
                ID3D12DeviceChild::IID,
                ID3D12Pageable::IID,
                ID3D12CommandSignature::IID,
            ],
            native,
            Some(&parent),
            Action {
                stride: desc.ByteStride,
                kind,
            },
            out,
        )
    }
}
