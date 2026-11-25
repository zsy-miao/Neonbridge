//! Wrapped root signatures and the Xbox extended compute pipeline factory.
use super::{Device, Out, identity, wrap_pageable};
use crate::proxy::{Proxy, Slots, allocate, check_output, child_slots, unwrap};
use core::{ffi::c_void, mem::ManuallyDrop, slice};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT, Interface};

pub(crate) use super::compute_table::ComputeTable;
pub(crate) use super::compute_scene::{RootSource, SceneRoot, SceneSource};

static ROOT: Slots<8> = child_slots::<ID3D12RootSignature, RootSource, 8>();
pub(crate) type Root = Proxy<ID3D12RootSignature, RootSource>;
pub(crate) type Pipeline = Proxy<ID3D12PipelineState, PipelineData>;
// Inline console SetPipelineState copies 32 bytes starting at +0x10 from
// this object to the guest command list. Desktop command lists use slot 25
// with an empty packet buffer, so no Xbox command packets are advertised.
#[repr(C)]
#[derive(Default)]
pub(crate) struct PipelineData {
    reserved: usize,
    packets: [u8; 32],
    pub(crate) stencil_slot: Option<u32>,
    pub(crate) draw_trace: Option<std::sync::Arc<crate::diagnostics::RasterDraw>>,
    pub(crate) scene_root: Option<SceneRoot>,
    pub(crate) reflected: Option<std::sync::Arc<ComputeTable>>,
    pub(crate) raster_table: Option<std::sync::Arc<ComputeTable>>,
    pub(crate) null_usage: Option<std::sync::Arc<crate::diagnostics::null_usage::Tag>>,
    pub(crate) compute_threads: Option<[u32; 3]>,
}

const _: () = assert!(
    core::mem::offset_of!(Proxy<ID3D12PipelineState, PipelineData>, state)
        + core::mem::offset_of!(PipelineData, packets)
        == 0x10
);
// GetInformationX is slot 11. Two methods follow it. A shorter table would
// read off the end, so those calls fail closed.
pub(super) static PIPELINE: Slots<14> = {
    let mut slots = child_slots::<ID3D12PipelineState, PipelineData, 14>();
    slots[11] = crate::slot!(scratch_info);
    slots
};

// FidelityFX queries a blob for the console pipeline's scratch size. Desktop
// pipelines manage their own scratch. The caller skips the blob on failure.
unsafe extern "system" fn scratch_info(
    _this: &Proxy<ID3D12PipelineState, PipelineData>,
    _arg1: usize,
    _arg2: usize,
    blob: Out,
) -> HRESULT {
    if blob.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller provided storage for one COM interface pointer.
    unsafe { blob.write(core::ptr::null_mut()) };
    E_NOTIMPL
}

// A null root signature is valid for a shader with an embedded signature.
// The caller keeps every non-null guest object alive for the returned borrow.
pub(crate) unsafe fn root_proxy<'a>(
    guest: *mut c_void,
    device: *mut c_void,
) -> Result<Option<&'a Root>, HRESULT> {
    if guest.is_null() {
        return Ok(None);
    }
    // SAFETY: unwrap checks both the guest vtable and device identity.
    unsafe { unwrap::<ID3D12RootSignature, RootSource>(guest, device, &ROOT) }
        .map(Some)
        .ok_or(E_INVALIDARG)
}

pub(crate) unsafe fn root_signature<'a>(
    guest: *mut c_void,
    device: *mut c_void,
) -> Result<Option<&'a ID3D12RootSignature>, HRESULT> {
    unsafe { root_proxy(guest, device) }.map(|root| root.map(|p| &p.native))
}

pub(crate) unsafe fn pipeline<'a>(
    guest: *mut c_void,
    device: *mut c_void,
) -> Result<Option<&'a Proxy<ID3D12PipelineState, PipelineData>>, HRESULT> {
    if guest.is_null() {
        return Ok(None);
    }
    // SAFETY: as with root_signature, the caller owns the live guest reference.
    unsafe { unwrap::<ID3D12PipelineState, PipelineData>(guest, device, &PIPELINE) }
        .map(Some)
        .ok_or(E_INVALIDARG)
}

pub(super) unsafe extern "system" fn create_root_signature(
    this: &Device,
    node: u32,
    blob: *const u8,
    length: usize,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: the guest supplies a readable IID and writable output slot.
    let hr = unsafe { check_output::<ID3D12RootSignature>(iid, out) };
    if hr.is_err() {
        return hr;
    }
    if blob.is_null() || length == 0 || length > isize::MAX as usize {
        return E_INVALIDARG;
    }
    // SAFETY: serialized signature bytes are borrowed only for this call.
    let bytes = unsafe { slice::from_raw_parts(blob, length) };
    match unsafe {
        this.native
            .CreateRootSignature::<ID3D12RootSignature>(node, bytes)
    } {
        // SAFETY: the returned guest child owns both its native interface and device.
        Ok(native) => unsafe {
            allocate(
                &ROOT,
                &[ID3D12DeviceChild::IID, ID3D12RootSignature::IID],
                native,
                Some(&identity(this)),
                RootSource::new(bytes),
                out,
            )
        },
        Err(error) => error.code(),
    }
}

// Slot 67 receives a serialized console PSO. FidelityFX retries slot 62 with a
// shader descriptor if its precompiled ACO pipeline cannot be deserialized.
pub(super) unsafe extern "system" fn deserialize_pipeline_x(
    _this: &Device,
    _desc: *const c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: the guest supplies a readable IID and writable output slot.
    let hr = unsafe { check_output::<ID3D12PipelineState>(iid, out) };
    if hr.is_err() { hr } else { E_NOTIMPL }
}

// Slot 62 takes a compute descriptor followed by an array of Xbox extensions.
// Portable DXIL and a standard serialized root signature work with zero extensions.
pub(super) unsafe extern "system" fn create_pipeline_x(
    this: &Device,
    desc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
    extension_count: u32,
    _extensions: *const c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // The progress panel counts one pipeline per call from the title.
    let building = crate::progress::building();
    crate::failures::begin();
    // SAFETY: a non-null guest descriptor is readable for this call.
    if let Some(desc) = unsafe { desc.as_ref() } {
        crate::failures::stage("CS", desc.CS.pShaderBytecode.cast(), desc.CS.BytecodeLength);
    }
    // SAFETY: the arguments are passed through unchanged.
    let status = unsafe { create_pipeline_x_inner(this, desc, extension_count, _extensions, iid, out) };
    building.finish(status.is_ok());
    // SAFETY: the guest shaders noted above stay readable for this whole call.
    unsafe { super::record_pipeline("slot62", status) };
    status
}

unsafe fn create_pipeline_x_inner(
    this: &Device,
    desc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
    extension_count: u32,
    _extensions: *const c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: the guest supplies a readable IID and writable output slot.
    let hr = unsafe { check_output::<ID3D12PipelineState>(iid, out) };
    if hr.is_err() {
        return hr;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if extension_count != 0 || desc.CachedPSO.CachedBlobSizeInBytes != 0 {
        return E_NOTIMPL;
    }
    if desc.CS.pShaderBytecode.is_null()
        || desc.CS.BytecodeLength == 0
        || desc.CS.BytecodeLength > isize::MAX as usize
    {
        return E_INVALIDARG;
    }
    let guest = desc
        .pRootSignature
        .as_ref()
        .map_or(core::ptr::null_mut(), Interface::as_raw);
    // SAFETY: the descriptor borrows its root signature for pipeline creation.
    let root = match unsafe { root_proxy(guest, identity(this).as_raw()) } {
        Ok(root) => root,
        Err(error) => return error,
    };
    // SAFETY: the guest retains its shader bytes until this synchronous call returns.
    let bytes =
        unsafe { slice::from_raw_parts(desc.CS.pShaderBytecode.cast(), desc.CS.BytecodeLength) };
    use super::compute_table::{Program, TableLayout};
    let layout = match root.map(|r| super::compute_table::reflected(
        &this.native, &r.state, Program::Compute(bytes), desc.NodeMask,
    )).transpose() {
        Ok(layout) => layout,
        Err(error) => return error.code(),
    };
    let guest_threads = tuxbox_dxbc::shader_compute_threads(bytes);
    let (translation, reflected) = match layout {
        Some(TableLayout::Reflected(table)) => {
            // Embedded RTS0 cannot be replaced. The translator reads that original
            // root and fills only the slots the appended signature adds.
            let code = super::shader_cache::compute_with_root(bytes, table.layout.payload())
                .or_else(|| super::shader_cache::prepared(bytes));
            let Some(code) = code else {
                super::compute_table::capture_stage_root(bytes, &table.original);
                crate::diagnostics::compute_failure(bytes, E_NOTIMPL.0);
                return unsafe { null_compute(this, root.map(|p| &p.native), desc, guest_threads, out) }.unwrap_or(E_NOTIMPL);
            };
            (Some(code), Some(table))
        }
        Some(TableLayout::Original) => (root.and_then(|r| {
            super::shader_cache::compute_for_original_root(bytes, r.state.payload())
        }), None),
        _ => (super::shader_cache::prepared(bytes).or_else(|| {
            super::shader_cache::compute_with_root(bytes, root?.state.payload())
        }), None),
    };
    let compute_threads = guest_threads
        .or_else(|| translation.as_deref().and_then(tuxbox_dxbc::shader_compute_threads));
    let code = translation.as_deref().unwrap_or(bytes);
    if let Some(refusal) = unsupported_wave_size(this, code) {
        crate::diagnostics::compute_wave_failure(bytes, &refusal.detail);
        crate::failures::refusal("CS", &refusal.tag);
        return unsafe { null_compute(this, root.map(|p| &p.native), desc, compute_threads, out) }
            .unwrap_or(E_INVALIDARG);
    }
    let scene_root = if let Some(translated) = &translation {
        match super::compute_scene::private_root(
            &this.native,
            bytes,
            translated,
            root.map(|p| &p.state),
            desc.NodeMask,
        ) {
            Ok(root) => root,
            Err(e) => return e.code(),
        }
    } else {
        None
    };
    let label = super::shader_cache::source_digest(bytes)
        .map(|hash| format!("CS {}", &hash[..8]))
        .unwrap_or_else(|| "CS".to_string());
    let mut host = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(
            scene_root
                .as_ref()
                .map(|s| &s.native)
                .or_else(|| reflected.as_ref().map(|t| &t.native))
                .or(root.map(|p| &p.native))
                .cloned(),
        ),
        CS: D3D12_SHADER_BYTECODE {
            pShaderBytecode: code.as_ptr().cast(),
            BytecodeLength: code.len(),
        },
        NodeMask: desc.NodeMask,
        Flags: desc.Flags,
        ..Default::default()
    };
    // The driver compile must own its device, root signature, and shader bytes:
    // a deadline can return while that compile is still writing the GPU cache.
    let signature = core::mem::ManuallyDrop::into_inner(host.pRootSignature.clone());
    let compile = |shader: &[u8]| {
        let device = this.native.clone();
        let signature = signature.clone();
        let shader = shader.to_vec();
        let node_mask = desc.NodeMask;
        let flags = desc.Flags;
        let name = label.clone();
        crate::big_stack::run_deadline(&label, move || {
            // Large programs queue here so the driver does not compile them
            // all at once. The queue is on the detached compile thread, so the
            // title's own thread still returns at the deadline.
            let _slot = crate::compile_gate::large_compile(shader.len(), |slots| {
                crate::diagnostics::log(&format!("{name} waits for one of {slots} large compile slots"));
            });
            owned_compile(device, signature, shader, node_mask, flags)
        })
    };
    // Boot 174727 substituted null pipelines for translated shaders whose
    // driver compile passed 20s. Those null pipelines have no paged layout, so
    // the dispatch that precedes the account-picker draws was dropped.
    // Large translated programs are the exception: the driver can take longer
    // than any load will wait, so they get the deadline and a stand-in too.
    let mut native = if translation.is_some() && !crate::compile_gate::is_large(code.len()) {
        static WAIT: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&WAIT, || {
            "translated compute pipelines wait for the driver compile".to_string()
        });
        let device = this.native.clone();
        let signature = signature.clone();
        let shader = code.to_vec();
        let node_mask = desc.NodeMask;
        let flags = desc.Flags;
        crate::big_stack::run_logged(&label, move || {
            owned_compile(device, signature, shader, node_mask, flags)
        })
    } else {
        match compile(code) {
            crate::big_stack::PipelineWait::Ready(result) => result,
            crate::big_stack::PipelineWait::Deferred => {
                // The original guest shader would be a second long driver compile.
                unsafe { ManuallyDrop::drop(&mut host.pRootSignature) };
                return unsafe { null_compute(this, root.map(|p| &p.native), desc, compute_threads, out) }.unwrap_or(E_NOTIMPL);
            }
        }
    };
    if translation.is_some() && reflected.is_none() && scene_root.is_none() && native.is_err() {
        // Structural checks cannot validate DXIL instructions or root bindings.
        // Let the adapter handle the original guest shader if it rejects the cache.
        native = match compile(bytes) {
            crate::big_stack::PipelineWait::Ready(result) => result,
            crate::big_stack::PipelineWait::Deferred => {
                unsafe { ManuallyDrop::drop(&mut host.pRootSignature) };
                return unsafe { null_compute(this, root.map(|p| &p.native), desc, compute_threads, out) }.unwrap_or(E_NOTIMPL);
            }
        };
    }
    unsafe { ManuallyDrop::drop(&mut host.pRootSignature) };
    if let Err(error) = &native {
        if let Some(root) = root { super::compute_table::capture_stage_root(bytes, &root.state); }
        crate::diagnostics::compute_failure(bytes, error.code().0);
        if let Some(status) = unsafe { null_compute(this, root.map(|p| &p.native), desc, compute_threads, out) } {
            return status;
        }
    } else if let Some(table) = &reflected {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!(
            "created compute pipeline with {} reflected FSSM descriptor slots", table.lengths.iter().map(|&v| u64::from(v)).sum::<u64>()
        ));
    }
    unsafe {
        wrap_pageable(
            native,
            &PIPELINE,
            this,
            PipelineData {
                scene_root,
                reflected,
                compute_threads,
                ..Default::default()
            },
            out,
        )
    }
}

/// Compile one compute pipeline from copies the helper thread owns.
///
/// Compile a large translated program on a detached thread, a few at a time,
/// and stop waiting at the pipeline deadline. The compile owns everything it
/// reads and carries on, so the driver can cache its result.
pub(super) fn large_deferred(
    label: &str,
    device: ID3D12Device,
    root: Option<ID3D12RootSignature>,
    shader: Vec<u8>,
    node_mask: u32,
    flags: D3D12_PIPELINE_STATE_FLAGS,
) -> crate::big_stack::PipelineWait<windows::core::Result<ID3D12PipelineState>> {
    let name = label.to_owned();
    crate::big_stack::run_deadline(label, move || {
        let _slot = crate::compile_gate::large_compile(shader.len(), |slots| {
            crate::diagnostics::log(&format!("{name} waits for one of {slots} large compile slots"));
        });
        owned_compile(device, root, shader, node_mask, flags)
    })
}

/// The guest descriptor can be released as soon as the caller returns. A
/// deadline does that while the driver is still compiling, so the bytes and
/// the root signature in that descriptor must not be the ones the driver reads.
fn owned_compile(
    device: ID3D12Device,
    root: Option<ID3D12RootSignature>,
    shader: Vec<u8>,
    node_mask: u32,
    flags: D3D12_PIPELINE_STATE_FLAGS,
) -> windows::core::Result<ID3D12PipelineState> {
    let mut descriptor = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(root),
        CS: D3D12_SHADER_BYTECODE {
            pShaderBytecode: shader.as_ptr().cast(),
            BytecodeLength: shader.len(),
        },
        NodeMask: node_mask,
        Flags: flags,
        ..Default::default()
    };
    // SAFETY: `shader` and `descriptor.pRootSignature` stay alive until this
    // call returns. `device` is an AddRef held by this thread.
    let result = unsafe { device.CreateComputePipelineState(&descriptor) };
    unsafe { ManuallyDrop::drop(&mut descriptor.pRootSignature) };
    result
}

/// Create a resource-free compute pipeline on the guest root (see `fallback`).
unsafe fn null_compute(
    this: &Device, root: Option<&ID3D12RootSignature>, desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC,
    compute_threads: Option<[u32; 3]>, out: Out,
) -> Option<HRESULT> {
    use super::fallback::{Stage, bytecode, enabled, shader, substituted};
    if !enabled() { return None; }
    let mut host = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(root.cloned()),
        CS: bytecode(shader(Stage::Compute)?),
        NodeMask: desc.NodeMask,
        Flags: desc.Flags,
        ..Default::default()
    };
    let native = crate::big_stack::run_logged("null CS", || unsafe { this.native.CreateComputePipelineState::<ID3D12PipelineState>(&host) });
    unsafe { ManuallyDrop::drop(&mut host.pRootSignature) };
    if native.is_err() { return None; }
    let null_usage = crate::diagnostics::display::enabled().then(|| {
        let digest = if !desc.CS.pShaderBytecode.is_null() && desc.CS.BytecodeLength <= 64 * 1024 * 1024 {
            let code = unsafe { std::slice::from_raw_parts(desc.CS.pShaderBytecode.cast::<u8>(), desc.CS.BytecodeLength) };
            super::shader_cache::source_digest(code)
        } else { None };
        std::sync::Arc::new(crate::diagnostics::null_usage::Tag {
            kind: "compute", stages: format!("CS={}", digest.as_deref().unwrap_or("unknown")),
        })
    });
    substituted("compute");
    Some(unsafe { wrap_pageable(native, &PIPELINE, this, PipelineData { null_usage, compute_threads, ..Default::default() }, out) })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct WaveRefusal {
    pub(super) detail: String,
    pub(super) tag: String,
}

/// Returns `Some(WaveRefusal)` when `required` `[min, max]` has no overlap with `host`.
pub(super) fn check_wave_range(required: (u32, u32), host: (u32, u32)) -> Option<WaveRefusal> {
    let (req_min, req_max) = required;
    let (host_min, host_max) = host;
    if host_min == 0 || host_max < host_min || req_min > req_max {
        return None;
    }
    if req_min <= host_max && host_min <= req_max {
        return None;
    }
    let (detail, tag) = if req_min == req_max {
        (
            format!("host lacks wave{req_min} (supported wave range [{host_min}, {host_max}])"),
            format!("host-lacks-wave{req_min}"),
        )
    } else {
        (
            format!(
                "host wave range [{host_min}, {host_max}] excludes WaveSize [{req_min}, {req_max}]"
            ),
            format!("host-wave-range-{host_min}-{host_max}-excludes-{req_min}-{req_max}"),
        )
    };
    Some(WaveRefusal { detail, tag })
}

/// Returns a refusal if `shader` is portable DXIL whose required wave size is
/// unsupported by `device`'s `D3D12_FEATURE_D3D12_OPTIONS1` wave lane range.
pub(super) fn unsupported_wave_size(device: &Device, shader: &[u8]) -> Option<WaveRefusal> {
    let required = super::shader_cache::portable_wave_range(shader)?;
    let host = super::dxgi::wave_range(device)?;
    check_wave_range(required, host)
}

#[cfg(test)]
#[path = "../tests/device/compute.rs"]
mod tests;
