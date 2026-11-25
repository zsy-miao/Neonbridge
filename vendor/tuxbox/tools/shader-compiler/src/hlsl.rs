//! RDNA2 → HLSL translator (stage 3 of the shader pipeline).
//!
//! Model: one HLSL invocation == one RDNA2 *lane*. Every VGPR/SGPR is a `uint` holding the
//! raw 32-bit pattern (floats via asfloat/asuint), so no type inference is needed. Lane
//! masks used for control flow are represented by their bit for *this* lane (0/1).
//! Recognized wave64 ballots consumed as integer data retain both mask words. Descriptor-table pointers in user SGPRs are tracked symbolically: an
//! `s_load_dwordxN` from a never-written SGPR is a descriptor fetch from a root table, and
//! the way the descriptor is consumed (s_buffer_load → CBV, MIMG resource → SRV, MIMG
//! sampler → sampler) classifies the table. Slots map 1:1 to HLSL registers because the
//! title's root signature (UE layout) uses one SRV/CBV/sampler table per stage from
//! register 0.
//!
//! Scope: forward-only control flow (no loops yet). Anything not understood returns `Err`
//! naming the instruction — the caller must fall back; nothing is guessed.

use crate::meta::SigElement;
use crate::rdna2::{disassemble, Enc, Insn, Opnd};
use crate::{ShaderExtract, Stage};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

mod driver;
mod emitter;
mod rewrite;
mod text;
pub use driver::*;
use rewrite::*;
use text::*;
mod code_table;
mod wave_alloc;
mod group_extrema;
mod compare;
mod uniform_texel;
mod pixel_quad;
mod mubuf_offsets;
mod scalar_values;
mod code_offset;
mod scalar_bits;
#[cfg(test)]
#[path = "tests/hlsl/raw_width.rs"]
mod raw_width;
#[cfg(test)]
#[path = "tests/hlsl/raw_idxen.rs"]
mod raw_idxen;
#[cfg(test)]
#[path = "tests/hlsl/wide_atomic_test.rs"]
mod wide_atomic_test;
#[cfg(test)]
#[path = "tests/hlsl/cpu_oracle.rs"]
mod cpu_oracle;
#[cfg(test)]
#[path = "tests/hlsl/heap_resinfo.rs"]
mod heap_resinfo;
#[cfg(test)]
#[path = "tests/hlsl/compute_swizzle.rs"]
mod compute_swizzle;
#[cfg(test)]
#[path = "tests/hlsl/sat_pk_u8_i16.rs"]
mod sat_pk_u8_i16;
mod fssm_bindings;
mod micropoly;
mod mesh;
mod mesh_exports;
mod exec_group;
mod exec_snapshot;
mod hair_cluster;
mod hair_cluster_export;
mod instance_cull;
mod instance_cull_tail;
mod instance_cull_hzb;
mod instance_cull_post;
mod instance_cull_views;
mod classify_materials;
mod int16;
mod int32;
mod float64;
mod indexed_lane;
mod particle_sort;
mod persistent;
mod persistent_lanes;
mod persistent_resources;
mod lds_tile;
mod lds_order;
mod ps09_classify;
mod subsurface_setup;
mod hair_compact;
mod hair_multisample;
mod hair_visibility_single;
mod hair_visibility_indirect;
mod packed_root;
mod mesh_bindings;
mod params;
pub(crate) use params::flat_registers;
use params::Param;
mod light_grid;
mod lone_lane;
mod page_waves;
mod wave_cull;
mod ray_resolve;
mod fspx_wave;
mod singleton_wave;
mod wave_sum;
mod lockstep;
mod wave_loops;
mod raw_index;
mod reflection_compact;
pub(crate) mod radiance_probe;
mod screen_gather;
mod reflection_far;
mod sampler_heap;
mod lane_reads;
mod heap_offsets;
mod heap_origins;
use sampler_heap::DirectSampler;
mod reflection_probe;
mod wave_scan;
use packed_root::{PackedRoot, RootAbi};

pub struct Translated {
    pub hlsl: String,
    pub entry: &'static str,
    /// Target profile for the generated HLSL.
    pub profile: &'static str,
    /// Per-vertex parameter reads require a shader-model 6.1 pixel shader.
    pub needs_barycentrics: bool,
    /// A two-word typed image atomic requires shader model 6.6.
    pub typed_int64_atomics: bool,
    pub packed_dot_product: bool,
    /// Required wave size for compute shaders with width-specific lane reads.
    pub required_wave_size: Option<u32>,
    /// Workgroup dimensions for the subset verified to tolerate either Scarlett wave width.
    pub portable_compute_group: Option<[u32; 3]>,
    /// Pixel shaders: the interpolation modifier of each input. Pass it to
    /// [`translate_linked`] when translating the stage that feeds this shader.
    pub ps_input_interp: InputInterp,
}

/// Interpolation modifiers of a pixel shader's inputs, by semantic (`TEXCOORD0`), in input
/// signature order. Only inputs with a modifier are listed; the text is the HLSL modifier
/// as emitted (it may be the `TB_VERTEX_INPUT` macro).
///
/// DXC packs two signature elements into one register only if they have the same
/// interpolation mode. The pixel shader picks modifiers from how its microcode reads each
/// parameter (per-vertex, flat, centroid, integer), while the stage feeding it used to
/// declare none, so the two sides could be packed differently: D3D12 then rejects the
/// pipeline (`CREATEGRAPHICSPIPELINESTATE_SHADER_LINKAGE_REGISTERINDEX` /
/// `_REGISTERMASK`). Seen in Fortnite: a geometry shader wrote TEXCOORD0.xy and
/// SV_RenderTargetArrayIndex to r0.xy and r2.x, its pixel shader read TEXCOORD0 per vertex
/// (`nointerpolation`) and DXC packed SV_RenderTargetArrayIndex into r0.z.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InputInterp(pub Vec<(String, String)>);

impl InputInterp {
    fn get(&self, semantic: &str) -> Option<&str> {
        self.0.iter().find(|(s, _)| s.eq_ignore_ascii_case(semantic)).map(|(_, m)| m.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sym {
    DirectRaw(u32),
    /// Root SRV address words 0 and 1, or word 1 after setting the byte stride.
    MeshRawAddress { register: u8, word: u8 },
    SamplerOffset(DirectSampler),
    HeapSampler(DirectSampler),
    HeapOffset,
    HeapDescriptor(u32, Option<usize>),
    /// Dword 1 and up of a descriptor fetched at a run-time heap index. The index itself
    /// lives in dword 0, so a use is valid only while the others are still the loaded ones.
    HeapWord(u8, Option<usize>),
    /// Guest sampler-heap index read at a run-time byte offset (compute sampler heap).
    HeapSamplerIndex(Option<usize>),
    /// Byte offset of a 16-byte sampler descriptor: a scalar shifted left by four.
    HeapSamplerOffset,
    /// See `lane_reads`.
    NoLaneRead,
    InlineWord(u32),
    Table(u8),
    Desc { table: u8, off: u32, k: u32 },
    /// Dword `k` of one of several descriptors (control-flow join of different `s_load`s,
    /// e.g. a loop that picks texture i of N). `set` indexes `Emitter::multi_sets`; which
    /// candidate is live is tracked at run time in a `tb_dsel<reg>` variable.
    Multi { set: u16, k: u32 },
    /// A PC-relative pointer into the shader's own code section (`s_getpc_b64` + offsets),
    /// used to read constant tables embedded after the program.
    Code,
}

/// An image op whose texture and/or sampler register holds one of several descriptors.
/// Each candidate is `(descriptor id, resolved resource)`; the id is what the run-time
/// selector `tb_dsel<reg>` holds.
#[derive(Clone, Debug)]
struct MSel {
    tex_reg: u16,
    samp_reg: u16,
    tex: Vec<(u32, Res)>,
    samp: Vec<(u32, Res)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Res {
    HeapLoad(u16),
    HeapBuffer(u16),
    /// Byte-address buffer named by a heap index held in this SGPR (compute heap).
    HeapRaw(u16),
    HeapTexture(u16, u32),
    Cb(u32),
    Tex(u32, u32), // slot, dim
    Samp(u32),
    HeapSampler(DirectSampler),
    /// Sampler-heap byte offset in this SGPR becomes a sampler index (16-byte descriptors).
    HeapSamplerLoad(u16),
    /// Sampler named by the sampler-heap index held in this SGPR.
    HeapSamplerDyn(u16),
    Uav(u32, u32), // slot, dim
    Buf(u32, bool), // slot, is_uav (structured buffer)
    Raw(u32, bool), // slot, is_uav (byte-address buffer)
    TBuf(u32, bool), // slot, is_uav (typed buffer)
    CbDyn(u32),      // constant buffer read through MUBUF (per-lane offset)
    CodeTbl,         // s_load from the code section (embedded constant table)
}

struct Emitter<'a> {
    lds_tile: Option<lds_tile::LdsTile>,
    packed: Option<PackedRoot>,
    ex: &'a ShaderExtract,
    insns: Vec<Insn>,
    res: HashMap<usize, Vec<Res>>, // per-instruction resolved resources
    cb_size: BTreeMap<u32, u32>,   // slot -> dwords
    zero_buffer_offsets: std::cell::OnceCell<BTreeSet<(usize, u8)>>,
    ordinary_bit_sites: std::cell::OnceCell<BTreeSet<usize>>,
    smem_constants: std::cell::OnceCell<BTreeMap<(usize, u8), u32>>,
    tex: BTreeMap<u32, u32>,       // slot -> dim
    samp: BTreeSet<u32>,
    samp_cmp: BTreeSet<u32>,
    uav: BTreeMap<u32, u32>,       // slot -> dim
    sbuf: BTreeMap<(bool, u32), u32>, // (is_uav, slot) -> element dwords used
    reflected_strides: BTreeSet<(bool, u32)>,
    /// PersistentClusterCull's validated buffer layout and cross-group UAV queue.
    persistent_queue: bool,
    raw: BTreeSet<(bool, u32)>,
    // Native sites with a proven byte-stride-one linear descriptor.
    raw_byte_index: BTreeSet<usize>,
    tbuf: BTreeSet<(bool, u32)>,      // typed buffers (is_uav, slot)
    tbuf_uint: BTreeSet<u32>,         // typed UAV slots whose element must be uint (atomics)
    /// Inferred integer element type of typed buffers: (is_uav, slot) -> 1 uint, 2 sint.
    tbuf_ty: BTreeMap<(bool, u32), u8>,
    /// Inferred integer texel type of textures read with texel fetches / written with image
    /// stores: (is_uav, slot) -> 1 uint, 2 sint. Absent = float (the default).
    img_ty: BTreeMap<(bool, u32), u8>,
    uses_heap_sampler: bool,
    resource_heap_base: Option<u8>,
    /// The heap base comes from FSSM user-data class 5. Compute reads that
    /// register directly (`compute_heap_base`); mesh adds the eight system SGPRs
    /// (`mesh_heap_base`) and then uses this same lowering.
    compute_heap: bool,
    /// A root-less pixel shader that indexes a descriptor heap (`pixel_heap_base`).
    pixel_heap: bool,
    /// Sampler registers bound through the root signature. A heap sampler with the same
    /// number would reuse the HLSL name.
    table_samplers: BTreeSet<u32>,
    /// Vertex programs: how the values of each typed heap load are used, as
    /// [float or unknown, uint, sint].
    heap_load_ty: HashMap<usize, [bool; 3]>,
    /// The root carries TuxBox's guest-to-host heap index map (root SRV t0, space32766).
    heap_map: bool,
    /// Compute sampler heap base SGPR (FSSM user-data class 6).
    sampler_heap_base: Option<u8>,
    /// Sampler slots read from that heap: bound from SamplerDescriptorHeap in main.
    heap_sampler_slots: BTreeSet<u32>,
    private_samplers: BTreeMap<usize, u32>,
    binding_spaces: BTreeMap<(u32, u32), u32>,
    mesh_bindings: Option<mesh_bindings::MeshBindings>,
    compute_derivatives: bool,
    /// Pixel-shader colour-export epilogues (see `find_export_epilogs`): pack instruction
    /// index -> capture id, and export instruction index -> the capture ids feeding its
    /// two packed operands (usize::MAX = operand not written).
    epilog_pack: HashMap<usize, usize>,
    epilog_exp: HashMap<usize, (usize, usize)>,
    /// Index of the instruction being emitted.
    cur_idx: usize,
    /// Structured buffers read with scalar loads (`s_buffer_load` through an SRV/UAV
    /// descriptor): they need the byte-offset helper.
    sbuf_flat: BTreeSet<(bool, u32)>,
    is_cs: bool,
    /// Nesting depth of emitted HLSL loops at the current instruction.
    loop_depth: u32,
    is_gs: bool,
    /// Compute shader uses LDS (groupshared memory).
    uses_lds: bool,
    /// Shader reads constant tables embedded in its code section.
    uses_code: bool,
    /// The program uses register-relative moves: VGPRs live in the `tb_v` array.
    vreg_array: bool,
    /// Interned candidate lists `(table, offset)` for `Sym::Multi`.
    multi_sets: Vec<Vec<(u8, u32)>>,
    /// Image ops whose texture and/or sampler is one of several descriptors.
    /// UAV texture slots used with image atomics (declared `<uint>`).
    uav_uint: BTreeSet<u32>,
    /// Typed image slots carrying a two-word visibility value in a 64-bit atomic.
    wide_visibility_uavs: BTreeSet<u32>,
    /// Instruction where all 128 rasterizer threads vote in their logical wave.
    micropoly_vote: Option<usize>,
    view_cull: Option<instance_cull_views::ViewProgram>,
    /// The shader uses wave intrinsics (needs shader model 6: DXC only).
    required_wave_size: Option<u32>,
    // Proven whole-wave EXECZ guards of verified masked reductions.
    reduction_execz_guards: BTreeSet<usize>,
    /// Set when every branch is a decision of the whole wave (see `lockstep`),
    /// with the condition codes to vote after the instructions that set them.
    lockstep: Option<BTreeMap<usize, lockstep::Vote>>,
    /// Structured loop latches with a verified guest trip bound.
    wave_bounded_latches: BTreeSet<usize>,
    portable_wave64: bool,
    msel: HashMap<usize, MSel>,
    /// Buffer ops (MUBUF, s_buffer_load) whose descriptor register holds one of several
    /// descriptors: (register key, candidates as (descriptor id, resource)).
    bsel: HashMap<usize, (u16, Vec<(u32, Res)>)>,
    /// Descriptor loads `(base register, descriptor id)` and base-dword moves `(dst, src)`
    /// per instruction, for maintaining the run-time `tb_dsel<reg>` selectors.
    dsel_loads: HashMap<usize, (u16, u32)>,
    dsel_runtime: HashMap<usize, (u16, u8, u8, u32)>,
    dsel_moves: HashMap<usize, (u16, u16)>,
    /// Registers read before written (entry ABI), from `live_in`.
    live: Vec<String>,
    /// Pixel shader needs its position within the 2x2 quad (ds_swizzle).
    uses_quad: bool,
    compute_swizzle: bool,
    vote_defs: BTreeSet<usize>,
    vote_uses: BTreeMap<usize, usize>,
    exec_snapshots: exec_snapshot::Snapshots,
    reflected_lds_words: Option<u32>,
    hoisted_swizzle: Option<usize>,
    indexed_lane: bool,
    /// Complete 32-thread wave using independent one-word vector comparison masks.
    single_wave32: bool,
    resolve_wave32: bool,
    wave32_compare_masks: bool,
    light_grid: bool,
    page_waves: bool,
    wave_cull: bool,
    /// Mesh first-lane reads for which a lane's own value is the one read.
    own_lane_reads: BTreeSet<usize>,
    /// Mesh samples whose sampler a lane picks among sampler-heap entries; the first word of
    /// such a descriptor then holds its heap index.
    chosen_samplers: BTreeSet<usize>,
    /// Instruction ranges of a mesh program's loops.
    loop_ranges: Vec<(usize, usize)>,
    fspx_wave: Option<u32>,
    hair_multisample: bool,
    hoist_wave_exchanges: bool,
    regs: BTreeSet<String>,
    body: String,
    tmp: usize,
    /// PS attribute index -> input register (registers the shader actually reads).
    ps_attrs: Vec<Param>,
    pulled_attrs: BTreeSet<usize>,
    /// Each pixel is a wave of its own.
    pixel_wave: bool,
    dispatched: bool,
    /// The reduction being emitted adds.
    pixel_sum: bool,
    flat_attrs: BTreeSet<usize>,
    /// VS parameter-export index -> output register.
    out_params: Vec<Param>,
    /// Pixel shader: the interpolation modifier given to each input (filled by `finish`).
    ps_in_interp: InputInterp,
    /// Stage feeding a pixel shader: modifiers to put on the matching outputs.
    out_interp: InputInterp,
    /// One function-scoped resource for each heap view of an SGPR. The local
    /// captures the descriptor at assignment.
    heap_locals: BTreeSet<(HeapView, u16)>,
    /// Views whose local still holds this SGPR's current heap index.
    heap_bound: BTreeSet<(HeapView, u16)>,
}

/// HLSL resource view of one `ResourceDescriptorHeap` index.
///
/// The index at every current heap subscript is an SGPR (a scalar shift, or
/// `v_readfirstlane` into an SGPR). That value is uniform for the lanes that
/// execute the assignment, so the subscript is `ResourceDescriptorHeap[sN]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HeapView {
    Raw,
    RawRw,
    Constants,
    Typed,
    TypedUint,
    TypedRw,
    Tex1D,
    Tex2D,
    Tex2DUint,
    Tex2DRwUint,
    Tex3D,
    TexCube,
    Tex1DArray,
    Tex2DArray,
}

impl HeapView {
    fn hlsl(self) -> &'static str {
        match self {
            HeapView::Raw => "ByteAddressBuffer",
            HeapView::RawRw => "RWByteAddressBuffer",
            HeapView::Constants => "ConstantBuffer<TBHeapConstants>",
            HeapView::Typed => "Buffer<float4>",
            HeapView::TypedUint => "Buffer<uint4>",
            HeapView::TypedRw => "RWBuffer<float4>",
            HeapView::Tex1D => "Texture1D<float4>",
            HeapView::Tex2D => "Texture2D<float4>",
            HeapView::Tex2DUint => "Texture2D<uint4>",
            HeapView::Tex2DRwUint => "RWTexture2D<uint4>",
            HeapView::Tex3D => "Texture3D<float4>",
            HeapView::TexCube => "TextureCube<float4>",
            HeapView::Tex1DArray => "Texture1DArray<float4>",
            HeapView::Tex2DArray => "Texture2DArray<float4>",
        }
    }

    fn local(self, sgpr: u16) -> String {
        let tag = match self {
            HeapView::Raw => "raw",
            HeapView::RawRw => "raww",
            HeapView::Constants => "cb",
            HeapView::Typed => "buf",
            HeapView::TypedUint => "bufu",
            HeapView::TypedRw => "rwbuf",
            HeapView::Tex1D => "t1",
            HeapView::Tex2D => "t2",
            HeapView::Tex2DUint => "t2u",
            HeapView::Tex2DRwUint => "rwt2u",
            HeapView::Tex3D => "t3",
            HeapView::TexCube => "tc",
            HeapView::Tex1DArray => "t1a",
            HeapView::Tex2DArray => "t2a",
        };
        format!("tb_heap_{tag}_{sgpr}")
    }
}

/// Distance between the names of resources that share a register in different spaces.
const SPACE_ALIAS: u32 = 1 << 16;
/// Synthetic opcode of the wave-reduction instruction produced by `lower_wave_reductions`.
const OP_WAVE_REDUCE: u32 = 0x7ff0;
const OP_DPP_BINARY: u32 = 0x7ff5;
const OP_LOCAL_EXTREMA: u32 = 0x7fe3;
const OP_QUAD_MOVE: u32 = 0x7fe1;
const OP_ROW_XOR_MOVE: u32 = 0x7fe2;
const OP_ROW_SHARE_MOVE: u32 = 0x7fe3;
const OP_ROW_ROR_MOVE: u32 = 0x7fe4;
const OP_ROW_SHIFT_MOVE: u32 = 0x7fe5;

use crate::rdna2::{wide_bit_result, writes_scalar};

#[cfg(test)]
#[path = "tests/hlsl.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/hlsl/election_order_regression.rs"]
mod election_order_regression;

#[cfg(test)]
#[path = "tests/hlsl/reduction_execz_regression.rs"]
mod reduction_execz_regression;
