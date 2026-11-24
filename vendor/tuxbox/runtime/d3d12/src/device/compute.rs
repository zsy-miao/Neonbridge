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
    /// The null descriptor each unwritten table slot needs for these shaders.
    pub(crate) null_kinds: Option<std::sync::Arc<super::null_kinds::NullKinds>>,
    /// Filled by a background worker when this pipeline was created without
    /// its translation (see `stream::background`).
    pub(crate) upgrade: std::sync::OnceLock<std::sync::Arc<super::stream::Upgrade>>,
    /// What identifies this pipeline across runs (see `pipeline_cache`), and
    /// whether a command list has bound it yet.
    pub(crate) key: Option<std::sync::Arc<str>>,
    pub(crate) used: std::sync::atomic::AtomicBool,
    /// The native is a shared placeholder until a worker makes the pipeline.
    pub(crate) lazy: bool,
    /// A compute pipeline (slot 62): its dispatches wait for it rather than drop
    /// (see `wait_pending_compute`).
    pub(crate) compute: bool,
}

/// The host pipeline and state to bind: a worker's rebuild once it exists.
/// The flag is set while a lazy pipeline is still being made: the list
/// drops its draws and dispatches.
pub(crate) fn current(p: &Pipeline) -> (&ID3D12PipelineState, &PipelineData, bool) {
    if let Some(key) = &p.state.key
        && !p.state.used.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        super::pipeline_cache::note_used(key);
    }
    let Some(slot) = p.state.upgrade.get() else {
        return (&p.native, &p.state, false);
    };
    let built = if p.state.lazy { super::stream::background::ready(slot) } else { slot.get() };
    match built {
        Some(built) => {
            let p = built.pipeline();
            (&p.native, &p.state, false)
        }
        None => {
            if p.state.lazy && super::stream::background::failed(slot) {
                crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::BindsUntranslatable);
                super::stream::background::failed_bind(slot);
            }
            (&p.native, &p.state, p.state.lazy)
        }
    }
}

/// test105: the loading screen's Bink movie showed blocky, stale, colour-shifted
/// tiles (a datamosh) while every dispatch of its pipelines was dropped as
/// "pipeline still being made" for about 20 s: a decode step that is skipped
/// leaves the next frames building on a wrong reference. A compute pipeline
/// still being made at its bind is waited for, outside the frame's budget, up
/// to TUXBOX_COMPUTE_PENDING_WAIT_MS (3000; 0 drops its dispatches as before).
/// Returns whether the bind waited; logs each pipeline that waited (first 16,
/// then powers of two) with its key, the wait and the caller.
/// test111 hung ("no frame presented for 15 s"): title threads waited on 32+
/// pending compute pipelines one bind after another, each up to the full
/// limit. The waits of all binds share TUXBOX_COMPUTE_PENDING_FRAME_MS
/// (4000; 0 unlimited) per presented frame; past it a bind drops its dispatches
/// as before, the frame ends and the budget refills.
static BIND_WAIT_LEFT_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MAX);

fn bind_wait_frame_ms() -> i64 {
    static MS: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *MS.get_or_init(|| crate::env_toggle!("TUXBOX_COMPUTE_PENDING_FRAME_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(4000))
}

fn over_budget_ms() -> i64 {
    static MS: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *MS.get_or_init(|| crate::env_toggle!("TUXBOX_COMPUTE_OVER_BUDGET_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(750))
}

/// rebox 8af34bf (idea, re-derived): a draw into a display-sized target is
/// what the frame shows, so a graphics pipeline still being made when it is
/// bound with such a target waits up to TUXBOX_DISPLAY_DRAW_WAIT_MS (250; 0
/// drops its draws as before) outside the frame's budget instead.
pub(crate) fn wait_pending_display(p: &Pipeline) -> bool {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let limit = *MS.get_or_init(|| crate::env_toggle!("TUXBOX_DISPLAY_DRAW_WAIT_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(250));
    if limit == 0 || !p.state.lazy || p.state.compute {
        return false;
    }
    let Some(slot) = p.state.upgrade.get() else { return false };
    if super::stream::background::failed(slot) {
        return false;
    }
    let started = std::time::Instant::now();
    let built = super::stream::background::wait_for(slot, std::time::Duration::from_millis(limit)).is_some();
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        let key = p.state.key.as_deref().unwrap_or("-");
        crate::diagnostics::log_line(format_args!(
            "graphics pipeline {} still being made at a bind with a display-sized target: waited {:.1} ms, {} ({n} so far; TUXBOX_DISPLAY_DRAW_WAIT_MS=0 drops its draws)",
            &key[..key.len().min(16)], started.elapsed().as_secs_f64() * 1e3, if built { "made" } else { "not made, draws dropped" }));
    }
    true
}

pub(crate) fn refill_bind_wait() {
    let ms = bind_wait_frame_ms();
    BIND_WAIT_LEFT_MS.store(if ms == 0 { i64::MAX } else { ms }, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn wait_pending_compute(p: &Pipeline) -> bool {
    static LIMIT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let limit = *LIMIT.get_or_init(|| {
        crate::env_toggle!("TUXBOX_COMPUTE_PENDING_WAIT_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(3000)
    });
    // test106: 9186337 tested the key for "compute:", but the key is the digest
    // of that string, so no bind ever waited. The pipeline is marked instead.
    if limit == 0 || !p.state.lazy || !p.state.compute {
        return false;
    }
    let key = p.state.key.as_deref().unwrap_or("-");
    let Some(slot) = p.state.upgrade.get() else { return false };
    if super::stream::background::failed(slot) {
        return false;
    }
    use std::sync::atomic::Ordering as O;
    let frame_ms = bind_wait_frame_ms();
    if frame_ms != 0 && BIND_WAIT_LEFT_MS.load(O::Relaxed) > frame_ms {
        refill_bind_wait();
    }
    let mut left = BIND_WAIT_LEFT_MS.load(O::Relaxed);
    // test119 hung at the world switch: dispatches were dropped as "still
    // being made" with this frame's bind waits spent, and a GPU memory
    // wait later never saw its label (a dropped dispatch can be the one
    // that writes it). Every DXIL is cached now, so making one is the
    // driver's compile alone: past the frame budget a bind still makes
    // its pipeline on this thread (stolen from the queue), up to
    // TUXBOX_COMPUTE_OVER_BUDGET_MS (750; 0 drops as before).
    let over = over_budget_ms();
    if left <= 0 && over > 0 {
        static OVER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = OVER.fetch_add(1, O::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "compute pipeline {} still being made with this frame's bind waits spent: made at its bind anyway, up to {over} ms ({n} so far; TUXBOX_COMPUTE_OVER_BUDGET_MS=0 drops its dispatches)",
                &key[..key.len().min(16)]));
        }
        left = over;
    }
    if left <= 0 {
        static SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SKIPPED.fetch_add(1, O::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "compute pipeline {} still being made at its bind: not waited, this frame's bind waits are spent ({n} so far; TUXBOX_COMPUTE_PENDING_FRAME_MS)",
                &key[..key.len().min(16)]));
        }
        return false;
    }
    let started = std::time::Instant::now();
    let wait = limit.min(u64::try_from(left).unwrap_or(u64::MAX));
    let built = super::stream::background::wait_for(slot, std::time::Duration::from_millis(wait)).is_some();
    BIND_WAIT_LEFT_MS.fetch_sub(started.elapsed().as_millis() as i64, O::Relaxed);
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 16 || n.is_power_of_two() {
        let caller = match crate::survey::title_rva() {
            Some(rva) => format!("title RVA {rva:#x}"),
            None => "outside the title (a middleware module such as Bink)".into(),
        };
        crate::diagnostics::log_line(format_args!(
            "compute pipeline {} still being made at its bind: waited {:.1} ms, {} ({caller}; {n} so far; TUXBOX_COMPUTE_PENDING_WAIT_MS=0 drops its dispatches)",
            &key[..key.len().min(16)], started.elapsed().as_secs_f64() * 1e3,
            if built { "made" }
            else if super::stream::background::failed(slot) { "not made: its build failed (see the creation failure above), dispatches dropped" }
            else if wait < limit { "not made: this frame's bind waits ran out, dispatches dropped" }
            else { "not made in time, dispatches dropped" }));
    }
    true
}

const _: () = assert!(
    core::mem::offset_of!(Proxy<ID3D12PipelineState, PipelineData>, state)
        + core::mem::offset_of!(PipelineData, packets)
        == 0x10
);
// GetInformationX is slot 11. Two methods follow it (TuxBox aeed457): a
// shorter table would read off the end, so those calls fail closed.
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
    crate::diagnostics::rootsig::note(bytes);
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
/// Device slots 57 and 67 (desc, riid, ppv): a pipeline described by console
/// state blocks (Xbox2Windows d3d12_xs_device.cpp, MIT). The description's
/// shader stages are not decoded yet, so the title gets a placeholder
/// pipeline whose draws and dispatches command lists drop, S_OK instead of
/// E_NOTIMPL, only with TUXBOX_SLOT57=placeholder (default E_NOTIMPL). The first calls log 96 description bytes, dump 0x300 of them
/// to %TEMP%\tuxbox-slotN-K.bin, and decode state block A's blend words
/// (cb_blend). By default it answers E_NOTIMPL.
pub(super) unsafe extern "system" fn deserialize_pipeline_x<const N: u32>(
    this: &Device,
    desc: *const c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    // SAFETY: the guest supplies a readable IID and writable output slot.
    let hr = unsafe { check_output::<ID3D12PipelineState>(iid, out) };
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // E_NOTIMPL by default: the title then falls back to pipelines it draws
    // with. The placeholder (S_OK, draws dropped) left the picture black
    // (test38-45: clears only; test46 with the stub drew).
    // TUXBOX_SLOT57=placeholder answers the placeholder.
    let stub = crate::env_toggle!("TUXBOX_SLOT57") != Some("placeholder");
    if call < 2 {
        unsafe { describe_state_blocks(N, call, desc) };
    }
    if hr.is_err() {
        return hr;
    }
    if stub {
        return E_NOTIMPL;
    }
    let Some(placeholder) = super::stream::background::placeholder(this) else {
        return E_NOTIMPL;
    };
    let state = PipelineData { lazy: true, ..Default::default() };
    let _ = state.upgrade.set(super::stream::background::never());
    if call < 4 {
        crate::diagnostics::log_line(format_args!(
            "device slot {N} pipeline from state blocks at title RVA {:#x}: placeholder (its draws are dropped; TUXBOX_SLOT57=placeholder chose it)",
            crate::survey::title_rva().unwrap_or(0)));
    }
    unsafe { wrap_pageable(Ok(placeholder), &PIPELINE, this, state, out) }
}

/// Whether [address, address + bytes) is committed readable memory.
fn readable(address: usize, bytes: usize) -> bool {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
    let Some(end) = address.checked_add(bytes) else { return false };
    let mut at = address;
    while at < end {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if address == 0 || unsafe { VirtualQuery(Some(at as _), &mut region, size_of_val(&region)) } == 0
            || region.State != MEM_COMMIT || region.Protect.0 & (PAGE_NOACCESS.0 | PAGE_GUARD.0) != 0 || region.Protect.0 == 0
        {
            return false;
        }
        at = region.BaseAddress as usize + region.RegionSize;
    }
    true
}

/// Logs and dumps one slot 57/67 description and its state block A blend.
unsafe fn describe_state_blocks(slot: u32, call: u32, desc: *const c_void) {
    let address = desc as usize;
    let have = [0x300usize, 0x60, 0x28].into_iter().find(|&n| readable(address, n)).unwrap_or(0);
    let bytes = if have == 0 { &[][..] } else { unsafe { slice::from_raw_parts(desc.cast::<u8>(), have) } };
    let hex: String = bytes.iter().take(96).map(|b| format!("{b:02x} ")).collect();
    let dumped = (have == 0x300).then(|| {
        let path = std::env::temp_dir().join(format!("tuxbox-slot{slot}-{call}.bin"));
        std::fs::write(&path, bytes).ok().map(|()| path.display().to_string())
    }).flatten();
    // words[1], words[2]: state block A and its size.
    let blend = (bytes.len() >= 0x28).then(|| {
        let word = |n: usize| u64::from_le_bytes(bytes[n * 8..n * 8 + 8].try_into().unwrap()) as usize;
        let (block, size) = (word(1), word(2));
        if size == 0 || size > 0x1000 || !readable(block, size) {
            return format!("state block A {block:#x}+{size:#x} unreadable");
        }
        let block = unsafe { slice::from_raw_parts(block as *const u8, size) };
        if !super::cb_blend::enabled() {
            return "blend decode off (TUXBOX_SLOT57_CB_BLEND=0)".into();
        }
        match super::cb_blend::state_block_blend(block) {
            Ok(blend) => format!("blend enabled on targets {:?}",
                (0..8).filter(|&n| blend.RenderTarget[n].BlendEnable.as_bool()).collect::<Vec<_>>()),
            Err((target, reason)) => format!("blend refused on target {target}: {reason}; unblended"),
        }
    }).unwrap_or_else(|| "no state block words".into());
    crate::diagnostics::log_line(format_args!(
        "device slot {slot} call {call}: desc {address:#x} [{hex}] ({have} bytes readable, dump {}); {blend}",
        dumped.as_deref().unwrap_or("none")));
}

/// What identifies a guest compute pipeline across runs (see `pipeline_cache`).
unsafe fn compute_key(this: &Device, desc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC) -> Option<String> {
    let desc = unsafe { desc.as_ref() }?;
    if desc.CS.pShaderBytecode.is_null() || desc.CS.BytecodeLength == 0 || desc.CS.BytecodeLength > isize::MAX as usize {
        return None;
    }
    let bytes = unsafe { slice::from_raw_parts(desc.CS.pShaderBytecode.cast::<u8>(), desc.CS.BytecodeLength) };
    let guest = desc.pRootSignature.as_ref().map_or(core::ptr::null_mut(), Interface::as_raw);
    let root = unsafe { root_proxy(guest, identity(this).as_raw()) }.ok()?;
    let root = match root {
        Some(root) => super::shader_cache::source_digest(root.state.payload())?,
        None => "-".into(),
    };
    let shader = super::shader_cache::source_digest(bytes)?;
    let canonical = format!("compute:{shader};{root};{};{}", desc.NodeMask, desc.Flags.0);
    super::shader_cache::source_digest(canonical.as_bytes())
}

// Slot 62 takes a compute descriptor followed by an array of Xbox extensions.
// Portable DXIL and a standard serialized root signature work with zero extensions.
pub(super) unsafe extern "system" fn create_pipeline_x(
    this: &Device,
    desc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
    extension_count: u32,
    extensions: *const c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    use super::stream::background;
    let worker = background::on_worker();
    let key = if worker { None } else { unsafe { compute_key(this, desc) } };
    if let Some(key) = key.as_deref()
        && crate::diagnostics::capture::shaders_on()
        && let Some(guest) = unsafe { desc.as_ref() }
        && !guest.CS.pShaderBytecode.is_null()
        && guest.CS.BytecodeLength != 0
    {
        let bytes = unsafe { slice::from_raw_parts(guest.CS.pShaderBytecode.cast::<u8>(), guest.CS.BytecodeLength) };
        crate::diagnostics::capture::pipeline_shaders(key, super::shader_cache::source_digest(bytes).into_iter().collect());
    }
    // As for streams: a pipeline no earlier run bound is made on first bind,
    // one an earlier run bound is handed to the workers now.
    let used_before = key.as_deref().is_some_and(|key| super::pipeline_cache::used_before(key) == Some(true));
    if !worker
        && (!used_before || !background::eager_on_title())
        && extension_count == 0
        && let Some(guest) = unsafe { desc.as_ref() }
        && guest.CachedPSO.CachedBlobSizeInBytes == 0
        && !guest.CS.pShaderBytecode.is_null()
        && guest.CS.BytecodeLength != 0
        && guest.CS.BytecodeLength <= isize::MAX as usize
        && unsafe { check_output::<ID3D12PipelineState>(iid, out) }.is_ok()
        && let Some(placeholder) = background::placeholder(this)
    {
        let bytes = unsafe { slice::from_raw_parts(guest.CS.pShaderBytecode.cast::<u8>(), guest.CS.BytecodeLength) };
        let work = background::Work::Compute(background::OwnedCompute::copy(guest, bytes));
        let state = PipelineData { key: key.map(Into::into), lazy: true, compute: true, ..Default::default() };
        let owner = (*identity(this)).clone();
        let _ = state.upgrade.set(if used_before { background::queue(owner, work) } else { background::lazy(owner, work) });
        return unsafe { wrap_pageable(Ok(placeholder), &PIPELINE, this, state, out) };
    }
    let _ = super::shader_cache::take_missed();
    let _ = super::shader_cache::take_misses();
    let status = unsafe { make_compute(this, desc, extension_count, extensions, iid, out) };
    if status.is_err() {
        crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::PipelinesRefused);
        // leonida-ce 83bf1f60: no null record for the title to walk (see background::refused).
        if !worker
            && background::refused_placeholder_on()
            && unsafe { check_output::<ID3D12PipelineState>(iid, out) }.is_ok()
            && let Some(placeholder) = background::placeholder(this)
        {
            let state = PipelineData { key: key.map(Into::into), lazy: true, compute: true, ..Default::default() };
            let _ = state.upgrade.set(background::refused("compute pipeline", status));
            return unsafe { wrap_pageable(Ok(placeholder), &PIPELINE, this, state, out) };
        }
    }
    let missed = super::shader_cache::take_missed();
    if status.is_err() || worker {
        return status;
    }
    // SAFETY: success wrote a new pipeline proxy that the guest has not seen yet.
    let Some(pipeline) = (unsafe { (*out).cast::<Pipeline>().as_mut() }) else {
        return status;
    };
    pipeline.state.key = key.map(Into::into);
    // A translation missed the wait budget: a worker makes the real pipeline.
    if missed && let Some(desc) = unsafe { desc.as_ref() } {
        let bytes = unsafe { slice::from_raw_parts(desc.CS.pShaderBytecode.cast::<u8>(), desc.CS.BytecodeLength) };
        let work = background::Work::Compute(background::OwnedCompute::copy(desc, bytes));
        let _ = pipeline.state.upgrade.set(background::queue((*identity(this)).clone(), work));
        // test110: such a pipeline was not marked lazy, so its binds never
        // waited for the worker and dropped their dispatches (and label
        // writes). Now it is waited on as a lazy one is
        // (TUXBOX_COMPUTE_PENDING_WAIT_MS).
        pipeline.state.lazy = true;
        pipeline.state.compute = true;
    }
    status
}

unsafe fn make_compute(
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
    let (translation, reflected) = match layout {
        Some(TableLayout::Reflected(table)) => {
            // Embedded RTS0 cannot be replaced. The translator reads that original
            // root and fills only the slots the appended signature adds.
            let code = super::shader_cache::compute_with_root(bytes, table.layout.payload())
                .or_else(|| super::shader_cache::prepared(bytes));
            let Some(code) = code else {
                super::compute_table::capture_stage_root(bytes, &table.original);
                // A miss on the title's thread goes to the workers (create_pipeline),
                // so it is not final; say so rather than look like a failure.
                let why = super::shader_cache::take_reason();
                let why = if super::shader_cache::missed() { format!("{why}; handed to the pipeline workers, not final") } else { why.to_string() };
                crate::diagnostics::compute_failure(bytes, E_NOTIMPL.0, &why);
                return unsafe { null_compute(this, root.map(|p| &p.native), desc, out) }.unwrap_or(E_NOTIMPL);
            };
            (Some(code), Some(table))
        }
        Some(TableLayout::Original) => (root.and_then(|r| {
            super::shader_cache::compute_with_root(bytes, r.state.payload())
        }), None),
        _ => (super::shader_cache::prepared(bytes).or_else(|| {
            super::shader_cache::compute_with_root(bytes, root?.state.payload())
        }), None),
    };
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
    // Literal samplers the translation looks up in tb_desc_table: reserve
    // their host samplers and add the table's root SRV.
    // A stage that reads typed buffer formats gets the view_formats root SRV.
    let reflected = match (&translation, root) {
        (Some(code), Some(root)) if scene_root.is_none()
            && (super::desc_table::declared(code) || super::view_formats::declared(code)) =>
        {
            let samplers = super::desc_table::declared(code);
            if samplers {
                this.state.desc_table.literals(&this.native, &tuxbox_dxbc::inline_sampler::literal_words(bytes));
            }
            match super::compute_table::with_runtime_tables(
                &this.native, reflected.as_ref(), &root.state, desc.NodeMask, samplers, super::view_formats::declared(code),
            ) {
                Ok(table) => Some(table),
                Err(error) => return error.code(),
            }
        }
        _ => reflected,
    };
    let code = translation.as_deref().unwrap_or(bytes);
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
    // SAFETY: native creation consumes the descriptor synchronously. The owned
    // clone in the C layout must be released explicitly on success and failure.
    let create = |descriptor: &D3D12_COMPUTE_PIPELINE_STATE_DESC| unsafe {
        super::pipeline_cache::compute(&this.native, descriptor)
    };
    let mut native = create(&host);
    if translation.is_some() && reflected.is_none() && scene_root.is_none() && native.is_err() {
        // Structural checks cannot validate DXIL instructions or root bindings.
        // Let the adapter handle the original guest shader if it rejects the cache.
        host.CS = desc.CS;
        native = create(&host);
    }
    unsafe { ManuallyDrop::drop(&mut host.pRootSignature) };
    if let Some(table) = reflected.as_ref().filter(|t| t.lengths.is_empty() && t.index_map.is_some()) {
        // 6ixpack: heap-indexed compute pipelines on the generated index-map root.
        use std::sync::atomic::{AtomicU32, Ordering};
        static MADE: AtomicU32 = AtomicU32::new(0);
        static FAILED: AtomicU32 = AtomicU32::new(0);
        let _ = table;
        let (counter, what) = if native.is_ok() { (&MADE, "created") } else { (&FAILED, "failed") };
        let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            let code = native.as_ref().err().map(|e| e.code().0).unwrap_or(0);
            crate::diagnostics::log_once(&std::sync::Once::new(), || format!("heap compute pipeline {what} ({n} so far, last {code:#x})"));
        }
    }
    if let Err(error) = &native {
        if let Some(root) = root { super::compute_table::capture_stage_root(bytes, &root.state); }
        crate::diagnostics::compute_failure(bytes, error.code().0, "host CreateComputePipelineState rejected it");
        if let Some(status) = unsafe { null_compute(this, root.map(|p| &p.native), desc, out) } {
            return status;
        }
    } else if let Some(table) = &reflected {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!(
            "created compute pipeline with {} reflected FSSM descriptor slots", table.lengths.iter().map(|&v| u64::from(v)).sum::<u64>()
        ));
    }
    // Registers of the translation follow the root it was made for.
    let null_kinds = translation.as_deref().and_then(|code| {
        let root = reflected.as_ref().map(|t| &t.layout).or(root.map(|p| &p.state))?;
        super::null_kinds::reflect(root.payload(), [code])
    });
    unsafe {
        wrap_pageable(
            native,
            &PIPELINE,
            this,
            PipelineData {
                scene_root,
                reflected,
                null_kinds,
                ..Default::default()
            },
            out,
        )
    }
}

/// Create a resource-free compute pipeline on the guest root (see `fallback`).
unsafe fn null_compute(
    this: &Device, root: Option<&ID3D12RootSignature>, desc: &D3D12_COMPUTE_PIPELINE_STATE_DESC, out: Out,
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
    let native = crate::big_stack::run(|| unsafe { this.native.CreateComputePipelineState::<ID3D12PipelineState>(&host) });
    unsafe { ManuallyDrop::drop(&mut host.pRootSignature) };
    if native.is_err() { return None; }
    if let Ok(pipeline) = &native {
        let code = unsafe { std::slice::from_raw_parts(desc.CS.pShaderBytecode.cast::<u8>(), desc.CS.BytecodeLength) };
        if crate::diagnostics::display::enabled() && let Some(digest) = super::shader_cache::source_digest(code) {
            crate::diagnostics::display::mark_null_compute(pipeline.as_raw(), digest);
        }
    }
    substituted("compute");
    super::stream::background::mark_nulled();
    Some(unsafe { wrap_pageable(native, &PIPELINE, this, PipelineData::default(), out) })
}
