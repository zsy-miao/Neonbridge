//! The DXBC container around a DXIL program: SFI0, ISG1, OSG1, PSV0 and DXIL
//! parts, sealed with the digest D3D12 checks. Compute only so far (no
//! signature elements). Shader model 6.7 and validator 1.7, as dxc writes them.
//!
//! Ported from five-star-recomp (GPL-3.0-or-later, ThatRecompDev and the Five
//! Star Recomp contributors), src/FiveStar.Shaders/Dxil/Container
//! (ContainerWriter.cs, ShaderContainer.cs, PsvWriter.cs, SignatureWriter.cs).

use super::hash;

/// DXIL validator and shader model minor version (1.7, 6.7).
pub const MAJOR: u32 = 1;
pub const MINOR: u32 = 7;
pub const MODEL_MAJOR: u32 = 6;

/// DXIL shader kinds (the program header's and PSV0's numbers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Pixel = 0,
    Vertex = 1,
    Geometry = 2,
    Compute = 5,
    Mesh = 13,
}

/// DXIL shader flags (entry property 0); the validator recomputes them and compares.
pub mod flags {
    pub const DOUBLE_PRECISION: u64 = 1 << 2;
    pub const RAW_AND_STRUCTURED_BUFFERS: u64 = 1 << 4;
    pub const DOUBLE_EXTENSIONS: u64 = 1 << 6;
    pub const STENCIL_EXPORT: u64 = 1 << 11;
    pub const TYPED_UAV_LOAD_ADDITIONAL_FORMATS: u64 = 1 << 13;
    pub const UAVS_AT_EVERY_STAGE: u64 = 1 << 16;
    pub const WAVE_OPS: u64 = 1 << 19;
    pub const INT64_OPS: u64 = 1 << 20;
    pub const BARYCENTRICS: u64 = 1 << 22;
    pub const ATOMIC_INT64_ON_GROUP_SHARED: u64 = 1 << 28;
    pub const RESOURCE_HEAP_INDEXING: u64 = 1 << 30;
    pub const SAMPLER_HEAP_INDEXING: u64 = 1 << 31;
    /// SM 6.7 texture operations: non-immediate offsets, SampleCmpLevel.
    pub const ADVANCED_TEXTURE_OPS: u64 = 1 << 34;
}

/// Shader flag -> SFI0 feature bit.
const FEATURES: [(u64, u64); 11] = [
    (flags::DOUBLE_PRECISION, 0x1),
    (flags::DOUBLE_EXTENSIONS, 0x20),
    (flags::STENCIL_EXPORT, 0x200),
    (flags::TYPED_UAV_LOAD_ADDITIONAL_FORMATS, 0x800),
    (flags::WAVE_OPS, 0x4000),
    (flags::INT64_OPS, 0x8000),
    (flags::BARYCENTRICS, 0x2_0000),
    (flags::ATOMIC_INT64_ON_GROUP_SHARED, 0x80_0000),
    (flags::RESOURCE_HEAP_INDEXING, 0x200_0000),
    (flags::SAMPLER_HEAP_INDEXING, 0x400_0000),
    (flags::ADVANCED_TEXTURE_OPS, 0x2000_0000),
];

/// A resource bound by register, as PSV0 lists it: PSV type (1 sampler, 2 CBV,
/// 3 SRV typed, 4 SRV raw, 5 SRV structured, 6 UAV typed, 7 UAV raw, 8 UAV
/// structured), space, register range and DXIL resource kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub kind_psv: u32,
    pub space: u32,
    pub lower: u32,
    pub upper: u32,
    pub kind: u32,
}

/// What the container says about a compute program.
#[derive(Clone, Debug)]
pub struct Compute {
    pub threads: [u32; 3],
    pub wave_size: Option<u32>,
    pub flags: u64,
    pub bindings: Vec<Binding>,
}

/// Header, part offsets, parts (fourcc, size, bytes), then the digest.
pub fn build(parts: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    const HEADER: usize = 32;
    let table = HEADER + 4 * parts.len();
    let total = table + parts.iter().map(|(_, d)| 8 + d.len()).sum::<usize>();
    let mut out = vec![0u8; total];
    out[..4].copy_from_slice(b"DXBC");
    out[20..22].copy_from_slice(&1u16.to_le_bytes());
    out[24..28].copy_from_slice(&(total as u32).to_le_bytes());
    out[28..32].copy_from_slice(&(parts.len() as u32).to_le_bytes());
    let mut offset = table;
    for (i, (fourcc, data)) in parts.iter().enumerate() {
        out[HEADER + 4 * i..HEADER + 4 * i + 4].copy_from_slice(&(offset as u32).to_le_bytes());
        out[offset..offset + 4].copy_from_slice(*fourcc);
        out[offset + 4..offset + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        out[offset + 8..offset + 8 + data.len()].copy_from_slice(data);
        offset += 8 + data.len();
    }
    hash::seal(&mut out);
    out
}

/// The DXIL part: program header (kind, version, size in dwords), bitcode
/// header ("DXIL", DXIL version, offset, size), bitcode.
pub fn program(bitcode: &[u8], kind: Kind) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + bitcode.len());
    let version = (u32::from(kind as u8) << 16) | (MODEL_MAJOR << 4) | MINOR;
    let words = ((24 + bitcode.len()) / 4) as u32;
    for w in [version, words] {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(b"DXIL");
    for w in [(MAJOR << 8) | MINOR, 16, bitcode.len() as u32] {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(bitcode);
    out
}

/// SFI0: the optional features the shader flags imply.
pub fn sfi0(shader_flags: u64) -> Vec<u8> {
    let features = FEATURES.iter().filter(|(f, _)| shader_flags & f != 0).fold(0u64, |a, (_, b)| a | b);
    features.to_le_bytes().to_vec()
}

/// A signature part with no elements (count 0, elements at offset 8).
pub fn empty_signature() -> Vec<u8> {
    [0u32, 8].iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// PSV0 with runtime info version 2 (48 bytes) for a compute program.
pub fn psv0_compute(c: &Compute) -> Vec<u8> {
    let mut info = [0u8; 48];
    let (min, max) = c.wave_size.map_or((0, u32::MAX), |w| (w, w));
    info[16..20].copy_from_slice(&min.to_le_bytes());
    info[20..24].copy_from_slice(&max.to_le_bytes());
    info[24] = Kind::Compute as u8;
    for (i, t) in c.threads.iter().enumerate() {
        info[36 + 4 * i..40 + 4 * i].copy_from_slice(&t.to_le_bytes());
    }
    let mut out = Vec::new();
    let word = |out: &mut Vec<u8>, w: u32| out.extend_from_slice(&w.to_le_bytes());
    word(&mut out, 48);
    out.extend_from_slice(&info);
    word(&mut out, c.bindings.len() as u32);
    if !c.bindings.is_empty() {
        word(&mut out, 24);
        for b in &c.bindings {
            for w in [b.kind_psv, b.space, b.lower, b.upper, b.kind, 0] {
                word(&mut out, w);
            }
        }
    }
    // String table (one empty string, padded) and an empty semantic index table.
    word(&mut out, 4);
    out.extend_from_slice(&[0; 4]);
    word(&mut out, 0);
    out
}

/// The full container for a compute program's bitcode.
pub fn compute(bitcode: &[u8], c: &Compute) -> Vec<u8> {
    build(&[
        (b"SFI0", sfi0(c.flags)),
        (b"ISG1", empty_signature()),
        (b"OSG1", empty_signature()),
        (b"PSV0", psv0_compute(c)),
        (b"DXIL", program(bitcode, Kind::Compute)),
    ])
}
