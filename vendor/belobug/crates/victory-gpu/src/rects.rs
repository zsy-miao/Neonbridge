//! Rectangle lists for indirect draws. The console draws a rectangle from
//! three vertices; the host has no such primitive, so direct draws expand
//! each into two triangles on the CPU ([`crate::primitive::expand_rects`]).
//! An indirect draw's vertex range is only known on the GPU, so here a
//! compute shader of ours (HLSL compiled by the system's compiler, like the
//! argument clamp) expands every rectangle of each vertex buffer the draw
//! reads, and rewrites the draw records for the expanded buffers: vertex
//! count and first vertex times two (6 vertices for every 3).
//!
//! The fourth corner is v0 + v2 - v1 for the vertex formats the CPU
//! expansion knows (32- and 16-bit floats, 16- and 8-bit unorm); other
//! elements keep the first vertex's values.
//!
//! shortcut: the whole buffer is expanded for every draw (twice its size in
//! a buffer the list keeps), and vertex strides, element offsets and the
//! buffer's address must be multiples of 4 bytes; other draws are dropped.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

use crate::host::{Host, HostList, host, transition};

/// Elements one expansion handles.
pub(crate) const ELEMENTS: usize = 12;

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=16, b0), UAV(u0), UAV(u1)"
cbuffer C : register(b0) {
    // 0: expand vertices; 1: rewrite draw records.
    uint mode;
    // Vertex stride (mode 0) or record stride (mode 1), bytes.
    uint stride;
    // Rectangles (mode 0) or records (mode 1).
    uint count;
    uint elements;
    // offset | DXGI format << 16, per element.
    uint4 e[3];
};
RWByteAddressBuffer src : register(u0);
RWByteAddressBuffer dst : register(u1);

uint element(uint i) { return e[i / 4][i % 4]; }

// Component count and bytes of the formats rectangles use (as the CPU
// expansion): kind 0 f32, 1 f16, 2 unorm16, 3 unorm8.
void format(uint f, out uint n, out uint kind) {
    n = 0; kind = 0;
    if (f == 2) { n = 4; kind = 0; }
    else if (f == 6) { n = 3; kind = 0; }
    else if (f == 16) { n = 2; kind = 0; }
    else if (f == 41) { n = 1; kind = 0; }
    else if (f == 10) { n = 4; kind = 1; }
    else if (f == 34) { n = 2; kind = 1; }
    else if (f == 54) { n = 1; kind = 1; }
    else if (f == 11) { n = 4; kind = 2; }
    else if (f == 35) { n = 2; kind = 2; }
    else if (f == 28 || f == 29 || f == 87 || f == 91) { n = 4; kind = 3; }
}

uint bytes(uint kind) { return kind == 0 ? 4 : kind == 3 ? 1 : 2; }

float read(uint at, uint kind) {
    uint word = src.Load(at & ~3u);
    uint shift = (at & 3u) * 8;
    if (kind == 0) return asfloat(word);
    if (kind == 1) return f16tof32((word >> shift) & 0xFFFF);
    if (kind == 2) return float((word >> shift) & 0xFFFF) / 65535.0;
    return float((word >> shift) & 0xFF) / 255.0;
}

void write(uint at, uint kind, float v) {
    if (kind == 0) { dst.Store(at, asuint(v)); return; }
    uint bits;
    uint mask;
    if (kind == 1) { bits = f32tof16(v); mask = 0xFFFF; }
    else if (kind == 2) { bits = uint(round(saturate(v) * 65535.0)); mask = 0xFFFF; }
    else { bits = uint(round(saturate(v) * 255.0)); mask = 0xFF; }
    uint word_at = at & ~3u;
    uint shift = (at & 3u) * 8;
    uint word = dst.Load(word_at);
    dst.Store(word_at, (word & ~(mask << shift)) | ((bits & mask) << shift));
}

[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= count) return;
    if (mode == 1) {
        // D3D12_DRAW_ARGUMENTS: vertices, instances, first vertex, first instance.
        uint at = id.x * stride;
        uint4 a = src.Load4(at);
        dst.Store4(at, uint4(a.x / 3 * 6, a.y, a.z / 3 * 6, a.w));
        return;
    }
    uint from = id.x * 3 * stride;
    uint to = id.x * 6 * stride;
    // Triangles 0 1 2 and 0 2 3, with 3 the fourth corner.
    uint order[6] = { 0, 1, 2, 0, 2, 0 };
    for (uint k = 0; k < 6; k++) {
        for (uint b = 0; b < stride; b += 4) {
            dst.Store(to + k * stride + b, src.Load(from + order[k] * stride + b));
        }
    }
    uint fourth = to + 5 * stride;
    for (uint i = 0; i < elements; i++) {
        uint off = element(i) & 0xFFFF;
        uint n, kind;
        format(element(i) >> 16, n, kind);
        uint size = bytes(kind);
        for (uint c = 0; c < n; c++) {
            uint o = off + c * size;
            // Only components inside the vertex: the rest belong to the next.
            if (o + size > stride) break;
            float v = read(from + o, kind) + read(from + 2 * stride + o, kind) - read(from + stride + o, kind);
            write(fourth + o, kind, v);
        }
    }
}
"#;

struct Expand {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Expand {}
// SAFETY: as above.
unsafe impl Sync for Expand {}

fn expand() -> Option<&'static Expand> {
    static EXPAND: std::sync::OnceLock<Option<Expand>> = std::sync::OnceLock::new();
    EXPAND
        .get_or_init(|| match make() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!(error = %e, "no rectangle expansion: indirect rectangle draws dropped");
                None
            }
        })
        .as_ref()
}

fn make() -> windows::core::Result<Expand> {
    let device = &host().ok_or_else(windows::core::Error::empty)?.device;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; out pointers ours.
    let compiled = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("rects"),
            None,
            None,
            s!("main"),
            s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = compiled {
        if let Some(errors) = errors {
            // SAFETY: the blob's own bytes.
            let text = unsafe {
                std::slice::from_raw_parts(errors.GetBufferPointer().cast::<u8>(), errors.GetBufferSize())
            };
            tracing::warn!("rectangle expansion: {}", String::from_utf8_lossy(text));
        }
        return Err(e);
    }
    let code = code.ok_or_else(windows::core::Error::empty)?;
    // SAFETY: the blob's own bytes; the root signature is inside the shader.
    unsafe {
        let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes)?;
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr().cast(),
                BytecodeLength: bytes.len(),
            },
            ..Default::default()
        })?;
        Ok(Expand { root, pso })
    }
}

fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: std::mem::ManuallyDrop::new(None),
            }),
        },
    }
}

/// Runs the shader in `mode` over `count` items, reading host address
/// `from` and writing `to`.
fn run(list: &HostList, e: &Expand, constants: &[u32; 16], from: u64, to: u64, count: u32) {
    let l = &list.list;
    // SAFETY: an open list; our root signature and pipeline; the addresses
    // name live buffers the list keeps.
    unsafe {
        l.ResourceBarrier(&[uav_barrier()]);
        l.SetComputeRootSignature(&e.root);
        l.SetPipelineState(&e.pso);
        l.SetComputeRoot32BitConstants(0, 16, constants.as_ptr().cast(), 0);
        l.SetComputeRootUnorderedAccessView(1, from);
        l.SetComputeRootUnorderedAccessView(2, to);
        l.Dispatch(count.div_ceil(64), 1, 1);
        l.ResourceBarrier(&[uav_barrier()]);
    }
}

/// Records the expansion of the rectangles of `bytes` bytes of vertices
/// (`stride` apart, with `elements` as (offset, DXGI format)) at host
/// address `from`: a buffer of their triangles the list keeps, ready to be
/// read as vertices, and its size. Replaces the list's pipeline (callers
/// bind theirs after).
pub(crate) fn vertices(
    list: &HostList,
    host: &Host,
    from: u64,
    bytes: u64,
    stride: u32,
    elements: &[(u32, u32)],
) -> Option<(ID3D12Resource, u64, u32)> {
    let e = expand()?;
    if stride == 0 || !stride.is_multiple_of(4) || !from.is_multiple_of(4) || elements.len() > ELEMENTS {
        return None;
    }
    let rects = u32::try_from(bytes / u64::from(stride) / 3).ok()?;
    if rects == 0 {
        return None;
    }
    let size = u64::from(rects) * 6 * u64::from(stride);
    let size32 = u32::try_from(size).ok()?;
    let (buffer, _) = host.shared_buffer(size)?;
    crate::host::name(&buffer, || format!("rectangles {size:#x}"));
    list.keep(&buffer);
    // SAFETY: a live buffer.
    let to = unsafe { buffer.GetGPUVirtualAddress() };
    let mut constants = [0u32; 16];
    constants[..4].copy_from_slice(&[0, stride, rects, elements.len() as u32]);
    for (i, &(offset, format)) in elements.iter().enumerate() {
        constants[4 + i] = (offset & 0xFFFF) | format << 16;
    }
    run(list, e, &constants, from, to, rects);
    // SAFETY: an open list and our live buffer, written as unordered access.
    unsafe {
        list.list.ResourceBarrier(&[transition(
            &buffer,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
        )]);
    }
    Some((buffer, to, size32))
}

/// Records the rewrite of `records` draw records `stride` bytes apart at
/// host address `from` in `args` (left in the indirect-argument state, as
/// [`crate::indirect::bound`] leaves it) for the expanded vertex buffers: a
/// buffer of the new records, ready to be read as arguments.
pub(crate) fn records(
    list: &HostList,
    host: &Host,
    args: &ID3D12Resource,
    from: u64,
    stride: u32,
    records: u32,
) -> Option<ID3D12Resource> {
    let e = expand()?;
    if stride < 16 || !stride.is_multiple_of(4) || !from.is_multiple_of(4) || records == 0 {
        return None;
    }
    let size = u64::from(stride) * u64::from(records);
    let (buffer, _) = host.shared_buffer(size)?;
    crate::host::name(&buffer, || format!("rectangle records {size:#x}"));
    list.keep(&buffer);
    // SAFETY: a live buffer.
    let to = unsafe { buffer.GetGPUVirtualAddress() };
    // SAFETY: an open list and live buffers.
    unsafe {
        list.list.ResourceBarrier(&[transition(
            args,
            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )]);
    }
    let mut constants = [0u32; 16];
    constants[..3].copy_from_slice(&[1, stride, records]);
    run(list, e, &constants, from, to, records);
    // SAFETY: as above.
    unsafe {
        list.list.ResourceBarrier(&[
            transition(
                args,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            ),
            transition(
                &buffer,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            ),
        ]);
    }
    Some(buffer)
}

#[cfg(test)]
mod tests {
    /// The expansion compiles with the system's compiler and the host takes it.
    #[test]
    fn rectangle_expansion_builds() {
        if super::host().is_none() {
            return; // no host GPU here
        }
        assert!(super::expand().is_some());
    }
}
