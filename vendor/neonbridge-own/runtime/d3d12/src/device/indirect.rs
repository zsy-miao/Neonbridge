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

/// The kind of a signature accepted in place of a refusal: its indirect
/// calls are dropped at ExecuteIndirect (TUXBOX_SIGNATURE_ACCEPT).
pub(crate) const DROPPED: D3D12_INDIRECT_ARGUMENT_TYPE = D3D12_INDIRECT_ARGUMENT_TYPE(-1);

/// TUXBOX_SIGNATURE_ACCEPT (default on, =0 refuses as before): a signature we
/// cannot make (more than one argument, an unmapped type, a stride short of
/// its record or not on 4 bytes, no arguments, a root it does not take) is
/// handed out anyway and its calls dropped at execute, as VIctory-fxceless
/// victory-gpu signature.rs create keeps every layout and draw.rs
/// execute_indirect drops what cannot run on the host (GPL-3.0, credit to
/// its authors). The native behind it is a plain draw signature.
fn accept_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_SIGNATURE_ACCEPT") != Some("0"))
}

unsafe fn accepted(this: &Device, desc: &D3D12_COMMAND_SIGNATURE_DESC, refusal: HRESULT, out: Out) -> HRESULT {
    if !accept_enabled() {
        return refusal;
    }
    let stride = desc.ByteStride.max(size_of::<D3D12_DRAW_ARGUMENTS>() as u32).next_multiple_of(4);
    let argument = D3D12_INDIRECT_ARGUMENT_DESC { Type: D3D12_INDIRECT_ARGUMENT_TYPE_DRAW, ..Default::default() };
    let host = D3D12_COMMAND_SIGNATURE_DESC { ByteStride: stride, NumArgumentDescs: 1, pArgumentDescs: &argument, NodeMask: desc.NodeMask };
    let mut native = None;
    if unsafe { this.native.CreateCommandSignature(&host, None, &mut native) }.is_err() {
        return refusal;
    }
    let Some(native): Option<ID3D12CommandSignature> = native else { return refusal };
    let parent = identity(this);
    unsafe {
        allocate(
            &SIGNATURE,
            &[ID3D12DeviceChild::IID, ID3D12Pageable::IID, ID3D12CommandSignature::IID],
            native,
            Some(&parent),
            Action { stride: desc.ByteStride, kind: DROPPED },
            out,
        )
    }
}
static SIGNATURE: Slots<8> = child_slots::<ID3D12CommandSignature, Action, 8>();

// The guest holds this reference and the list's device throughout recording.
pub(crate) unsafe fn signature<'a>(
    guest: *mut c_void,
    device: *mut c_void,
) -> Option<&'a Signature> {
    unsafe { unwrap(guest, device, &SIGNATURE) }
}

/// A signature the title asked for and we refuse; its indirect work is lost.
/// Logged (first 8, then powers of two) with the first argument's type.
fn refused(desc: &D3D12_COMMAND_SIGNATURE_DESC, why: &str) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        let first = unsafe { (*desc.pArgumentDescs).Type.0 };
        crate::diagnostics::log_line(format_args!(
            "command signature refused ({why}): {} arguments, first type {first}, stride {} ({n} so far)",
            desc.NumArgumentDescs, desc.ByteStride));
    }
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
    if desc.pArgumentDescs.is_null() && desc.NumArgumentDescs != 0 {
        return E_INVALIDARG;
    }
    if desc.NumArgumentDescs == 0 || !desc.ByteStride.is_multiple_of(4) {
        return unsafe { accepted(this, desc, E_INVALIDARG, out) };
    }
    if desc.NumArgumentDescs != 1 {
        refused(desc, "more than one argument");
        return unsafe { accepted(this, desc, E_NOTIMPL, out) };
    }
    // Only Type is initialized for these single-action signatures.
    let guest_kind = unsafe { (*desc.pArgumentDescs).Type };
    let (kind, record_size) = match (guest_kind, desc.ByteStride) {
        (D3D12_INDIRECT_ARGUMENT_TYPE_DRAW, _) => (guest_kind, size_of::<D3D12_DRAW_ARGUMENTS>()),
        (D3D12_INDIRECT_ARGUMENT_TYPE_DRAW_INDEXED, _) => {
            (guest_kind, size_of::<D3D12_DRAW_INDEXED_ARGUMENTS>())
        }
        // leonida c8247cdf: the console's dispatch type 105 is a DISPATCH at
        // any stride (it was taken only at 12 bytes).
        (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH | D3D12_INDIRECT_ARGUMENT_TYPE(105), _) => (
            D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH,
            size_of::<D3D12_DISPATCH_ARGUMENTS>(),
        ),
        (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH, _) => (guest_kind, size_of::<D3D12_DISPATCH_MESH_ARGUMENTS>()),
        (D3D12_INDIRECT_ARGUMENT_TYPE(107), 16) => (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH, 16),
        (D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS, _) => {
            (guest_kind, size_of::<D3D12_DISPATCH_RAYS_DESC>())
        }
        _ => {
            refused(desc, "argument type not mapped");
            return unsafe { accepted(this, desc, E_NOTIMPL, out) };
        }
    };
    if (desc.ByteStride as usize) < record_size {
        return unsafe { accepted(this, desc, E_INVALIDARG, out) };
    }
    let parent = identity(this);
    if !guest_root.is_null() {
        // Frosty supplies its global root for a 104-byte ray dispatch. The
        // host signature has no root-changing arguments and takes a null root.
        if guest_kind != D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS || desc.ByteStride != 104 {
            return unsafe { accepted(this, desc, E_INVALIDARG, out) };
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
