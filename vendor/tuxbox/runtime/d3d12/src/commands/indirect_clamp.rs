//! Indirect execution's arguments, bounded on the GPU just before the
//! execution reads them (VIctory's argument clamp). The title's own compute
//! passes write most records; a pass TuxBox drops or translates wrongly
//! leaves whatever the memory held, and a draw of four billion vertices or a
//! dispatch past the host's limits runs until the driver resets the device.
//!
//! Each recording also has a budget (2^28 vertex instances or threads, mesh
//! groups counted as 128): records past it run empty, so thousands of
//! garbage records cannot run for seconds either. With a count buffer only
//! the counted records are touched.
//!
//! The clamp writes the title's buffer, so it runs only for default-heap
//! buffers that allow unordered access; the guest has them in the indirect
//! argument state, as D3D12 requires.
//!
//! shortcut: the bounds (2^24 vertices, 2^26 vertex instances, 65,535 groups
//! per dimension, one-dimensional mesh dispatches, the budget) cut legitimate
//! work beyond them; raise them if a title needs more.
use core::{ffi::c_void, mem::ManuallyDrop, slice};
use std::sync::OnceLock;
use windows::Win32::Foundation::{E_NOTIMPL, FreeLibrary};
use windows::Win32::Graphics::{Direct3D::ID3DBlob, Direct3D12::*, Dxgi::Common::DXGI_SAMPLE_DESC};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::core::{HRESULT, Interface, Result, s, w};

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), UAV(u0), UAV(u1), UAV(u2)"
cbuffer Record : register(b0) { uint kind; uint stride; uint records; uint counted; };
RWByteAddressBuffer args : register(u0);
// The recording's work so far (vertex instances, threads).
RWByteAddressBuffer budget : register(u1);
RWByteAddressBuffer count : register(u2);
static const uint BUDGET = 1u << 28;
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (kind == 4) {
        // A new recording's budget.
        if (id.x == 0) budget.Store(0, 0);
        return;
    }
    uint limit = counted != 0 ? min(records, count.Load(0)) : records;
    if (id.x >= limit) return;
    uint at = id.x * stride;
    uint cost;
    if (kind <= 1) {
        // Draws: vertex or index count, then instances.
        uint n = min(args.Load(at), 1u << 24);
        uint instances = min(args.Load(at + 4), (1u << 26) / max(n, 1u));
        cost = n * instances;
        args.Store2(at, uint2(n, instances));
    } else if (kind == 2) {
        uint3 g = min(args.Load3(at), uint3(65535, 65535, 65535));
        cost = min(g.x * g.y * g.z, 1u << 20) * 64;
        args.Store3(at, g);
    } else {
        // Mesh dispatches: the console's are one-dimensional.
        uint3 g = args.Load3(at);
        g = uint3(min(g.x, 65535u), min(g.y, 1u), min(g.z, 1u));
        cost = g.x * g.y * g.z * 128;
        args.Store3(at, g);
    }
    uint before;
    budget.InterlockedAdd(0, cost, before);
    if (before >= BUDGET || before + cost > BUDGET) {
        // Draws keep their layout with no vertices; groups become zero.
        args.Store(at, 0);
    }
}
"#;

struct Clamp {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Clamp {}
// SAFETY: as above.
unsafe impl Sync for Clamp {}

fn compile() -> Result<Vec<u8>> {
    type Compile = unsafe extern "system" fn(
        *const c_void, usize, *const u8, *const c_void, *const c_void,
        *const u8, *const u8, u32, u32, *mut *mut c_void, *mut *mut c_void,
    ) -> HRESULT;
    let library = unsafe { LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)? };
    let result = (|| unsafe {
        let entry = GetProcAddress(library, s!("D3DCompile")).ok_or(E_NOTIMPL)?;
        let compile: Compile = core::mem::transmute(entry);
        let mut raw = core::ptr::null_mut();
        compile(
            SOURCE.as_ptr().cast(), SOURCE.len(), s!("clamp").as_ptr(), core::ptr::null(), core::ptr::null(),
            s!("main").as_ptr(), s!("cs_5_1").as_ptr(), 0, 0, &mut raw, core::ptr::null_mut(),
        )
        .ok()?;
        let blob = ID3DBlob::from_raw(raw);
        Ok(slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()).to_vec())
    })();
    let _ = unsafe { FreeLibrary(library) };
    result
}

fn make(device: &ID3D12Device) -> Result<Clamp> {
    let code = compile()?;
    // The root signature is inside the shader.
    let root: ID3D12RootSignature = unsafe { device.CreateRootSignature(0, &code)? };
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
        ..Default::default()
    };
    let pipeline = unsafe { device.CreateComputePipelineState(&desc) };
    unsafe { ManuallyDrop::drop(&mut desc.pRootSignature) };
    Ok(Clamp { root, pipeline: pipeline? })
}

fn clamp(device: &ID3D12Device) -> Option<&'static Clamp> {
    static CLAMP: OnceLock<Option<Clamp>> = OnceLock::new();
    CLAMP
        .get_or_init(|| match make(device) {
            Ok(clamp) => Some(clamp),
            Err(error) => {
                crate::diagnostics::log_once(&std::sync::Once::new(), || {
                    format!("no indirect argument clamp: {error}")
                });
                None
            }
        })
        .as_ref()
}

fn budget_buffer(device: &ID3D12Device) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: 256,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    unsafe {
        device.CreateCommittedResource(
            &heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, None, &mut buffer,
        )?
    };
    buffer.ok_or(E_NOTIMPL.into())
}

/// Whether the clamp may write this buffer in place.
fn writable(buffer: &ID3D12Resource) -> bool {
    let desc = unsafe { buffer.GetDesc() };
    if !desc.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
        return false;
    }
    let mut heap = D3D12_HEAP_PROPERTIES::default();
    // Placed and reserved buffers have no properties of their own; the
    // unordered access flag already rules out upload and readback heaps.
    unsafe { buffer.GetHeapProperties(Some(&mut heap), None) }.is_err()
        || heap.Type == D3D12_HEAP_TYPE_DEFAULT
}

pub(super) fn transition(resource: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { core::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER { pResource: ManuallyDrop::new(None) }),
        },
    }
}

/// The host kind number the shader uses, or `None` for records it leaves alone.
fn shader_kind(kind: D3D12_INDIRECT_ARGUMENT_TYPE) -> Option<u32> {
    match kind {
        D3D12_INDIRECT_ARGUMENT_TYPE_DRAW => Some(0),
        D3D12_INDIRECT_ARGUMENT_TYPE_DRAW_INDEXED => Some(1),
        D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH => Some(2),
        D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH => Some(3),
        _ => None,
    }
}

/// An argument or count buffer and the byte offset into it.
pub(super) struct Region<'a> {
    pub(super) buffer: &'a ID3D12Resource,
    pub(super) offset: u64,
}

/// Records, in `list`, the clamp of `records` records of `kind`, `stride`
/// bytes apart. Replaces the list's compute root signature and pipeline: the
/// caller restores its bindings after, when this returns true. `budget` is
/// the recording's budget buffer, made on first use. Returns whether the
/// clamp ran and the budget buffer it made, which the recording must keep
/// alive through execution.
pub(super) fn bound(
    list: &ID3D12GraphicsCommandList,
    budget: &mut Option<ID3D12Resource>,
    kind: D3D12_INDIRECT_ARGUMENT_TYPE,
    stride: u32,
    records: u32,
    arguments: Region,
    count: Option<Region>,
) -> Result<(bool, Option<ID3D12Resource>)> {
    let Some(code) = shader_kind(kind) else { return Ok((false, None)) };
    if records == 0
        || !stride.is_multiple_of(4)
        || !writable(arguments.buffer)
        || count.as_ref().is_some_and(|c| !writable(c.buffer))
    {
        return Ok((false, None));
    }
    let device: ID3D12Device = unsafe {
        let mut device: Option<ID3D12Device> = None;
        list.GetDevice(&mut device)?;
        device.ok_or(E_NOTIMPL)?
    };
    let Some(clamp) = clamp(&device) else { return Ok((false, None)) };
    let made = match budget {
        Some(_) => None,
        None => Some(budget_buffer(&device)?),
    };
    let fresh = made.is_some();
    if let Some(made) = &made {
        *budget = Some(made.clone());
    }
    let budget = budget.as_ref().expect("budget set above");
    let address = unsafe { arguments.buffer.GetGPUVirtualAddress() } + arguments.offset;
    let same = count.as_ref().is_some_and(|c| c.buffer == arguments.buffer);
    let count_address = count.as_ref().map_or(address, |c| unsafe { c.buffer.GetGPUVirtualAddress() } + c.offset);
    let mut into = vec![transition(arguments.buffer, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)];
    let mut back = vec![uav_barrier(), transition(arguments.buffer, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT)];
    if let Some(count) = count.as_ref().filter(|_| !same) {
        into.push(transition(count.buffer, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS));
        back.push(transition(count.buffer, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT));
    }
    unsafe {
        list.ResourceBarrier(&into);
        list.SetComputeRootSignature(&clamp.root);
        list.SetPipelineState(&clamp.pipeline);
        let budget_address = budget.GetGPUVirtualAddress();
        if fresh {
            let constants = [4u32, 0, 0, 0];
            list.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
            list.SetComputeRootUnorderedAccessView(1, address);
            list.SetComputeRootUnorderedAccessView(2, budget_address);
            list.SetComputeRootUnorderedAccessView(3, count_address);
            list.Dispatch(1, 1, 1);
            list.ResourceBarrier(&[uav_barrier()]);
        }
        let constants = [code, stride, records, u32::from(count.is_some())];
        list.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
        list.SetComputeRootUnorderedAccessView(1, address);
        list.SetComputeRootUnorderedAccessView(2, budget_address);
        list.SetComputeRootUnorderedAccessView(3, count_address);
        list.Dispatch(records.div_ceil(64), 1, 1);
        list.ResourceBarrier(&back);
    }
    // The barriers borrow their resources (no reference was taken).
    core::mem::forget((into, back));
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!("indirect arguments clamped (kind {})", kind.0));
    Ok((true, made))
}

/// A recording's clamp scratch: the clamped copies of its ExecuteIndirect
/// arguments (VIctory-fxceless indirect.rs `bound`/`record_clamp`, GPL-3.0,
/// credit to its authors). The clamp above writes the title's own buffer, so
/// records it emptied (over the budget, or garbage) stay empty for every later
/// frame that reuses them without rewriting: the world vanishing over time.
/// Here the records (and the count) are copied into the scratch, clamped
/// there and executed from it; the title's buffers are only read.
/// TUXBOX_INDIRECT_SCRATCH=0 clamps in place as before.
pub(super) struct Scratch {
    buffer: ID3D12Resource,
    used: u64,
}

pub(super) fn scratch_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TUXBOX_INDIRECT_SCRATCH").is_none_or(|v| v != "0"))
}

/// Where an ExecuteIndirect reads its clamped arguments and count.
pub(super) struct Clamped {
    pub(super) buffer: ID3D12Resource,
    pub(super) arguments: u64,
    pub(super) count: Option<u64>,
}

fn scratch_buffer(device: &ID3D12Device, bytes: u64) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    unsafe {
        device.CreateCommittedResource(
            &heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, None, &mut buffer,
        )?
    };
    buffer.ok_or(E_NOTIMPL.into())
}

/// Whether the copy may read this buffer: anything but upload and readback
/// heaps, whose state cannot leave GENERIC_READ / COPY_DEST.
fn readable(buffer: &ID3D12Resource) -> bool {
    let mut heap = D3D12_HEAP_PROPERTIES::default();
    unsafe { buffer.GetHeapProperties(Some(&mut heap), None) }.is_err()
        || heap.Type == D3D12_HEAP_TYPE_DEFAULT
}

/// [`bound`] into the recording's scratch. Returns where to execute from
/// (`None`: not clamped, execute the title's buffers) and the buffers made,
/// which the recording must keep alive through execution.
#[allow(clippy::too_many_arguments)]
pub(super) fn bound_scratch(
    list: &ID3D12GraphicsCommandList,
    budget: &mut Option<ID3D12Resource>,
    scratch: &mut Option<Scratch>,
    kind: D3D12_INDIRECT_ARGUMENT_TYPE,
    stride: u32,
    records: u32,
    arguments: Region,
    count: Option<Region>,
) -> Result<(Option<Clamped>, Vec<ID3D12Resource>)> {
    let Some(code) = shader_kind(kind) else { return Ok((None, Vec::new())) };
    if records == 0
        || !stride.is_multiple_of(4)
        || !readable(arguments.buffer)
        || count.as_ref().is_some_and(|c| !readable(c.buffer))
    {
        return Ok((None, Vec::new()));
    }
    let device: ID3D12Device = unsafe {
        let mut device: Option<ID3D12Device> = None;
        list.GetDevice(&mut device)?;
        device.ok_or(E_NOTIMPL)?
    };
    let Some(clamp) = clamp(&device) else { return Ok((None, Vec::new())) };
    let mut made = Vec::new();
    let fresh = budget.is_none();
    if fresh {
        let b = budget_buffer(&device)?;
        made.push(b.clone());
        *budget = Some(b);
    }
    let budget = budget.as_ref().expect("budget set above");
    let bytes = u64::from(stride) * u64::from(records);
    // Arguments, then the count, each 16-byte aligned.
    let need = bytes.next_multiple_of(16) + 16;
    if scratch.as_ref().is_none_or(|s| s.used + need > unsafe { s.buffer.GetDesc() }.Width) {
        let grown = scratch.as_ref().map_or(0, |s| unsafe { s.buffer.GetDesc() }.Width * 2);
        let buffer = scratch_buffer(&device, need.max(grown).max(1 << 20))?;
        made.push(buffer.clone());
        *scratch = Some(Scratch { buffer, used: 0 });
    }
    let s = scratch.as_mut().expect("scratch set above");
    let (dst, at) = (s.buffer.clone(), s.used);
    let count_at = at + bytes.next_multiple_of(16);
    s.used += need;
    let same = count.as_ref().is_some_and(|c| c.buffer == arguments.buffer);
    let mut into = vec![
        transition(&dst, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_COPY_DEST),
        transition(arguments.buffer, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_COPY_SOURCE),
    ];
    let mut copied = vec![
        transition(&dst, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS),
        transition(arguments.buffer, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT),
    ];
    if let Some(c) = count.as_ref().filter(|_| !same) {
        into.push(transition(c.buffer, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_COPY_SOURCE));
        copied.push(transition(c.buffer, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT));
    }
    let back = [uav_barrier(), transition(&dst, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT)];
    let base = unsafe { dst.GetGPUVirtualAddress() };
    unsafe {
        list.ResourceBarrier(&into);
        list.CopyBufferRegion(&dst, at, arguments.buffer, arguments.offset, bytes);
        if let Some(c) = &count {
            list.CopyBufferRegion(&dst, count_at, c.buffer, c.offset, 4);
        }
        list.ResourceBarrier(&copied);
        list.SetComputeRootSignature(&clamp.root);
        list.SetPipelineState(&clamp.pipeline);
        let budget_address = budget.GetGPUVirtualAddress();
        if fresh {
            let constants = [4u32, 0, 0, 0];
            list.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
            list.SetComputeRootUnorderedAccessView(1, base + at);
            list.SetComputeRootUnorderedAccessView(2, budget_address);
            list.SetComputeRootUnorderedAccessView(3, base + count_at);
            list.Dispatch(1, 1, 1);
            list.ResourceBarrier(&[uav_barrier()]);
        }
        let constants = [code, stride, records, u32::from(count.is_some())];
        list.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
        list.SetComputeRootUnorderedAccessView(1, base + at);
        list.SetComputeRootUnorderedAccessView(2, budget_address);
        list.SetComputeRootUnorderedAccessView(3, base + count_at);
        list.Dispatch(records.div_ceil(64), 1, 1);
        list.ResourceBarrier(&back);
    }
    // The barriers borrow their resources (no reference was taken).
    core::mem::forget((into, copied, back));
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!("indirect arguments clamped into scratch (kind {})", kind.0));
    Ok((Some(Clamped { buffer: dst, arguments: at, count: count.is_some().then_some(count_at) }), made))
}
