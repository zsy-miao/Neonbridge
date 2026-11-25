//! DXIL emission (decision 0003, step 3): a console vertex, pixel, compute
//! or mesh shader as a PC DXIL container (SM 6.6) that host D3D12 accepts.
//! Compute shaders keep the console's thread-group size; one that depends
//! on the lane layout (cross-lane operations) declares the console's wave
//! size (`WaveSize`), so the host device must support it. On a host of
//! 32-lane waves, a wave64 program that no form in 32-lane waves gives as
//! the console runs it takes the paired form instead (`paired`): a host
//! wave for each console wave, two console lanes a thread, half the threads.
//!
//! What the host binds for a translated shader ([`HostShader`]):
//!
//! * **User data** at `b0, space 0` (32-bit root constants or a constant
//!   buffer, per stage): dword `index` holds a root parameter's value, as
//!   [`UserData`] lists. A root-constant dword is the constant. A
//!   descriptor table is the byte offset of its first descriptor in its
//!   heap: the descriptor index times 32 for CBV/SRV/UAV tables, times 16
//!   for sampler tables (the console's descriptor sizes; the code adds
//!   offsets in those units). The heap bases of dynamic resources are 0
//!   and not listed.
//! * **Descriptors** through SM 6.6 `ResourceDescriptorHeap[]` and
//!   `SamplerDescriptorHeap[]`, at the index the console address gives, so
//!   the host heaps keep the title's descriptor order. The root signature
//!   needs `CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED` and
//!   `SAMPLER_HEAP_DIRECTLY_INDEXED`.
//! * **Immediate samplers** as static samplers `s<n>, space 0`, `n` the
//!   position in [`HostShader::samplers`] (the console sampler words) and
//!   [`HostShader::static_samplers`] (the same decoded into a D3D12 static
//!   sampler description). Comparison sampling (added 2026-11-23): an
//!   immediate sampler it uses has a comparison filter there (bit 0x80);
//!   a heap sampler it uses must be a comparison sampler on the host.
//! * **Root CBVs and root SRVs** (raw buffers) as root descriptors
//!   `b<i>` / `t<i>` in [`ROOT_CBV_SPACE`], `i` the title's root parameter
//!   index ([`HostShader::root_cbvs`], [`HostShader::root_srvs`]); the
//!   address the title sets is the buffer's.
//! * **Vertex fetch** (added 2026-11-23). The title's mesh and
//!   vertex programs fetch vertices themselves with typed-format loads
//!   (`tbuffer_load_format_*`): the format (8_8_8_8 unorm, 16_16 snorm,
//!   2_10_10_10 unorm, 8_8_8_8 and 16_16_16_16 uint...) is in the
//!   instruction, so the translation reads raw bytes and converts them
//!   itself. Their descriptors are built around a root SRV (stride 1, no
//!   record limit): the host binds the root SRV `t<slot>` above with the
//!   address the title sets, nothing more. The vertex index, the stride
//!   and the attribute offset are the shader's own arithmetic on data in
//!   that buffer (per-stream headers), not descriptor fields.
//! * **Buffer views in the heap** are declared by how the shader reads
//!   them: raw (ByteAddressBuffer) for dword loads and stores, atomics and
//!   typed-format loads without an index; structured with the used-resource
//!   entry's stride when indexed; typed with four 32-bit float channels for
//!   `buffer_load_format_*` and `buffer_store_format_*`. The host's view at
//!   that index must be of that kind and start at the console descriptor's
//!   base address (raw: `R32_TYPELESS` with the raw flag), whatever kind the
//!   title's view description says. Typed-format loads from heap
//!   descriptors (1,024 world compute programs, ray-tracing material
//!   resolves) read raw views this way.
//!
//! * **Texture atomics** use a scalar integer UAV. The host binds R32_UINT
//!   for 32-bit operations and R32G32_UINT for 64-bit operations. The latter
//!   require SM 6.6 atomic support on typed and descriptor-heap resources.
//!
//! Entry points: [`translate`] (container bytes as the title passes them
//! at pipeline creation → [`HostShader`]), [`emit`] (the same with the
//! earlier stages already run). The root constants of each stage must
//! cover [`HostShader::user_data_dwords`]. Stability: these types and the
//! binding scheme are what pipeline creation builds on; changes are
//! announced.
//! * **Signatures** as the console container has them (same semantics,
//!   registers and masks), so the input layout and stage linking are the
//!   title's; pixel outputs are plain 32-bit values.
//! * **Mesh shaders** ([`HwStage::Ms`], `ms_6_6`): user data and
//!   descriptors as above (the root constants visible to the mesh stage);
//!   the pipeline is a mesh pipeline (pipeline-state stream, no input
//!   layout) with the title's pixel shader. The thread group is the
//!   console's in lanes (`[numthreads]` in the DXIL), the vertex and
//!   primitive maxima its own, triangles only, no wave size required. The
//!   console numbers groups in x only: the draw path passes the title's
//!   count as `DispatchMesh(x, 1, 1)`. Per-primitive outputs (`PSG1`) are
//!   the console's, without `SV_PrimitiveID` and `SV_CullPrimitive` (the
//!   console never exports them).
//! * **Rectangle lists** ([`rect_list_gs`], `gs_6_6`): the console draws a
//!   rectangle from three vertices; the host has no such topology. A
//!   pipeline the title makes for rectangles (topology type 19) gets a
//!   geometry shader of ours between the translated vertex and pixel
//!   shaders and draws triangle lists with the title's own vertex counts.
//!   It binds nothing.

mod bitcode;
mod container;
pub(crate) mod gs;
#[cfg(all(test, windows))]
mod descriptor_tests;
#[cfg(all(test, windows))]
mod diff_tests;
#[cfg(all(test, windows))]
mod intersect_tests;
#[cfg(all(windows, any(test, feature = "host")))]
pub mod host;
pub mod stand_in;
mod idiom;
mod link;
mod lower;
mod own_bits;
mod rect;
mod sampler;
mod shader;
mod threading;
#[cfg(windows)]
pub mod validator;
#[cfg(all(test, windows))]
mod valu_tests;
#[cfg(all(test, windows))]
mod waterfall_tests;
#[cfg(all(test, windows))]
mod wave64_tests;
mod wave;

use crate::dxbc::{Container, FourCC};
use crate::native::{HwStage, NativeProgram, RootType};
use crate::{ShaderError, io, ir, resource, structure};

pub use bitcode::census;
pub use link::{link_mesh, mesh_fits};
pub use sampler::StaticSampler;
pub use lower::set_host_max_wave;
pub use rect::rect_list_gs;
pub use shader::{
    MEMORY_FAULTS, MEMORY_PAGES, ROOT_CBV_SPACE, SAMPLER_SPACE, SLOT_ADDRESSES, SLOT_FAULTS, SLOT_MEMORY,
    SLOT_STRIDES, SLOT_STRIDES_COUNT, SLOT_STRIDES_DWORDS, USER_DATA_SPACE, USER_DISPATCH_THREADS, USER_MEMORY_BASE, USER_VIEW_ADDRESS,
    USER_VIEW_BIAS,
    set_view_heap_limit, view_heap_limit,
};

/// The loop limit without an override: 2^20.
pub const DEFAULT_LOOP_LIMIT: u32 = lower::LOOP_LIMIT;

/// The iteration count after which a translated loop is left, per lane and
/// per entry into the loop: [`DEFAULT_LOOP_LIMIT`].
///
/// With the `diagnostics` feature, `VICTORY_SHADER_LOOP_LIMIT` (decimal or
/// `0x` hex, at least 1) sets it instead. A loop that hits the limit in
/// every lane of a full-screen pass still runs for seconds (2^20 times a
/// 4K screen is about 10^13 iterations); a much lower limit shows whether
/// such loops are what makes a frame slow. The value is part of the
/// translation, so the shader cache must key on it (`victory-gpu`'s does).
pub fn loop_limit() -> u32 {
    static LIMIT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        if cfg!(feature = "diagnostics") {
            parse_loop_limit(std::env::var("VICTORY_SHADER_LOOP_LIMIT").ok().as_deref())
        } else {
            lower::LOOP_LIMIT
        }
    })
}

/// Which of a translated program's group barriers wait for the whole
/// group (`GroupMemoryBarrierWithGroupSync`); the others keep their memory
/// fence and lose the wait. A wait that some thread of the group never
/// reaches is undefined on the host, and NVIDIA's wait is for every
/// thread: the group stands until Windows resets the card. (The console's
/// barrier is the wave's, whatever lanes are live, and a wave that ended
/// is not waited for.) `VICTORY_GROUP_SYNC`:
/// - unset or `2`: every barrier waits, as before.
/// - `1`: the title's `s_barrier` waits only in a block every lane runs,
///   in a program no lane can be cut out of at the iteration limit.
/// - `0`: no barrier waits, neither the title's nor the ones the
///   translation adds (wave64 programs in two host waves exchange through
///   them, so such programs compute wrong values). For one run that tells
///   whether a barrier is what stops the card: with it no run can stand in
///   a barrier.
///
/// Part of the translation: the shader cache keys on it (`victory-gpu`'s
/// does).
pub fn group_sync() -> u8 {
    static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_GROUP_SYNC").ok().as_deref().map(str::trim) {
        Some("0") => 0,
        Some("1") => 1,
        _ => 2,
    })
}

/// The clean-up passes that run over a translated program before it is
/// written (`bitcode::opt`): a set of that module's pass bits. Every pass
/// is exact, so the switch is for telling a translator fault from a
/// clean-up fault, and for measuring what the clean-up is worth.
/// `VICTORY_SHADER_OPT`:
/// - unset: every pass.
/// - `0`: none (the module as the lowering made it).
/// - a number (decimal or `0x` hex): those passes; 1 dead code, 2 constants
///   and identities, 4 one instruction for one value, 8 what a branch taken
///   says about its condition, 16 one read for one place within a block,
///   32 a first-lane read of two host waves without the round that asks
///   which half has a lane (where every lane has one).
///
/// Part of the translation: the shader cache keys on it (`victory-gpu`'s
/// does).
pub fn shader_opt() -> u32 {
    static PASSES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *PASSES.get_or_init(|| parse_shader_opt(std::env::var("VICTORY_SHADER_OPT").ok().as_deref()))
}

/// Everything a translation depends on besides the container: the
/// translator's own source (hashed at build time), the value of every
/// `VICTORY_*` switch it reads, the host facts the device hands it (widest
/// wave, view heap limit) and the settings above. The shader cache keys on
/// this instead of on the whole executable, so a rebuild that leaves the
/// translator alone keeps its translations. Call it after the device is made.
pub fn translation_fingerprint() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(env!("VICTORY_SHADER_SOURCE_HASH").as_bytes());
    out.push(u8::from(cfg!(feature = "diagnostics")));
    out.push(u8::from(cfg!(debug_assertions)));
    for name in env!("VICTORY_SHADER_SWITCHES").split(',').filter(|n| !n.is_empty()) {
        out.extend_from_slice(name.as_bytes());
        out.push(b'=');
        if let Some(value) = std::env::var_os(name) {
            out.extend_from_slice(value.as_encoded_bytes());
        }
        out.push(0);
    }
    out.extend_from_slice(&lower::host_max_wave().to_le_bytes());
    out.extend_from_slice(&shader::view_heap_limit().to_le_bytes());
    out.extend_from_slice(&loop_limit().to_le_bytes());
    out.push(group_sync());
    out.extend_from_slice(&shader_opt().to_le_bytes());
    out
}

/// `VICTORY_SHADER_OPT`'s value; every pass when it is unset or
/// unreadable. Pure, for testing.
fn parse_shader_opt(value: Option<&str>) -> u32 {
    let parsed = value
        .map(str::trim)
        .and_then(|v| match v.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => v.parse().ok(),
        });
    parsed.map_or(bitcode::opt::ALL, |n| n & bitcode::opt::ALL)
}

/// `VICTORY_SHADER_LOOP_LIMIT`'s value; the default when it is unset,
/// unreadable or 0. Pure, for testing.
fn parse_loop_limit(value: Option<&str>) -> u32 {
    let parsed = value
        .map(str::trim)
        .and_then(|v| match v.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => v.parse().ok(),
        });
    match parsed {
        Some(n) if n > 0 => n,
        _ => lower::LOOP_LIMIT,
    }
}

/// A translated shader and what the host binds for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShader {
    pub stage: HwStage,
    /// A nonempty compute dispatch must have exactly one X group.
    pub dispatch_x_one: bool,
    /// The DXIL container, hashed as D3D12 expects.
    pub dxil: Vec<u8>,
    /// The user-data dwords the shader reads.
    pub user_data: Vec<UserData>,
    /// Immediate samplers: the four sampler words of `s<n>`.
    pub samplers: Vec<[u32; 4]>,
    /// The same samplers decoded, `s<n>` at position n: the static sampler
    /// descriptions the host's root signature declares.
    pub static_samplers: Vec<StaticSampler>,
    /// Root CBVs the shader reads: root parameter indices, each bound as
    /// `b<index>` in [`ROOT_CBV_SPACE`].
    pub root_cbvs: Vec<u8>,
    /// Root SRVs (raw buffers) the shader reads: `t<index>` in
    /// [`ROOT_CBV_SPACE`].
    pub root_srvs: Vec<u8>,
    /// Root UAVs (raw buffers) the shader reads or writes: `u<index>` in
    /// [`ROOT_CBV_SPACE`].
    pub root_uavs: Vec<u8>,
    /// Heap descriptors that can be selected from root state at bind time.
    pub root_indexed_views: Vec<crate::resource::RootIndexedView>,
}

impl HostShader {
    /// Size of the user-data buffer the shader declares, in dwords (a
    /// multiple of 4; 0 when it reads none). Root constants must cover it.
    pub fn user_data_dwords(&self) -> u32 {
        let n = self
            .user_data
            .iter()
            .map(|u| u.index + 1)
            .max()
            .unwrap_or(0);
        n.next_multiple_of(4)
    }
}

/// One dword of user data and the root parameter that fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserData {
    /// Dword in the user-data buffer (the console's user-data index).
    pub index: u32,
    /// Root parameter index in the title's root signature.
    pub slot: u8,
    /// `Constants` or `Table` (see the module notes for the value).
    pub root: RootType,
    /// Dword within the parameter.
    pub dword: u32,
}

/// What [`emit`] reports for a geometry program (the GS half of a merged
/// ES+GS stage, [`crate::merged`]): everything before emission handles it,
/// but a geometry program alone is not emitted: its input vertices and the
/// runtime table come from the ES half, so only a pair is
/// ([`translate_merged_gs`], `gs.rs`).
pub const GS_EMIT: &str = "GS emit";

/// Translates the vertex, pixel, compute or mesh program of a console
/// shader container.
///
/// A program refused because the first half of an interpolation is read as
/// data is translated once more with first halves evaluated as the pair
/// they start (`lower::with_first_half_pairs`), and only such a program:
/// with that on for every program, 8,551 pixel programs of the title that
/// already translated came out changed (an extra evaluation in each), for
/// the sake of 9 that did not translate.
pub fn translate(container: &[u8]) -> Result<HostShader, ShaderError> {
    match translate_plain(container) {
        Err(ShaderError::Unsupported { what, .. }) if what == lower::FIRST_HALF => {
            lower::with_first_half_pairs(|| translate_plain(container))
        }
        other => other,
    }
}

fn translate_plain(container: &[u8]) -> Result<HostShader, ShaderError> {
    let c = Container::parse(container)?;
    let part = crate::program_part(&c).ok_or(ShaderError::Malformed("no DXIL part"))?;
    let native = crate::native_programs(part)?
        .into_iter()
        .find(|p| {
            matches!(
                p.stage,
                HwStage::Vs | HwStage::Ps | HwStage::Cs | HwStage::Ms | HwStage::Gs
            )
        })
        .ok_or(ShaderError::Unsupported {
            offset: 0,
            what: "a container without a vertex, pixel, compute, mesh or geometry program",
        })?;
    translate_native(&c, &native, false)
}

/// The two halves of a merged geometry stage, from the vertex container the
/// ES program is in and the geometry container: paired
/// ([`crate::merged::pair_es_gs`]), the ES half traced, raised and
/// structured as the stage runs it (with the merged layout), and the record
/// it writes matched with the vertex reads of the GS half
/// ([`crate::handoff::handoff`]).
pub fn merged_halves(
    es: &[u8],
    gs: &[u8],
) -> Result<(crate::merged::MergedGs, crate::handoff::Handoff), ShaderError> {
    let stage_of = |bytes: &[u8], stage| -> Result<NativeProgram, ShaderError> {
        let c = Container::parse(bytes)?;
        let dxil = c
            .part(FourCC::DXIL)
            .ok_or(ShaderError::Malformed("no DXIL part"))?;
        crate::native_programs(dxil.data)?
            .into_iter()
            .find(|p| p.stage == stage)
            .ok_or(ShaderError::Unsupported {
                offset: 0,
                what: "a container without the ES or GS program of a merged stage",
            })
    };
    let merged = crate::merged::pair_es_gs(stage_of(es, HwStage::Es)?, stage_of(gs, HwStage::Gs)?)?;
    let es_native = NativeProgram {
        layout: merged.layout.clone(),
        ..merged.es.clone()
    };
    let es_ir = crate::spill::build(&es_native)?;
    let st = structure::structure(&es_ir);
    resource::track_merged(&es_ir, &es_native.layout, es_native.stage, st.as_ref().ok())?;
    io::raise(&es_ir, &es_native)?;
    st?;
    let gs_ir = crate::spill::build(&merged.gs)?;
    let handoff = crate::handoff::handoff(&es_ir, &gs_ir)?;
    Ok((merged, handoff))
}

/// The two host shaders of a merged geometry stage ([`translate_merged_gs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedShaders {
    /// The ES half as a vertex shader: its outputs are the records it
    /// writes, [`MergedShaders::record_dwords`] dwords of them, as
    /// `TEXCOORD` registers of four 32-bit integers.
    pub vertex: HostShader,
    /// The geometry shader (triangles in, triangle strip out): its inputs are
    /// the vertex shader's outputs, its outputs the geometry container's own.
    pub geometry: HostShader,
    pub record_dwords: u32,
    /// The programs the two were made from, over the merged layout: what a
    /// host pipeline needs of them besides the host code (the console
    /// resources they read, whether they store).
    pub vertex_native: NativeProgram,
    pub geometry_native: NativeProgram,
}

/// Translates a geometry program with the ES program it is linked with: the
/// pair is one merged stage ([`crate::merged`]), whose user SGPRs hold the
/// root parameters of both ([`merged_halves`]), and comes out as a vertex
/// shader and a geometry shader (`gs.rs`, with what it assumes).
pub fn translate_merged_gs(es: &[u8], gs: &[u8]) -> Result<MergedShaders, ShaderError> {
    let (merged, handoff) = merged_halves(es, gs)?;
    let record = gs::record_signature(handoff.es.dwords);
    // The ES half as a vertex shader.
    let es_ir0 = crate::spill::build(&merged.es)?;
    let es_native = gs::es_as_vertex_shader(&merged, &es_ir0, &handoff)?;
    let es_ir = crate::spill::build(&es_native)?;
    let st = structure::structure(&es_ir);
    let res = resource::track_merged(&es_ir, &es_native.layout, es_native.stage, st.as_ref().ok())?;
    let io = io::raise(&es_ir, &es_native)?;
    let st = st?;
    lower::take_needs();
    let (vertex, _) = emit_program_with(
        &Container::parse(es)?,
        &es_native,
        &es_ir,
        &st,
        &io,
        &res,
        Overrides {
            inputs: Some(gs::vertex_id_input()),
            outputs: Some(record.clone()),
            gs: None,
        },
    )?;
    // The GS half.
    let gc = Container::parse(gs)?;
    let native = merged.program();
    let p = crate::spill::build(&native)?;
    let st = structure::structure(&p);
    let res = resource::track_merged(&p, &native.layout, native.stage, st.as_ref().ok())?;
    let io = io::raise(&p, &native)?;
    let st = st?;
    let outs = match gc.part(FourCC::OSG1) {
        Some(part) => signature(part.data)?,
        None => Vec::new(),
    };
    let plan = gs::plan(&p, &handoff, &outs)?;
    lower::take_needs();
    let (geometry, _) = emit_program_with(
        &gc,
        &native,
        &p,
        &st,
        &io,
        &res,
        Overrides {
            inputs: Some(record),
            outputs: None,
            gs: Some(plan),
        },
    )?;
    Ok(MergedShaders {
        vertex,
        geometry,
        record_dwords: handoff.es.dwords,
        vertex_native: es_native,
        geometry_native: native,
    })
}

/// `merged`: the layout is the merged ES+GS stage's ([`resource::track_merged`]).
fn translate_native(c: &Container, native: &NativeProgram, merged: bool) -> Result<HostShader, ShaderError> {
    let p = crate::spill::build(native)?;
    // Structurisation first: tracing per-lane descriptors needs it (its
    // failure is reported after the earlier stages').
    let st = structure::structure(&p);
    let track = if merged { resource::track_merged } else { resource::track };
    let res = track(&p, &native.layout, native.stage, st.as_ref().ok())?;
    let io = io::raise(&p, native)?;
    let st = st?;
    emit(c, native, &p, &st, &io, &res)
}

thread_local! {
    /// [`with_wave_wide`]'s choice, for this thread's translations.
    static WAVE_WIDE_HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` (a translation) with the wave-wide form of a wave64 compute
/// program on a host of 32-lane waves allowed, as `VICTORY_WAVE_WIDE=1`
/// allows it for all: for programs known to come out of it right, while
/// others stay off it (two programs that moved to it lost the card, runs
/// 121-124). The form is still tried only after the exact ones, so a
/// program that has one keeps it.
pub fn with_wave_wide<R>(on: bool, f: impl FnOnce() -> R) -> R {
    let before = WAVE_WIDE_HERE.replace(on);
    let out = f();
    WAVE_WIDE_HERE.set(before);
    out
}

thread_local! {
    /// [`with_firstlane_own`]'s choice, for this thread's translations.
    static FIRSTLANE_OWN_HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` (a translation) with a first-lane read where not every lane runs
/// taken from the host wave's own first lane, as `VICTORY_FIRSTLANE_OWN=1`
/// takes it for all: exact where the value is the same in every lane of the
/// console wave, which holds for the programs it is asked for (the
/// ray-tracing structure builder's scans, which read a sum of scalars).
pub fn with_firstlane_own<R>(on: bool, f: impl FnOnce() -> R) -> R {
    let before = FIRSTLANE_OWN_HERE.replace(on);
    let out = f();
    FIRSTLANE_OWN_HERE.set(before);
    out
}

/// Whether [`with_firstlane_own`] asked for it on this thread.
pub(super) fn firstlane_own_here() -> bool {
    FIRSTLANE_OWN_HERE.get()
}

/// Emits a program whose earlier stages already ran.
pub fn emit(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
) -> Result<HostShader, ShaderError> {
    if native.stage == HwStage::Gs {
        return Err(ShaderError::Unsupported {
            offset: 0,
            what: GS_EMIT,
        });
    }
    let r = emit_any(c, native, p, st, io, res);
    if r.is_ok() && !permutes_outside_sections(p) {
        return r;
    }
    // A lane mask parked in two lanes of a VGPR, one of which holds a single scalar at other
    // times, came back as two scalars: once more with the program built so that it comes back
    // as the mask it is ([`ir::with_shared_slots`]). The original's error if that does not
    // translate either.
    if matches!(&r, Err(ShaderError::Unsupported { what, .. }) if *what == "an opaque lane mask") {
        let again = ir::with_shared_slots(|| crate::spill::build(native)).and_then(|q| {
            let st = structure::structure(&q)?;
            let io = io::raise(&q, native)?;
            let res = resource::track(&q, &native.layout, native.stage, Some(&st))?;
            emit_any(c, native, &q, &st, &io, &res)
        });
        if again.is_ok() {
            return again;
        }
    }
    // A whole-wave section that a jump splits (two reductions a branch chooses between that
    // share their end): once more with each such section in one block
    // ([`ir::whole_sections`]), where the simulation follows it. The original's result if that
    // does not translate. Tried for a program that translates too where it permutes lanes
    // outside the sections followed (the title's water optical depth: `v_permlanex16_b32`
    // before the jump): such a permute lowered on its own reads the lanes the host has as they
    // are, where the console's section reads every lane, a lane outside the saved mask with
    // the reduction's identity.
    let mut q = None;
    for _ in 0..4 {
        match ir::whole_sections(q.as_ref().unwrap_or(p)) {
            Some(next) => q = Some(next),
            None => break,
        }
    }
    let Some(q) = q else { return r };
    let again = (|| {
        let st = structure::structure(&q)?;
        let io = io::raise(&q, native)?;
        let res = resource::track(&q, &native.layout, native.stage, Some(&st))?;
        emit_any(c, native, &q, &st, &io, &res)
    })();
    match again {
        Ok(shader) => Ok(shader),
        Err(_) => r,
    }
}

/// Whether `p` has a `v_permlane16_b32` or `v_permlanex16_b32` that no whole-wave section the
/// simulation follows ([`wave::sections`]) holds.
fn permutes_outside_sections(p: &ir::Program) -> bool {
    let permute = |inst: &ir::Inst| matches!(inst.name(), "v_permlane16_b32" | "v_permlanex16_b32");
    p.blocks.iter().enumerate().any(|(b, block)| {
        if !block.insts.iter().any(permute) {
            return false;
        }
        let sections = wave::sections(p, b);
        let held = |i: usize| sections.iter().any(|(&start, s)| start < i && i < s.end);
        block.insts.iter().enumerate().any(|(i, inst)| permute(inst) && !held(i))
    })
}

/// [`emit`] of `p` as it is.
fn emit_any(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
) -> Result<HostShader, ShaderError> {
    let r = emit_forms(c, native, p, st, io, res);
    // A console wave64 compute program on a host of 32-lane waves that no form above gives
    // the host (it still asks for 64-lane waves, or does not translate): once more in the
    // paired form, each console wave run by one host wave (`paired`; `VICTORY_WAVE_PAIRS=0`
    // turns it off).
    let refused = match &r {
        Ok((_, wave)) => *wave == Some(64),
        Err(e) => matches!(e, ShaderError::Unsupported { .. }),
    };
    if refused
        && native.stage == HwStage::Cs
        && native.wave64
        && lower::host_max_wave() == 32
        && pairs_on()
    {
        let again = paired(c, native, p, io, res);
        if std::env::var_os("VICTORY_WAVE_WHY").is_some() {
            match &again {
                Ok((_, wave)) => eprintln!("WAVE64-RETRY paired lanes: wave {wave:?}"),
                Err(e) => eprintln!("WAVE64-RETRY paired lanes: {e}"),
            }
        }
        if let Ok((shader, _)) = again {
            return Ok(shader);
        }
    }
    r.map(|(shader, _)| shader)
}

/// Whether console wave64 compute programs may take the paired form ([`paired`]):
/// `VICTORY_WAVE_PAIRS=0` turns it off (a test can choose for its own thread).
fn pairs_on() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_PAIRS.get() {
        return on;
    }
    std::env::var_os("VICTORY_WAVE_PAIRS").is_none_or(|v| v != "0")
}

#[cfg(test)]
thread_local! {
    /// A test's choice for [`pairs_on`], for its thread only.
    static TEST_PAIRS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// [`emit`] in every form but the paired one, with the wave size the result requires of the
/// host; where an approximate form translates the program, the paired form instead if it
/// does.
fn emit_forms(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
) -> Result<(HostShader, Option<u32>), ShaderError> {
    // A pixel program that waits for the pixels it overlaps (POPS): the host orders its views
    // instead (`Shader::ordered`), and the program goes past the wait.
    if native.stage == HwStage::Ps
        && let Some(q) = ir::without_pops_wait(p)
    {
        let st = structure::structure(&q)?;
        let io = io::raise(&q, native)?;
        let res = resource::track(&q, &native.layout, native.stage, Some(&st))?;
        return emit_forms(c, native, &q, &st, &io, &res);
    }
    // (what an earlier program's lowering on this thread left behind)
    lower::take_needs();
    let original = emit_program(c, native, p, st, io, res);
    // A console wave64 compute program on a host of 32-lane waves that
    // still needs 64 lanes, or whose cross-lane code sits where not every
    // lane runs: once more with its early-outs left out
    // ([`ir::without_early_outs`]), which is what usually keeps the whole
    // body from being a block every lane runs.
    let stuck = match &original {
        Ok((_, wave)) => *wave == Some(64),
        Err(ShaderError::Unsupported { what, .. }) => lower::needs_every_lane(what),
        Err(_) => false,
    };
    // (a mask with lanes elsewhere is only helped by the exact forms: not by each half-wave
    // on its own)
    let exact_only = matches!(&original, Err(ShaderError::Unsupported { what, .. }) if *what == lower::MASK_ELSEWHERE);
    if stuck && native.stage == HwStage::Cs && native.wave64 && lower::host_max_wave() == 32 {
        let why = std::env::var_os("VICTORY_WAVE_WHY").is_some();
        let report = |what: &str, r: &Result<(HostShader, Option<u32>), ShaderError>| {
            if why {
                match r {
                    Ok((_, wave)) => eprintln!("WAVE64-RETRY {what}: wave {wave:?}"),
                    Err(e) => eprintln!("WAVE64-RETRY {what}: {e}"),
                }
            }
        };
        let done = |r: &Result<(HostShader, Option<u32>), ShaderError>| matches!(r, Ok((_, wave)) if *wave != Some(64));
        // Each host wave as a wave of its own (`lower::with_local_waves`),
        // for programs still stopped where not every lane runs.
        let local_on = std::env::var_os("VICTORY_WAVE_LOCAL").is_none_or(|v| v != "0");
        // Where the wave operations are the allocation idiom
        // ([`idiom::wave_local`]) that is exact. Other programs are taken
        // the same way last of all, each half-wave giving its own results
        // (approximate: a tile-wide result becomes two half-tile ones);
        // `VICTORY_WAVE_LOCAL=idiom` refuses those instead.
        let strict = std::env::var_os("VICTORY_WAVE_LOCAL").is_some_and(|v| v == "idiom");
        let idiom = |p: &ir::Program, st: &structure::Structured| match idiom::wave_local(p, st) {
            Ok(()) => true,
            Err((offset, what)) => {
                if why {
                    eprintln!("WAVE64-RETRY local waves: not the allocation idiom at {offset:#x}: {what}");
                }
                false
            }
        };
        // The program in two other forms: without its early-outs, and
        // without any branch that only skips code no lane is left for.
        let form = |p: Option<ir::Program>| {
            p.and_then(|p| {
                let st = structure::structure(&p).ok()?;
                let io = io::raise(&p, native).ok()?;
                let res = resource::track(&p, &native.layout, native.stage, Some(&st)).ok()?;
                Some((p, st, io, res))
            })
        };
        let forms = [
            ("without early-outs", form(ir::without_early_outs(p))),
            ("without skips", form(ir::without_skips(p, false))),
        ];
        for (name, f) in &forms {
            if let Some((p, st, io, res)) = f {
                let again = emit_program(c, native, p, st, io, res);
                report(name, &again);
                if done(&again) {
                    return again;
                }
            }
        }
        // Off unless asked for (`VICTORY_WAVE_WIDE=1`), for a console wave run as two host
        // waves: this form puts group barriers inside branches and loops, and runs 121-124
        // lost the card in a submission of two programs that had just moved to it
        // (`ptfx_CS_lightAtlas_Sprite` and `_Trail`). The 32-lane programs' form below, which
        // has no barrier, stays on.
        let wide_on = std::env::var_os("VICTORY_WAVE_WIDE").is_some_and(|v| v == "1") || WAVE_WIDE_HERE.get();
        // Where an approximate form below translates the program, the paired form
        // ([`paired`]) is tried first: it runs each console wave as one host wave and gives
        // what the console's 64 lanes compute, at no barrier but the entry's; the approximate
        // form is kept for a program the paired form does not take. Before, the paired form
        // was tried only where no form took the program, and 39 of the title's programs ran
        // approximate, each half-wave counting, ORing or electing over its own 32 lanes
        // (shadow-mask tiles and traces, temporal resolves, irradiance-cache and fallback
        // lighting, light clustering, scattering gathers, decal and fire resolves, the light
        // atlas, lens-flare scans). 36 now take the paired form; the other three (lens-flare
        // scans) were the allocation idiom with a multiply-add (`idiom::wave_local` now knows it)
        // and keep the per-half form, which is exact for them. Only these change: a program
        // an approximate form does not take goes on as before, and the paired form stays
        // last for it (`emit_any`). (`VICTORY_WAVE_PAIRS=0` turns the paired form off here
        // too.)
        let exactly = |approximate: Result<(HostShader, Option<u32>), ShaderError>| {
            if pairs_on() {
                let again = paired(c, native, p, io, res);
                report("paired lanes, before an approximate form", &again);
                if again.is_ok() {
                    return again;
                }
            }
            approximate
        };
        // exact first: each host wave on its own where that is exact (the idiom, every
        // form; no barriers), then the other exact forms; then approximate
        for exact in [true, false] {
            if !exact {
                // Before anything approximate, the exact forms that cost group barriers.
                // Every skip left out that the code after it cannot see
                // ([`ir::without_skips`], `deep`): most of the body is then code every
                // lane runs, where the two host waves can exchange.
                // (`VICTORY_WAVE_DEEP_SKIPS=0` and `VICTORY_WAVE_WIDE=0` turn the two off.)
                let deep_on = std::env::var_os("VICTORY_WAVE_DEEP_SKIPS").is_none_or(|v| v != "0");
                let deep = if deep_on { form(ir::without_skips(p, true)) } else { None };
                if let Some((p, st, io, res)) = &deep {
                    let again = emit_program(c, native, p, st, io, res);
                    report("without skips, scalars followed", &again);
                    if done(&again) {
                        return again;
                    }
                }
                // Still stopped where not every lane runs: the per-lane branches around
                // those places taken as the console takes them, by the whole wave at once
                // ([`widen`]), at two group barriers per such branch.
                if wide_on {
                    let bases = [
                        deep.as_ref().map(|f| &f.0),
                        forms[1].1.as_ref().map(|f| &f.0),
                        forms[0].1.as_ref().map(|f| &f.0),
                        Some(p),
                    ];
                    if let Some(base) = bases.into_iter().flatten().next() {
                        let again = widen(c, native, base, why);
                        report("wave-wide branches", &again);
                        if done(&again) {
                            return again;
                        }
                    }
                }
                if strict {
                    break;
                }
            }
            if !local_on || exact_only {
                continue;
            }
            for (name, f) in &forms {
                if let Some((p, st, io, res)) = f
                    && idiom(p, st) == exact
                {
                    let local = lower::with_local_waves(|| emit_program(c, native, p, st, io, res));
                    report(&format!("local waves ({}), {name}", if exact { "idiom" } else { "APPROXIMATE" }), &local);
                    if done(&local) {
                        return if exact { local } else { exactly(local) };
                    }
                }
            }
            if idiom(p, st) == exact {
                let local = lower::with_local_waves(|| emit_program(c, native, p, st, io, res));
                report(if exact { "local waves (idiom)" } else { "local waves (APPROXIMATE)" }, &local);
                if done(&local) {
                    return if exact { local } else { exactly(local) };
                }
            }
        }
        if local_on && !exact_only {
            // Last: every mask and lane read per host wave.
            if !strict {
                for (name, f) in &forms {
                    if let Some((p, st, io, res)) = f {
                        let local = lower::with_local_waves_any(|| emit_program(c, native, p, st, io, res));
                        report(&format!("local waves (APPROXIMATE, any mask), {name}"), &local);
                        if done(&local) {
                            return exactly(local);
                        }
                    }
                }
                let local = lower::with_local_waves_any(|| emit_program(c, native, p, st, io, res));
                report("local waves (APPROXIMATE, any mask)", &local);
                if done(&local) {
                    return exactly(local);
                }
            }
        }
        // Last of all (`VICTORY_WAVE_LDS_LOOSE=1` turns it on): LDS read and written inside
        // one lane-branch region, ordered between the two host waves only where the region
        // is entered and left (`lower::with_loose_lds`, approximate). A 32-lane host refuses
        // the 64-lane form outright, which drops every dispatch of the program.
        // Flamingo: off by default. In run 54 the three programs it let run
        // made the world grainy and smeared (refused, their passes were left
        // out and the picture was clean).
        let loose_on = std::env::var_os("VICTORY_WAVE_LDS_LOOSE").is_some_and(|v| v == "1");
        if loose_on && !strict {
            let bases = [forms[0].1.as_ref(), forms[1].1.as_ref()];
            for (name, f) in [("without early-outs", bases[0]), ("without skips", bases[1])] {
                if let Some((p, st, io, res)) = f {
                    let again = lower::with_loose_lds(|| emit_program(c, native, p, st, io, res));
                    report(&format!("loose LDS (APPROXIMATE), {name}"), &again);
                    if done(&again) {
                        return again;
                    }
                }
            }
            let again = lower::with_loose_lds(|| emit_program(c, native, p, st, io, res));
            report("loose LDS (APPROXIMATE)", &again);
            if done(&again) {
                return again;
            }
            if local_on && !exact_only {
                let local = lower::with_loose_lds(|| lower::with_local_waves_any(|| emit_program(c, native, p, st, io, res)));
                report("loose LDS + local waves (APPROXIMATE, any mask)", &local);
                if done(&local) {
                    return local;
                }
            }
        }
    }
    // SCC of a mask operation joined with a scalar test behind a per-lane skip (the 32-lane
    // trace programs): right once the wave takes that skip together ([`widen`]; the
    // lowering has named the skipped blocks).
    let original = match original {
        Err(ShaderError::Unsupported { what, .. })
            if native.stage == HwStage::Cs
                && what == lower::LANE_MASK
                && std::env::var_os("VICTORY_WAVE_WIDE").is_none_or(|v| v != "0") =>
        {
            let again = widen(c, native, p, std::env::var_os("VICTORY_WAVE_WHY").is_some());
            match again {
                Ok(_) => again,
                Err(_) => emit_program(c, native, p, st, io, res),
            }
        }
        other => other,
    };
    // A descriptor joined from what each way into a block brings, where a way brings what no
    // heap index stands for (the title's terrain programs pick an immediate sampler that
    // clamps or wraps by a uniform condition, each arm setting its own constant): once more
    // with that block copied for each way in ([`split_choices`]).
    let original = match original {
        Err(ShaderError::Unsupported { what, .. }) if what == lower::CHOSEN => {
            split_choices(c, native, p, res).unwrap_or(original)
        }
        other => other,
    };
    // Retry unsupported programs only. Threading can separate an unknown
    // register value from its apparent use on a mutually exclusive path.
    if !matches!(original, Err(ShaderError::Unsupported { .. })) {
        return original;
    }
    let edges = threading::constant_edges(p, st);
    if edges.is_empty() {
        return original;
    }
    let p = ir::thread_edges(p, &edges)?;
    let st = structure::structure(&p)?;
    let io = io::raise(&p, native)?;
    let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
    emit_program(c, native, &p, &st, &io, &res)
}

/// `p` translated with the joins of its descriptors chosen by control flow copied for each way
/// in ([`ir::split_joins`]), so that each copy holds one descriptor; again while the choice
/// moves on to a later join (a use further down), a few times at most. `None` unless a form
/// translates.
fn split_choices(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    res: &resource::Resources,
) -> Option<Result<(HostShader, Option<u32>), ShaderError>> {
    /// The joins where a descriptor is chosen from what a heap index cannot stand for: a way
    /// in that brings neither a loaded descriptor nor an earlier join's choice.
    fn joins(res: &resource::Resources) -> Vec<usize> {
        fn walk(s: &resource::Source, out: &mut Vec<usize>) {
            use resource::Source;
            if let Source::Phi { block, args } = s {
                let other = |a: &Source| !matches!(a, Source::Loaded { .. } | Source::Phi { .. });
                if args.iter().any(other) && !out.contains(block) {
                    out.push(*block);
                }
                for a in args {
                    walk(a, out);
                }
            }
        }
        let mut out = Vec::new();
        for d in &res.descriptors {
            walk(&d.source, &mut out);
        }
        out
    }
    let mut form = ir::split_joins(p, &joins(res))?;
    for _ in 0..4 {
        let st = structure::structure(&form).ok()?;
        let io = io::raise(&form, native).ok()?;
        let res = resource::track(&form, &native.layout, native.stage, Some(&st)).ok()?;
        match emit_program(c, native, &form, &st, &io, &res) {
            Err(ShaderError::Unsupported { what, .. }) if what == lower::CHOSEN => {
                form = ir::split_joins(&form, &joins(&res))?;
            }
            Ok(done) => return Some(Ok(done)),
            Err(_) => return None,
        }
    }
    None
}

/// `p` (a console wave64 compute program on a host of 32-lane waves) with the per-lane
/// branches around its cross-lane code taken wave-wide ([`structure::structure_wide`]): the
/// lowering names the blocks where the two host waves must meet and could not
/// (`lower::take_needs`), the branches whose regions hold them are widened, and so on until
/// it translates or a branch cannot be widened. Only where a thread group is one console
/// wave: with several, the waves of a group would decide differently and a barrier inside
/// such a branch would wait for threads that never come.
fn widen(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    why: bool,
) -> Result<(HostShader, Option<u32>), ShaderError> {
    const NOT: ShaderError = ShaderError::Unsupported {
        offset: 0,
        what: "wave-wide branches in a thread group of several console waves",
    };
    match native.io {
        // (a program whose waves are the host's meets no one: no barrier, any group)
        crate::native::StageIo::Cs(_) if !(native.wave64 && lower::host_max_wave() == 32) => {}
        crate::native::StageIo::Cs(cs) if cs.threads.iter().product::<u32>() == 64 => {}
        crate::native::StageIo::Cs(cs) => {
            if why {
                eprintln!("WAVE64-WIDE not tried: thread groups of {:?}", cs.threads);
            }
            return Err(NOT);
        }
        _ => return Err(NOT),
    }
    let mut wide = std::collections::HashSet::new();
    loop {
        let st = structure::structure_wide(p, &wide)?;
        let io = io::raise(p, native)?;
        let res = resource::track(p, &native.layout, native.stage, Some(&st))?;
        lower::take_needs();
        let r = emit_program(c, native, p, &st, &io, &res);
        let needs = lower::take_needs();
        // (or translated for 64-lane waves still: shared memory read and written where not
        // every lane runs, whose blocks the lowering names too)
        let stuck = matches!(&r, Err(ShaderError::Unsupported { what, .. })
                if lower::needs_every_lane(what) || *what == lower::LANE_MASK && !needs.is_empty())
            || matches!(&r, Ok((_, Some(64)))) && !needs.is_empty();
        if !stuck {
            if why {
                eprintln!("WAVE64-WIDE {} branches taken wave-wide", wide.len());
            }
            return r;
        }
        let more: Vec<usize> = lower::branches_around(&st, &needs)
            .unwrap_or_default()
            .into_iter()
            .filter(|b| !wide.contains(b))
            .collect();
        if more.is_empty() {
            if why {
                eprintln!("WAVE64-WIDE stopped with {} branches wave-wide: a branch around it cannot be", wide.len());
            }
            return r;
        }
        wide.extend(more);
    }
}

/// `p` (a console wave64 compute program on a host of 32-lane waves) in the paired form
/// ([`lower::with_paired_lanes`]): each console wave run by one host wave of 32 lanes, each
/// thread two of its lanes, with every per-lane branch taken by the whole wave as the console
/// takes it ([`structure::structure_wide`]), so that a host wave's threads go the same way and
/// EXEC stays each lane's guard. The program's barriers are reached by whole host waves where
/// the console's waves reach them; the only barriers added are on entry, before the program,
/// where a group of several console waves numbers its waves (`Lower::wave_base`). A thread
/// group whose console waves are not full would need the lanes past its end kept out of every
/// effect: refused.
/// Descriptors and addresses stay those traced on the lanes' own paths (`res`, as for every
/// other form): the per-lane form rests on a lane that reads a scalar after a join having
/// come the way the wave did, and knowing less of a lane's way the wave-wide form would miss
/// some of them (a sampler set in a skip that only a wave with no lane left for it skips).
fn paired(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    io: &io::Io,
    res: &resource::Resources,
) -> Result<(HostShader, Option<u32>), ShaderError> {
    let crate::native::StageIo::Cs(cs) = native.io else {
        return Err(ShaderError::Unsupported {
            offset: 0,
            what: "paired lanes outside compute",
        });
    };
    if cs.threads.iter().product::<u32>() % 64 != 0 {
        return Err(ShaderError::Unsupported {
            offset: 0,
            what: "paired lanes in a thread group that does not fill its console waves",
        });
    }
    // every per-lane branch the wave's; again while that leaves another branch per lane
    let mut wide = std::collections::HashSet::new();
    let st = loop {
        let st = structure::structure_wide(p, &wide)?;
        let lanes: Vec<usize> = (0..st.exits.len())
            .filter(|b| !wide.contains(b))
            .filter(|&b| {
                matches!(st.exits[b], structure::LaneExit::Branch { when, .. }
                    if !st.bools.support(when).iter().all(|a| matches!(a, structure::Atom::Uniform(_))))
            })
            .collect();
        if lanes.is_empty() {
            break st;
        }
        wide.extend(lanes);
    };
    lower::with_paired_lanes(|| emit_program(c, native, p, &st, io, res))
}

/// The translated program, and the wave size it requires of the host.
fn emit_program(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
) -> Result<(HostShader, Option<u32>), ShaderError> {
    emit_program_with(c, native, p, st, io, res, Overrides::default())
}

/// What a half of a merged stage takes in place of the container's: the
/// signatures it declares and, for the geometry half, its plan.
#[derive(Default)]
struct Overrides {
    inputs: Option<Vec<container::SigElement>>,
    outputs: Option<Vec<container::SigElement>>,
    gs: Option<gs::GsPlan>,
}

fn emit_program_with(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
    over: Overrides,
) -> Result<(HostShader, Option<u32>), ShaderError> {
    let sig = |fourcc| -> Result<Vec<container::SigElement>, ShaderError> {
        match c.part(fourcc) {
            Some(part) => signature(part.data),
            None => Ok(Vec::new()),
        }
    };
    let pixel = native.stage == HwStage::Ps;
    let mut inputs = match over.inputs {
        Some(i) => i,
        None => sig(FourCC::ISG1)?,
    };
    let mut outputs = match over.outputs {
        Some(o) => o,
        None => sig(FourCC::OSG1)?,
    };
    // Interpolation as DXC declares it by default; pixel inputs that are
    // read get the mode their barycentrics say.
    let interpolated = if pixel { &mut inputs } else { &mut outputs };
    for e in interpolated {
        e.interp = match (e.kind, e.comp) {
            (3, _) => 4,
            // Constant, as DXC declares it (the AMD compiler crashed on a
            // front face without a mode).
            (13, _) => 1,
            // Until a read says which mode (`lower`).
            (28, _) => 2,
            (0 | 6 | 7, 9) => 2,
            (0, _) => 1,
            _ => 0,
        };
    }
    // With a conservative depth output (SV_DepthLessEqual, GreaterEqual) DXIL
    // wants the position centroid (or per sample), as DXC declares it then.
    if pixel && outputs.iter().any(|e| matches!(e.kind, 18 | 19)) {
        for e in inputs.iter_mut().filter(|e| e.kind == 3) {
            e.interp = 5;
        }
    }
    // Per-primitive outputs are constant. The console never exports a
    // primitive id or cull flag a mesh shader writes (it culls with the
    // primitive export), so those rows are left out.
    let mut prims = sig(FourCC::PSG1)?;
    prims.retain(|e| !matches!(e.kind, 10 | 30));
    for e in &mut prims {
        e.interp = 1;
    }
    // A mesh group's id arrives multiplied by max(threads, vertices,
    // primitives) (the code divides by that); the thread count is in PSV0
    // from runtime info version 2 on.
    // shortcut: older containers fall back to the group's lane count, right
    // when the HLSL group fills its waves (the title's do).
    let mesh_stride = match &native.io {
        crate::native::StageIo::Ms(ms) => {
            let psv = c.part(FourCC::PSV0).map(|p| p.data);
            let threads = psv
                .filter(|d| crate::dxbc::u32_at(d, 0).is_ok_and(|n| n >= 48))
                .and_then(|d| crate::dxbc::u32_at(d, 4 + 36).ok());
            threads.map_or(ms.threads, |t| {
                t.max(ms.max_vertices).max(ms.max_primitives)
            })
        }
        _ => 0,
    };
    let l = lower::lower(&lower::Parts {
        native,
        p,
        st,
        io,
        res,
        inputs,
        outputs,
        prims,
        mesh_stride,
        gs: over.gs,
    })?;
    let wave_size = l.wave_size;
    let shader = HostShader {
        stage: native.stage,
        dispatch_x_one: l.dispatch_x_one,
        dxil: l.dxil,
        user_data: l.user_data,
        static_samplers: l
            .samplers
            .iter()
            .enumerate()
            .map(|(n, &w)| {
                let mut s = StaticSampler::from_words(w);
                if l.cmp_samplers >> n & 1 != 0 {
                    s.filter |= 0x80;
                }
                s
            })
            .collect(),
        samplers: l.samplers,
        root_cbvs: l.root_cbvs,
        root_srvs: l.root_srvs,
        root_uavs: l.root_uavs,
        root_indexed_views: res.root_indexed_views.clone(),
    };
    Ok((shader, wave_size))
}

/// The rows of an `ISG1`/`OSG1` part.
fn signature(part: &[u8]) -> Result<Vec<container::SigElement>, ShaderError> {
    use crate::dxbc::u32_at;
    let count = u32_at(part, 0)? as usize;
    let base = u32_at(part, 4)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + 32 * i;
        let name_at = u32_at(part, at + 4)? as usize;
        let name = part
            .get(name_at..)
            .and_then(|s| s.split(|&b| b == 0).next())
            .ok_or(ShaderError::Malformed("signature name out of bounds"))?;
        let mask = *part
            .get(at + 24)
            .ok_or(ShaderError::Malformed("truncated"))?;
        if u32_at(part, at + 28)? != 0 {
            return Err(ShaderError::Unsupported {
                offset: 0,
                what: "a minimum-precision signature element",
            });
        }
        if u32_at(part, at)? != 0 {
            return Err(ShaderError::Unsupported {
                offset: 0,
                what: "a signature element of another stream",
            });
        }
        let kind = match u32_at(part, at + 12)? {
            0 => 0,
            1 => 3,
            2 => 6,
            3 => 7,
            4 => 4,
            5 => 5,
            6 => 1,
            7 => 10,
            8 => 2,
            9 => 13,
            10 => 12,
            64 => 16,
            65 => 17,
            66 => 14,
            67 => 19,
            68 => 18,
            69 => 20,
            70 => 15,
            23 => 28, // barycentrics
            25 => 30, // cull primitive
            _ => {
                return Err(ShaderError::Unsupported {
                    offset: 0,
                    what: "a system value in the signature",
                });
            }
        };
        // 16-bit elements are declared 32-bit; reading or writing one
        // converts (16-bit values travel packed on the console).
        let raw = u32_at(part, at + 16)?;
        let comp = match (kind, raw) {
            // DXIL declares the front face a uint (the validator insists).
            (13, _) => 5,
            (_, 1 | 4) => 5,
            (_, 2 | 5) => 4,
            (_, 3 | 6) => 9,
            (_, c) => {
                return Err(ShaderError::Unsupported {
                    offset: 0,
                    what: match c {
                        0 => "a signature element of unknown type",
                        _ => "a 64-bit signature element",
                    },
                });
            }
        };
        let register = u32_at(part, at + 20)?;
        out.push(container::SigElement {
            name: String::from_utf8_lossy(name).into_owned(),
            index: u32_at(part, at + 8)?,
            kind,
            comp,
            interp: 0,
            row: (register != u32::MAX).then_some(register),
            col: mask.trailing_zeros().min(3) as u8,
            cols: mask.count_ones().max(1) as u8,
            usage: 0,
            half: (4..=6).contains(&raw),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::bitcode::Attr;
    use super::lower::LOOP_LIMIT;
    use super::parse_loop_limit;

    #[test]
    fn the_clean_up_switch_parses_or_runs_every_pass() {
        let all = super::bitcode::opt::ALL;
        assert_eq!(parse_shader_opt(None), all);
        assert_eq!(parse_shader_opt(Some("0")), 0);
        assert_eq!(parse_shader_opt(Some(" 1 ")), 1);
        assert_eq!(parse_shader_opt(Some("0x1")), 1);
        assert_eq!(parse_shader_opt(Some("0xffffffff")), all);
        assert_eq!(parse_shader_opt(Some("on")), all);
    }

    #[test]
    fn the_loop_limit_switch_parses_or_falls_back() {
        assert_eq!(parse_loop_limit(None), LOOP_LIMIT);
        assert_eq!(parse_loop_limit(Some("4096")), 4096);
        assert_eq!(parse_loop_limit(Some(" 0x1000 ")), 4096);
        assert_eq!(parse_loop_limit(Some("1")), 1);
        for bad in ["", "0", "-5", "lots", "0x", "4294967296"] {
            assert_eq!(parse_loop_limit(Some(bad)), LOOP_LIMIT, "{bad:?}");
        }
    }
    use super::container::SigElement;
    use super::shader::Shader;
    use super::*;
    use crate::native::{ComputeIo, StageIo};

    #[test]
    fn lane_selectors_wrap_at_the_console_wave_width() {
        use crate::native::{ResourceType, RootParam, UsedResource};

        let shader = |wave64, host, selector: u32| {
            let width = if wave64 { 64 } else { 32 };
            let lane = (128 + selector) << 9;
            let words = [
                0xBE85_0380u32, // s_mov_b32 s5, 0
                0xF408_0202,
                0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0
                // An ordinary vector move prevents spill replacement and gives both shaders the
                // same bitcode constant pool before the lane operations.
                0x7E12_0200 | (128 + width), // v_mov_b32 v9, width
                // Seed lane 32 in the constant pool even when testing selector 0 on wave64.
                0xBE86_0380 | if wave64 { 32 } else { 5 }, // s_mov_b32 s6, payload
                0xD761_0009,
                lane | 6, // v_writelane_b32 v9, s6, selector
                0xD760_0007,
                lane | 0x109, // v_readlane_b32 s7, v9, selector
                0x7E02_0207,  // v_mov_b32 v1, s7
                0xE070_0000,
                0x8002_0100, // buffer_store_dword v1, off, s[8:11], 0
                0xBF81_0000, // s_endpgm
            ];
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64,
                vgprs: 16,
                layout: vec![
                    RootParam {
                        root: RootType::Constants,
                        table_offset_dwords: 0,
                        sgprs: 4,
                        base_register: 0,
                        user_data: 0,
                        slot: 0,
                    },
                    RootParam {
                        root: RootType::Table,
                        table_offset_dwords: 0,
                        sgprs: 1,
                        base_register: 0,
                        user_data: 4,
                        slot: 1,
                    },
                ],
                used: vec![UsedResource {
                    kind: ResourceType::Uav,
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    lower_bound: 0,
                    stride: 0,
                    dwords: 4,
                    user_data: 4,
                    slot: 1,
                }],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 5 << 1,
                    threads: [if wave64 { 64 } else { 32 }, 1, 1],
                }),
                code: words.iter().flat_map(|word| word.to_le_bytes()).collect(),
            };
            let bytes = crate::dxbc::build(&[]);
            let container = Container::parse(&bytes).unwrap();
            let p = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
            let st = structure::structure(&p).unwrap();
            let io = io::raise(&p, &native).unwrap();
            let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let was = lower::TEST_HOST_WAVE.replace(Some(host));
            let result = emit(&container, &native, &p, &st, &io, &res);
            lower::TEST_HOST_WAVE.set(was);
            result.unwrap().dxil
        };

        for (wave64, host, width) in [(false, 32, 32), (true, 64, 64), (true, 32, 64)] {
            assert!(
                shader(wave64, host, width) == shader(wave64, host, 0),
                "guest {width}, host {host}"
            );
        }
        assert!(
            shader(true, 32, 32) != shader(true, 32, 0),
            "keep the wave64 upper half on a wave32 host"
        );
    }

    /// A pixel shader writing SV_StencilRef (channel y of `exp mrtz`).
    /// DXC sets ShaderFlags.StencilRef (bit 11) for a pixel shader whose
    /// output signature has SV_StencilRef, and the feature bit
    /// D3D_SHADER_FEATURE_STENCIL_REF (0x200) in SFI0; the validator checks
    /// both against the module.
    #[test]
    fn a_stencil_export_declares_the_stencil_ref_feature() {
        let words = [
            0x7E00_02F2u32, // v_mov_b32 v0, 1.0
            0x7E02_0285,    // v_mov_b32 v1, 5
            0xF800_1883,
            0x0000_0100, // exp mrtz v0, v1, off, off done vm
            0xBF81_0000,
        ];
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 2,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let el = |name: &str, kind, comp| SigElement {
            name: name.into(),
            index: 0,
            kind,
            comp,
            interp: 0,
            row: None,
            col: 0,
            cols: 1,
            usage: 1,
            half: false,
        };
        let osg1 =
            container::signature(&[el("SV_Depth", 17, 9), el("SV_StencilRef", 20, 5)], false);
        let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let out = emit(&c, &native, &p, &st, &io, &res).unwrap();
        // dxil.dll, where there is one, checks the flags against the module.
        #[cfg(windows)]
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut out.dxil.clone()).unwrap();
        }
        let host = Container::parse(&out.dxil).unwrap();
        let sfi0 = host.part(FourCC::SFI0).unwrap().data;
        let features = u64::from_le_bytes(sfi0.try_into().unwrap());
        assert_ne!(features & 0x200, 0, "SFI0 {features:#x}: no StencilRef");
    }

    /// A wave64 compute program that writes only EXEC's high dword
    /// (`s_mov_b32 exec_hi, 0`) before a vector instruction. The new mask was
    /// opaque and keyed by the unchanged low dword, so building its lane bit
    /// needed the mask's own value: lowering recursed until the stack
    /// overflowed, which takes the whole process down (found by
    /// random-input testing). Now that low dword, seen with two high
    /// dwords, has no mask, and the instruction fails like any other that
    /// cannot be expressed: its result is missing, and only a side effect
    /// that uses it would stop the translation.
    #[test]
    fn an_exec_high_write_does_not_overflow_the_stack() {
        // s_mov_b32 exec_hi, 0; v_mov_b32 v1, v0; s_endpgm
        let words = [0xBEFF_0380u32, 0x7E02_0300, 0xBF81_0000];
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: true,
            vgprs: 2,
            layout: vec![],
            used: vec![],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 0,
                threads: [64, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let container = crate::dxbc::build(&[]);
        let c = Container::parse(&container).unwrap();
        if let Err(e) = emit(&c, &native, &p, &st, &io, &res) {
            assert!(e.to_string().contains("its own value"), "{e}");
        }
    }

    /// A wave64 compute program: `body`, then a store of v1 to a UAV.
    fn stores_v1(body: &[u32]) -> Result<(), ShaderError> {
        use crate::native::RootParam;
        use crate::{ResourceType, RootType, UsedResource};
        // s_mov_b32 s1, 0; s_load_dwordx4 s[8:11], s[0:1], 0
        let mut words = vec![0xBE81_0380, 0xF408_0200, 0xFA00_0000];
        words.extend_from_slice(body);
        // v_lshlrev_b32 v3, 2, v0; buffer_store_dword v1, v3, s[8:11], 0 offen;
        // s_endpgm
        words.extend_from_slice(&[0x3406_0082, 0xE070_1000, 0x8002_0103, 0xBF81_0000]);
        let native = NativeProgram {
            stage: HwStage::Cs,
            wave64: true,
            vgprs: 4,
            layout: vec![RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 0,
                slot: 0,
            }],
            used: vec![UsedResource {
                kind: ResourceType::Uav,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 0,
                dwords: 4,
                user_data: 0,
                slot: 0,
            }],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 2,
                threads: [64, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, None).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        let container = crate::dxbc::build(&[]);
        let c = Container::parse(&container).unwrap();
        emit(&c, &native, &p, &st, &io, &res).map(|_| ())
    }

    /// The high dword of a wave64 mask rewritten alone (`s_mov_b32 vcc_hi`
    /// or `exec_hi`): lanes 32-63 took the mask the low dword was first
    /// written with (the compare's, EXEC's on entry). Now the store refuses;
    /// without the write it translates.
    #[test]
    fn a_mask_whose_high_dword_is_rewritten_is_refused() {
        // v_cmp_eq_u32 vcc, 0, v0; v_mov_b32 v2, 11; v_cndmask_b32 v1, 0, v2, vcc
        let select = [0x7D84_0080, 0x7E04_028B, 0x0202_0480];
        // The same with v_add_co_ci_u32 v1, vcc, 0, v2, vcc.
        let carry = [0x7D84_0080, 0x7E04_028B, 0x5002_0480];
        // v_mov_b32 v1, 7
        let mov = [0x7E02_0287];
        for body in [&select[..], &carry, &mov] {
            stores_v1(body).unwrap();
        }
        // s_mov_b32 vcc_hi, 0 after the compare.
        let select_hi = [0x7D84_0080, 0xBEEB_0380, 0x7E04_028B, 0x0202_0480];
        let carry_hi = [0x7D84_0080, 0xBEEB_0380, 0x7E04_028B, 0x5002_0480];
        // s_mov_b32 exec_hi, 0; v_mov_b32 v1, 7, then the same after
        // s_mov_b64 s[2:3], exec.
        let exec_hi = [0xBEFF_0380, 0x7E02_0287];
        let exec_hi_saved = [0xBE82_047E, 0xBEFF_0380, 0x7E02_0287];
        for body in [&select_hi[..], &carry_hi, &exec_hi, &exec_hi_saved] {
            assert!(stores_v1(body).is_err(), "{body:x?}");
        }
    }

    /// `ds_bpermute_b32` stores nothing: each lane reads the lane its address
    /// names. It stopped as "LDS the program does not allocate" (sticky lips)
    /// or "an LDS operation" (the checkerboard shadow masks).
    #[test]
    fn a_backward_permute_reads_the_lane_its_address_names() {
        // v_mov_b32 v2, 11; ds_bpermute_b32 v1, v0, v2
        stores_v1(&[0x7E04_028B, 0xDACC_0000, 0x0100_0200]).unwrap();
    }

    /// A carry-in reading a join of compares after a divergent branch. A
    /// lane that skipped the inner compare has VCC clear there (VOPC clears
    /// inactive lanes), not the first compare's bit, so the join is no
    /// per-lane phi: lowering refuses it.
    #[test]
    fn a_carry_in_from_a_divergent_join_is_refused() {
        let body = [
            0x7E04_028B, // v_mov_b32 v2, 11
            0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
            0xBE82_376A, // s_andn1_saveexec_b64 s[2:3], vcc
            0xBF88_0001, // s_cbranch_execz 1
            0x7D84_0081, // v_cmp_eq_u32 vcc, 1, v0
            0x88FE_027E, // s_or_b64 exec, exec, s[2:3]
            0x5002_0480, // v_add_co_ci_u32 v1, vcc, 0, v2, vcc
        ];
        let e = stores_v1(&body).unwrap_err().to_string();
        assert!(e.contains("a carry-in from an untracked mask"), "{e}");
    }

    /// A pixel shader writing a constant colour.
    #[cfg(windows)]
    fn constant_color() -> Vec<u8> {
        let mut s = Shader::new(true);
        s.outputs.push(SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 15,
            half: false,
        });
        let b = s.entry;
        for (c, v) in [1.0f32, 0.5, 0.25, 1.0].into_iter().enumerate() {
            let args = [
                s.m.i32(0),
                s.m.i32(0),
                s.m.i8(c as u8),
                s.m.f32(v.to_bits()),
            ];
            s.op(b, "storeOutput.f32", Attr::NoUnwind, s.t.void, 5, &args);
        }
        s.m.terminate(b, super::bitcode::Term::Ret);
        s.finish()
    }

    #[test]
    #[cfg(windows)]
    fn stub_pipeline_is_created() {
        let Ok(host) = super::host::Host::new(false) else {
            eprintln!("no SM 6.6 device; skipped");
            return;
        };
        let ps = super::HostShader {
            dispatch_x_one: false,
            stage: crate::HwStage::Ps,
            dxil: constant_color(),
            user_data: Vec::new(),
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: Vec::new(),
            root_srvs: Vec::new(),
            root_uavs: Vec::new(),
            root_indexed_views: Vec::new(),
        };
        let file = |var: &str, stage| {
            std::env::var(var).ok().map(|p| super::HostShader {
                dispatch_x_one: false,
                stage,
                dxil: std::fs::read(p).unwrap(),
                user_data: Vec::new(),
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: Vec::new(),
                root_srvs: Vec::new(),
                root_uavs: Vec::new(),
                root_indexed_views: Vec::new(),
            })
        };
        let ps = file("VICTORY_REF_PS", crate::HwStage::Ps).unwrap_or(ps);
        let vs = file("VICTORY_REF_VS", crate::HwStage::Vs)
            .unwrap_or_else(|| super::host::stub_vs(&ps).unwrap());
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut vs.dxil.clone()).unwrap();
        }
        host.pipeline(&vs, &ps).unwrap();
        // The runtime checks the container hash: no hash or a wrong one is
        // refused. (Recent runtimes also take sixteen 0x01 bytes, a bypass
        // value; ours works with older ones too.)
        let mut wrong = ps.dxil[4..20].to_vec();
        wrong[0] ^= 1;
        for digest in [&[0u8; 16][..], &wrong] {
            let mut other = ps.clone();
            other.dxil[4..20].copy_from_slice(digest);
            assert!(
                host.pipeline(&vs, &other).is_err(),
                "digest {digest:02x?} accepted"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn constant_color_validates() {
        let mut c = constant_color();
        let path = super::validator::Validator::default_path();
        let Ok(mut v) = super::validator::Validator::load(path.as_deref()) else {
            eprintln!("no dxil.dll; skipped");
            return;
        };
        let ours = c[4..20].to_vec();
        v.validate(&mut c).unwrap();
        assert_eq!(c[4..20], ours[..], "our hash differs from the validator's");
    }

    /// Triangle (0, 1, 2), each corner's 9-bit index in its 10-bit field.
    const TRIANGLE: u32 = 1 << 10 | 2 << 20;
    /// The edge flags of the three corners (bits 9, 19 and 29).
    const EDGE_FLAGS: u32 = 1 << 9 | 1 << 19 | 1 << 29;
    /// The culling bit.
    const CULL: u32 = 1 << 31;

    /// A one-wave (32-lane) triangle mesh program whose lanes all export
    /// primitive `prim` and, as vertex `lane`, the position
    /// (lane & 1 ? 3 : -1, lane & 2 ? 3 : -1, 0.5, 1): triangle (0, 1, 2)
    /// covers the whole target.
    fn triangle_mesh(prim: u32) -> Result<HostShader, ShaderError> {
        let words = [
            0xD765_0001u32, // v_mbcnt_lo_u32_b32 v1, -1, 0
            0x0001_00C1,
            0x3604_0281, // v_and_b32 v2, 1, v1
            0x3606_0282, // v_and_b32 v3, 2, v1
            0x7E04_0D02, // v_cvt_f32_u32 v2, v2
            0x7E06_0D03, // v_cvt_f32_u32 v3, v3
            0x1008_04F6, // v_mul_f32 v4, 4.0, v2
            0x0608_08F3, // v_add_f32 v4, -1.0, v4
            0x100A_06F4, // v_mul_f32 v5, 2.0, v3
            0x060A_0AF3, // v_add_f32 v5, -1.0, v5
            0x7E0C_02F0, // v_mov_b32 v6, 0.5
            0x7E0E_02F2, // v_mov_b32 v7, 1.0
            0x7E12_02FF, // v_mov_b32 v9, prim
            prim,
            0xF800_0941, // exp prim v9, off, off, off done
            0x0000_0009,
            0xF800_08CF, // exp pos0 v4, v5, v6, v7 done
            0x0706_0504,
            0xBF81_0000, // s_endpgm
        ];
        let native = NativeProgram {
            stage: HwStage::Ms,
            wave64: false,
            vgprs: 10,
            layout: vec![],
            used: vec![],
            io: StageIo::Ms(crate::native::MeshIo {
                outputs: vec![],
                clip_cntl: 0,
                threads: 32,
                max_vertices: 32,
                max_primitives: 32,
                topology: 2,
                lds_bytes: 0,
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let position = SigElement {
            name: "SV_Position".into(),
            index: 0,
            kind: 3,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 0xF,
            half: false,
        };
        let osg1 = container::signature(&[position], false);
        let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes)?;
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
        let st = structure::structure(&p)?;
        let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
        let io = io::raise(&p, &native)?;
        emit(&c, &native, &p, &st, &io, &res)
    }

    /// A primitive export's corners are 9-bit indices: the edge flag above
    /// each (bits 9, 19, 29) is no part of it. Masking ten bits made a
    /// flagged corner index 512 or more, past any mesh program's vertices
    /// (the draw test below shows the triangle). Flagged, unflagged and
    /// culled exports translate, and dxil.dll, where there is one, takes
    /// each module.
    #[test]
    fn primitive_export_indices_drop_the_edge_flags() {
        #[cfg(windows)]
        let mut validator = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        )
        .ok();
        for prim in [
            TRIANGLE,
            TRIANGLE | EDGE_FLAGS,
            TRIANGLE | EDGE_FLAGS | CULL,
        ] {
            let ms = triangle_mesh(prim).unwrap_or_else(|e| panic!("{prim:#x}: {e}"));
            #[cfg(windows)]
            if let Some(v) = validator.as_mut() {
                v.validate(&mut ms.dxil.clone())
                    .unwrap_or_else(|e| panic!("{prim:#x}: {e}"));
            }
        }
    }

    /// A triangle mesh program on the host GPU and on WARP (where they have
    /// mesh shaders): triangle (0, 1, 2) covers the target with its corners'
    /// edge flags set as without them; with the culling bit, nothing is lit.
    #[test]
    #[cfg(windows)]
    fn edge_flagged_triangles_draw_as_unflagged_ones() {
        use super::host::{Draw, Host, Root};
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_FEATURE_D3D12_OPTIONS7, D3D12_FEATURE_DATA_D3D12_OPTIONS7,
            D3D12_MESH_SHADER_TIER_NOT_SUPPORTED, D3D12_SHADER_VISIBILITY_MESH,
        };
        let _one = super::valu_tests::ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ps = constant_color();
        let mut ran = 0;
        for (label, warp) in [("hardware", false), ("warp", true)] {
            let Ok(host) = Host::new(warp) else {
                eprintln!("no SM 6.6 {label} device; skipped");
                continue;
            };
            let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
            // SAFETY: the query writes the structure of the given size.
            let queried = unsafe {
                host.device.CheckFeatureSupport(
                    D3D12_FEATURE_D3D12_OPTIONS7,
                    (&raw mut options).cast(),
                    size_of_val(&options) as u32,
                )
            };
            if queried.is_err() || options.MeshShaderTier == D3D12_MESH_SHADER_TIER_NOT_SUPPORTED {
                eprintln!("no mesh shaders on the {label} device; skipped");
                continue;
            }
            for (prim, culled) in [
                (TRIANGLE, false),
                (TRIANGLE | EDGE_FLAGS, false),
                (TRIANGLE | EDGE_FLAGS | CULL, true),
            ] {
                let ms = triangle_mesh(prim).unwrap();
                let root = host
                    .root_signature_for(&[(&ms, D3D12_SHADER_VISIBILITY_MESH)])
                    .unwrap();
                let pso = host
                    .mesh_pipeline_with(&root, &ms.dxil, &ps)
                    .unwrap_or_else(|e| panic!("{label}: mesh pipeline: {e}"));
                let texels = host
                    .draw(
                        &root,
                        &pso,
                        &Draw {
                            roots: vec![(0, Root::Constants(vec![0]))],
                            textures: &[],
                            samplers: &[],
                            vertices: &[],
                            stride: 0,
                            size: (16, 16),
                            mesh_groups: 1,
                        },
                    )
                    .unwrap_or_else(|e| panic!("{label}: draw: {e}"));
                let lit = texels.iter().filter(|t| t[3] != 0.0).count();
                if culled {
                    assert_eq!(lit, 0, "{label}, {prim:#x}: a culled triangle lit texels");
                    continue;
                }
                assert!(
                    texels.iter().all(|t| *t == [1.0, 0.5, 0.25, 1.0]),
                    "{label}, {prim:#x}: {lit} of 256 texels lit"
                );
            }
            eprintln!("{label}: edge-flagged triangles drew");
            ran += 1;
        }
        if ran == 0 {
            eprintln!("no device with mesh shaders; skipped");
        }
    }

    /// A pixel shader interpolating at sample positions (PERSP_SAMPLE
    /// barycentrics, `SPI_PS_INPUT_ADDR` bit 0) runs at sample frequency:
    /// DXC's PSV0 says so in PSInfo.SampleFrequency (runtime info byte 1)
    /// whenever an input is interpolated per sample.
    #[test]
    fn sample_interpolated_inputs_set_psv_sample_frequency() {
        let words = [
            0xC808_0000u32, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001,    // v_interp_p2_f32 v2, v1, attr0.x
            0xF800_180F,
            0x0202_0202, // exp mrt0, v2, v2, v2, v2 done vm
            0xBF81_0000,
        ];
        let psv_of = |input_addr: u32| {
            let native = super::NativeProgram {
                stage: super::HwStage::Ps,
                wave64: false,
                vgprs: 4,
                layout: vec![],
                used: vec![],
                io: crate::native::StageIo::Ps {
                    inputs: vec![crate::native::PsInput {
                        usage: crate::native::Usage::Generic,
                        row: 0,
                        mask: 0b1111,
                        attr: 0,
                        flat: false,
                        fp16: false,
                        pack2: false,
                        row2: 0,
                    }],
                    input_addr,
                    db_shader_control: 0,
                    patches: vec![],
                },
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            };
            let el = |name: &str, kind, usage| SigElement {
                name: name.into(),
                index: 0,
                kind,
                comp: 9,
                interp: 0,
                row: Some(0),
                col: 0,
                cols: 4,
                usage,
                half: false,
            };
            let isg1 = super::container::signature(&[el("TEXCOORD", 0, 0)], true);
            let osg1 = super::container::signature(&[el("SV_Target", 16, 15)], false);
            let bytes =
                crate::dxbc::build(&[(super::FourCC::ISG1, &isg1), (super::FourCC::OSG1, &osg1)]);
            let c = super::Container::parse(&bytes).unwrap();
            let p = crate::ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
            let st = crate::structure::structure(&p).unwrap();
            let res =
                crate::resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
            let io = crate::io::raise(&p, &native).unwrap();
            let out = super::emit(&c, &native, &p, &st, &io, &res).unwrap();
            // dxil.dll, where there is one, checks PSV0 against the module.
            #[cfg(windows)]
            if let Ok(mut v) = super::validator::Validator::load(
                super::validator::Validator::default_path().as_deref(),
            ) {
                v.validate(&mut out.dxil.clone()).unwrap();
            }
            let host = super::Container::parse(&out.dxil).unwrap();
            host.part(super::FourCC::PSV0).unwrap().data.to_vec()
        };
        // Runtime info starts after its size; PSInfo is {DepthOutput, SampleFrequency}.
        let center = psv_of(1 << 1);
        assert_eq!(center[4 + 1], 0, "center interpolation is per pixel");
        let sample = psv_of(1 << 0);
        assert_eq!(sample[4 + 1], 1, "PSInfo.SampleFrequency");
    }

    /// A pixel shader reading channel z of an input whose element starts at
    /// w. Looking the channel up subtracted the element's first channel
    /// before checking the channel lies in it, so in a build with overflow
    /// checks (tests, debug) any read below an element that starts at z or
    /// w panicked, whichever row it was in (found by random-input testing).
    #[test]
    fn reading_a_channel_below_an_input_element_does_not_underflow() {
        // v_interp_mov_f32 v3, p0, attr0.z; s_endpgm
        let words = [0xC80E_0202u32, 0xBF81_0000];
        let native = super::NativeProgram {
            stage: super::HwStage::Ps,
            wave64: false,
            vgprs: 4,
            layout: vec![],
            used: vec![],
            io: crate::native::StageIo::Ps {
                inputs: vec![crate::native::PsInput {
                    usage: crate::native::Usage::Generic,
                    row: 0,
                    mask: 0b1000,
                    attr: 0,
                    flat: false,
                    fp16: false,
                    pack2: false,
                    row2: 0,
                }],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let w_only = SigElement {
            name: "TEXCOORD".into(),
            index: 0,
            kind: 0,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 3,
            cols: 1,
            usage: 0,
            half: false,
        };
        let isg1 = super::container::signature(&[w_only], true);
        let bytes = crate::dxbc::build(&[(super::FourCC::ISG1, &isg1)]);
        let c = super::Container::parse(&bytes).unwrap();
        let p = crate::ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = crate::structure::structure(&p).unwrap();
        let res = crate::resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = crate::io::raise(&p, &native).unwrap();
        // Translated or refused with a reason; it must not panic.
        let _ = super::emit(&c, &native, &p, &st, &io, &res);
    }

    #[test]
    fn valu_div_f32_sequence_executes_on_gpu() {
        let words: &[u32] = &[
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0x0
            0xF408_0302,
            0xFA00_0020, // s_load_dwordx4 s[12:15], s[4:5], 0x20
            0x340E_0083, // v_lshlrev_b32 v7, 3, v0
            0xE034_1000,
            0x8002_0107, // buffer_load_dwordx2 v[1:2], v7, s[8:11], 0 offen
            0xD56D_6A03,
            0x0406_0502, // v_div_scale_f32 v3, vcc_lo, v2, v2, v1
            0xD56D_1008,
            0x0406_0501, // v_div_scale_f32 v8, s16, v1, v2, v1
            0x7E08_5503, // v_rcp_f32 v4, v3
            0xD54B_0005,
            0x23CA_0903, // v_fma_f32 v5, -v3, v4, 1.0
            0xD54B_0004,
            0x0412_0905, // v_fma_f32 v4, v5, v4, v4
            0x100A_0908, // v_mul_f32 v5, v8, v4
            0xD54B_0006,
            0x2422_0B03, // v_fma_f32 v6, -v3, v5, v8
            0xD56F_0004,
            0x0416_0906, // v_div_fmas_f32 v4, v6, v4, v5
            0xD55F_0001,
            0x0406_0504, // v_div_fixup_f32 v1, v4, v2, v1
            0x340E_0082, // v_lshlrev_b32 v7, 2, v0
            0xE070_1000,
            0x8003_0107, // buffer_store_dword v1, v7, s[12:15], 0 offen
            0xBF81_0000, // s_endpgm
        ];
        let native = super::NativeProgram {
            stage: super::HwStage::Cs,
            wave64: false,
            vgprs: 9,
            layout: vec![
                crate::RootParam {
                    root: crate::RootType::Constants,
                    table_offset_dwords: 0,
                    sgprs: 4,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                },
                crate::RootParam {
                    root: crate::RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 4,
                    slot: 1,
                },
            ],
            used: vec![
                crate::UsedResource {
                    kind: crate::ResourceType::Srv,
                    root: crate::RootType::Table,
                    table_offset_dwords: 0,
                    lower_bound: 0,
                    stride: 4,
                    dwords: 4,
                    user_data: 4,
                    slot: 1,
                },
                crate::UsedResource {
                    kind: crate::ResourceType::Uav,
                    root: crate::RootType::Table,
                    table_offset_dwords: 8,
                    lower_bound: 0,
                    stride: 4,
                    dwords: 4,
                    user_data: 4,
                    slot: 1,
                },
            ],
            io: crate::native::StageIo::Cs(crate::native::ComputeIo {
                rsrc2: 5 << 1,
                threads: [7, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = crate::ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let res = crate::resource::track(&p, &native.layout, native.stage, None).unwrap();
        let io = crate::io::raise(&p, &native).unwrap();
        let st = crate::structure::structure(&p).unwrap();
        let l = super::lower::lower(&super::lower::Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: Vec::new(),
            outputs: Vec::new(),
            prims: Vec::new(),
            mesh_stride: 0,
            gs: None,
        })
        .unwrap();
        let mut dxil = l.dxil;
        #[cfg(windows)]
        if let Ok(mut v) = super::validator::Validator::load(
            super::validator::Validator::default_path().as_deref(),
        ) {
            v.validate(&mut dxil).unwrap();
        }
        #[cfg(windows)]
        if let Ok(host) = super::host::Host::new(false) {
            let cs = super::HostShader {
                dispatch_x_one: false,
                stage: super::HwStage::Cs,
                dxil,
                user_data: l.user_data,
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: l.root_cbvs,
                root_srvs: l.root_srvs,
                root_uavs: l.root_uavs,
                root_indexed_views: Vec::new(),
            };
            let (root, pso) = host.compute_pipeline(&cs).unwrap();
            let inputs: Vec<u32> = [
                7.0f32, 3.0, -1.0, 1e-40, 1.0, 1e35, 1e-35, 1e-36, 0.0, 0.0, -1.0, 0.0, -1e35, 1e35,
            ]
            .into_iter()
            .map(f32::to_bits)
            .collect();
            let out = host
                .dispatch(
                    &root,
                    &pso,
                    &super::host::Dispatch {
                        roots: vec![(0, super::host::Root::Constants(vec![0, 0, 0, 0, 0]))],
                        views: &[
                            super::host::View::Buffer {
                                data: &inputs,
                                stride: super::host::Stride::Raw,
                            },
                            super::host::View::RawUav(64),
                        ],
                        samplers: &[],
                        groups: [1, 1, 1],
                    },
                )
                .unwrap();
            let got: Vec<f32> = out[0][..7].iter().copied().map(f32::from_bits).collect();
            assert!((got[0] - (7.0f32 / 3.0)).abs() < 1e-6, "got {:?}", got);
            assert_eq!(out[0][1], (-1.0f32 / 1e-40).to_bits());
            assert!(
                (got[2] - 1e-35).abs() <= 1e-40,
                "lane 2 got {:?} ({:#x})",
                got[2],
                out[0][2]
            );
            assert!(
                (got[3] - 10.0).abs() < 1e-5,
                "lane 3 got {:?} ({:#x})",
                got[3],
                out[0][3]
            );
            assert_eq!(out[0][4], 0xFFC0_0000);
            assert_eq!(out[0][5], (-1.0f32 / 0.0).to_bits());
            assert!(
                (got[6] - (-1.0)).abs() < 1e-6,
                "lane 6 got {:?} ({:#x})",
                got[6],
                out[0][6]
            );
        }
    }
}

#[cfg(test)]
mod atomic_tests {
    use super::*;
    use crate::native::{ComputeIo, RootParam, StageIo};

    #[test]
    fn heap_texture_atomic_widths() {
        // A synthetic compute shader loads a 2D UAV from a root table,
        // compares or adds at (0, 0), then uses the returned register(s).
        for (opcode, mask, wide) in [
            (16u32, 3, false),
            (16, 15, true),
            (17, 1, false),
            (17, 3, true),
        ] {
            let words = [
                0xBE81_0380, // s_mov_b32 s1, 0 (table address high)
                0xF40C_0200,
                0xFA00_0000, // s_load_dwordx8 s[8:15], s[0:1], 0
                0x7E00_0280,
                0x7E02_0280, // v0, v1 = coordinates
                0x7E04_0281,
                0x7E06_0282,
                0x7E08_0283,
                0x7E0A_0284, // v2..v5 = 1..4
                0xF000_2008 | opcode << 18 | mask << 8,
                0x0002_0200,
                // Use the old value in a second atomic at the same location.
                0xF044_0008 | (if wide { 3 } else { 1 }) << 8,
                0x0002_0200,
                0xBF81_0000,
            ];
            let native = NativeProgram {
                stage: HwStage::Cs,
                wave64: false,
                vgprs: 6,
                layout: vec![RootParam {
                    root: RootType::Table,
                    table_offset_dwords: 0,
                    sgprs: 1,
                    base_register: 0,
                    user_data: 0,
                    slot: 0,
                }],
                used: vec![],
                io: StageIo::Cs(ComputeIo {
                    rsrc2: 2,
                    threads: [32, 1, 1],
                }),
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            };
            let p = ir::build(&native.code, false, native.vgprs).unwrap();
            let bytes = crate::dxbc::build(&[]);
            let shader = emit(
                &Container::parse(&bytes).unwrap(),
                &native,
                &p,
                &structure::structure(&p).unwrap(),
                &io::raise(&p, &native).unwrap(),
                &resource::track(&p, &native.layout, native.stage, None).unwrap(),
            )
            .unwrap();
            let container = Container::parse(&shader.dxil).unwrap();
            let features = u64::from_le_bytes(
                container
                    .part(FourCC::SFI0)
                    .unwrap()
                    .data
                    .try_into()
                    .unwrap(),
            );
            let required = 0x1040_8000; // heap atomics, typed atomics, Int64Ops
            assert_eq!(features & required, if wide { required } else { 0 });
            #[cfg(windows)]
            if let Ok(mut v) =
                validator::Validator::load(validator::Validator::default_path().as_deref())
            {
                v.validate(&mut shader.dxil.clone()).unwrap();
            }
        }
    }
}

#[cfg(test)]
mod barycentric_tests {
    use super::container::SigElement;
    use super::*;
    use crate::native::{PsInput, ResourceType, RootParam, RootType, StageIo, Usage, UsedResource};

    /// `SPI_PS_INPUT_ADDR` bits: PERSP_CENTER, LINEAR_CENTER.
    const PERSP_CENTER: u32 = 1 << 1;
    const LINEAR_CENTER: u32 = 1 << 5;

    /// `exp mrt0, v<r>, v<r>, v<r>, v<r> done vm`.
    fn exp_mrt0(r: u32) -> [u32; 2] {
        [0xF800_180F, r * 0x0101_0101]
    }

    /// VOP2 `v_add_f32 v<dst>, v<a>, v<b>`.
    fn v_add_f32(dst: u32, a: u32, b: u32) -> u32 {
        3 << 25 | dst << 17 | b << 9 | (256 + a)
    }

    fn element(name: &str, kind: u8, row: u32) -> SigElement {
        SigElement {
            name: name.into(),
            index: 0,
            kind,
            comp: 9,
            interp: 0,
            row: Some(row),
            col: 0,
            cols: 4,
            usage: 0,
            half: false,
        }
    }

    /// Translates a pixel program with the input VGPRs `input_addr`
    /// enables and one float4 TEXCOORD0 attribute (attr 0, row 0).
    fn pixel(words: &[u32], input_addr: u32) -> Result<HostShader, ShaderError> {
        pixel_of(words, input_addr, false, Vec::new(), Vec::new())
    }

    fn pixel_with_input(words: &[u32], input_addr: u32, fp16: bool) -> Result<HostShader, ShaderError> {
        pixel_of(words, input_addr, fp16, Vec::new(), Vec::new())
    }

    /// [`pixel`] with TEXCOORD0 a 16-bit attribute when `fp16` (one value in the low half of
    /// each dword), and the root parameters `layout` and used resources `used`.
    fn pixel_of(
        words: &[u32],
        input_addr: u32,
        fp16: bool,
        layout: Vec<RootParam>,
        used: Vec<UsedResource>,
    ) -> Result<HostShader, ShaderError> {
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout,
            used,
            io: StageIo::Ps {
                inputs: vec![PsInput {
                    usage: Usage::Generic,
                    row: 0,
                    mask: 0b1111,
                    attr: 0,
                    flat: false,
                    fp16,
                    pack2: false,
                    row2: 0,
                }],
                input_addr,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        let isg1 = container::signature(&[element("TEXCOORD", 0, 0)], true);
        let osg1 = container::signature(&[target], false);
        let bytes = crate::dxbc::build(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
        let res = resource::track(&p, &native.layout, native.stage, None)?;
        let io = io::raise(&p, &native)?;
        let st = structure::structure(&p)?;
        emit(&c, &native, &p, &st, &io, &res)
    }

    /// The translation's `SFI0` features and `ISG1` rows.
    fn parts(shader: &HostShader) -> (u64, Vec<SigElement>) {
        let c = Container::parse(&shader.dxil).unwrap();
        let sfi0 = c.part(FourCC::SFI0).unwrap().data.try_into().unwrap();
        (
            u64::from_le_bytes(sfi0),
            signature(c.part(FourCC::ISG1).unwrap().data).unwrap(),
        )
    }

    fn validate(shader: &HostShader) {
        #[cfg(windows)]
        if let Ok(mut v) =
            validator::Validator::load(validator::Validator::default_path().as_deref())
        {
            v.validate(&mut shader.dxil.clone()).unwrap();
        }
        let _ = shader;
    }

    /// Custom interpolation reads I and J (v0, v1 with PERSP_CENTER) as
    /// plain floats: they are channels y and z of `SV_Barycentrics`, the
    /// weights of vertices 1 and 2.
    #[test]
    fn barycentrics_read_as_values_are_sv_barycentrics() {
        let [e0, e1] = exp_mrt0(2);
        let words = [v_add_f32(2, 0, 1), e0, e1, 0xBF81_0000];
        let shader = pixel(&words, PERSP_CENTER).unwrap();
        let (features, inputs) = parts(&shader);
        assert_ne!(features & 0x2_0000, 0, "no Barycentrics feature");
        let bary: Vec<_> = inputs.iter().filter(|e| e.kind == 28).collect();
        assert_eq!(bary.len(), 1, "{inputs:?}");
        assert_eq!((bary[0].index, bary[0].col, bary[0].cols), (0, 0, 3));
        // Not packed: the validator wants packing location -1.
        assert_eq!(bary[0].row, None);
        // Only y and z (I and J) are read.
        let isg1 = Container::parse(&shader.dxil).unwrap();
        let isg1 = isg1.part(FourCC::ISG1).unwrap().data;
        let rw: Vec<u8> = (0..2).map(|k| isg1[8 + 32 * k + 25]).collect();
        assert!(rw.contains(&0b110), "{rw:?}");
        validate(&shader);
    }

    /// A 16-bit result written over a first interpolation half's register
    /// (`v_fma_mixlo_f16` keeps the other half): the second half evaluates the pair, and
    /// what the first half leaves in the half kept is no number the code reads.
    #[test]
    fn a_half_written_over_a_first_interpolation_half_translates() {
        let [e0, e1] = exp_mrt0(4);
        let words = [
            0xD742_0002,
            0x0002_0000, // v_interp_p1ll_f16 v2, v0, attr0.x
            0xD75A_0003,
            0x040A_0200, // v_interp_p2_f16 v3, v1, attr0.x, v2
            0xCC21_0002,
            0x0A01_E503, // v_fma_mixlo_f16 v2, sel_lo(v3), 1.0, 0
            0x7E08_1702, // v_cvt_f32_f16 v4, v2
            e0,
            e1,
            0xBF81_0000,
        ];
        let shader = pixel_of(&words, PERSP_CENTER, true, Vec::new(), Vec::new()).unwrap();
        validate(&shader);
    }

    /// A level-of-detail query of a 2D array texture: the layer is no coordinate of the
    /// level, and DXIL wants it undefined (the validator refused three of the title's
    /// deferred programs: "out of bound coord must be undef").
    #[test]
    fn a_level_query_of_an_array_leaves_the_layer_out() {
        // A table of textures at s0, one of samplers at s1.
        let table = |user_data| RootParam {
            root: RootType::Table,
            table_offset_dwords: 0,
            sgprs: 1,
            base_register: 0,
            user_data,
            slot: user_data,
        };
        let range = |kind, dwords, user_data| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords,
            user_data,
            slot: user_data,
        };
        let [e0, e1] = exp_mrt0(4);
        let words = [
            0xBE82_0300, // s_mov_b32 s2, s0
            0xBE83_0380, // s_mov_b32 s3, 0
            0xF40C_0101,
            0xFA00_0000, // s_load_dwordx8 s[4:11], s[2:3], null
            0xBE8C_0301, // s_mov_b32 s12, s1
            0xBE8D_0380, // s_mov_b32 s13, 0
            0xF408_0406,
            0xFA00_0000, // s_load_dwordx4 s[16:19], s[12:13], null
            0x7E04_0280, // v_mov_b32 v2, 0 (layer 0)
            0xF180_0328,
            0x0081_0400, // image_get_lod v[4:5], v[0:2], s[4:11], s[16:19] dmask:0x3 2D array
            e0,
            e1,
            0xBF81_0000,
        ];
        let layout = vec![table(0), table(1)];
        let used = vec![range(ResourceType::Srv, 8, 0), range(ResourceType::Sampler, 4, 1)];
        let shader = pixel_of(&words, PERSP_CENTER, false, layout, used).unwrap();
        validate(&shader);
    }

    /// A pixel program ordered by primitive (POPS, the title's pedestrian cut-out
    /// programs): it takes part in the ordering, waits for the overlapping waves to leave
    /// the section, reads and writes a texture, and says it is done. The wait goes; the
    /// texture is a rasterizer-ordered view.
    #[test]
    fn an_ordered_pixel_program_writes_through_a_rasterizer_ordered_view() {
        let [e0, e1] = exp_mrt0(4);
        let words = [
            0xBE82_0300, // s_mov_b32 s2, s0
            0xBE83_0380, // s_mov_b32 s3, 0
            0xF40C_0101,
            0xFA00_0000, // s_load_dwordx8 s[4:11], s[2:3], null
            0xBA80_1019,
            0x0000_0001, // s_setreg_imm32_b32 hwreg(HW_REG_POPS_PACKER, 0, 3), 1
            0xBF06_8001, // s_cmp_eq_u32 s1, 0
            0xBF85_0004, // s_cbranch_scc1 -> past the wait
            0xBF8E_FFFF, // s_sleep 0xffff
            0x810C_01EF, // s_add_i32 s12, src_pops_exiting_wave_id, s1
            0xBF0A_0C01, // s_cmp_lt_u32 s1, s12
            0xBF84_FFFC, // s_cbranch_scc0 -> s_sleep
            0x7E04_0280, // v_mov_b32 v2, 0
            0x7E06_0280, // v_mov_b32 v3, 0
            0xF000_3F08,
            0x0001_0402, // image_load v[4:7], v[2:3], s[4:11] dmask:0xf 2D unorm glc
            0xF020_3F08,
            0x0001_0402, // image_store v[4:7], v[2:3], s[4:11] dmask:0xf 2D unorm glc
            0xBF90_0007, // s_sendmsg(MSG_ORDERED_PS_DONE)
            e0,
            e1,
            0xBF81_0000,
        ];
        let param = |root, sgprs, user_data| RootParam {
            root,
            table_offset_dwords: 0,
            sgprs,
            base_register: 0,
            user_data,
            slot: user_data,
        };
        let layout = vec![param(RootType::Table, 1, 0), param(RootType::Constants, 1, 1)];
        let used = vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 8,
            user_data: 0,
            slot: 0,
        }];
        let shader = pixel_of(&words, PERSP_CENTER, false, layout, used).unwrap();
        let (features, _) = parts(&shader);
        assert_ne!(features & 0x1000, 0, "no rasterizer-ordered views");
        validate(&shader);
    }

    /// Two kinds of barycentrics read directly: one element per mode.
    #[test]
    fn two_barycentric_modes_get_two_elements() {
        // v0-v1 perspective i, j; v2-v3 linear i, j.
        let [e0, e1] = exp_mrt0(4);
        let words = [v_add_f32(4, 0, 3), e0, e1, 0xBF81_0000];
        let shader = pixel(&words, PERSP_CENTER | LINEAR_CENTER).unwrap();
        let (_, inputs) = parts(&shader);
        let mut indices: Vec<u32> = inputs
            .iter()
            .filter(|e| e.kind == 28)
            .map(|e| e.index)
            .collect();
        indices.sort();
        assert_eq!(indices, [0, 1], "{inputs:?}");
        validate(&shader);
    }

    /// I and J only fed to `v_interp_*` stay host interpolation: no
    /// barycentrics are declared.
    #[test]
    fn interpolation_alone_declares_no_barycentrics() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            e0,
            e1,
            0xBF81_0000,
        ];
        let shader = pixel(&words, PERSP_CENTER).unwrap();
        let (features, inputs) = parts(&shader);
        assert_eq!(features & 0x2_0000, 0);
        assert!(inputs.iter().all(|e| e.kind != 28), "{inputs:?}");
        validate(&shader);
    }

    #[test]
    fn mixed_f16_write_preserves_the_first_interpolation_half() {
        for mix in [0xCC21_0002, 0xCC22_0002] {
            let words = [
                0xD742_0002,
                0x0002_0000, // v_interp_p1ll_f16 v2, v0, attr0.x
                0xD75A_0003,
                0x040A_0200, // v_interp_p2_f16 v3, v1, attr0.x, v2
                mix,
                0x03C9_E4F2, // v_fma_mixlo/hi_f16 v2, 1.0, 1.0, 1.0
                0x7E08_1702, // v_cvt_f32_f16 v4, v2
                0x2C0A_0490, // v_lshrrev_b32 v5, 16, v2
                0x7E0C_1705, // v_cvt_f32_f16 v6, v5
                0x7E0E_1703, // v_cvt_f32_f16 v7, v3
                0xF800_180F,
                0x0407_0604, // exp mrt0, v4, v6, v7, v4 done vm
                S_ENDPGM,
            ];
            let shader = pixel_with_input(&words, PERSP_CENTER, true).unwrap();
            let (_, inputs) = parts(&shader);
            assert_eq!(inputs.iter().filter(|e| e.kind == 28).count(), 1);
            assert_eq!(texcoord_mode(&shader), 1);
            validate(&shader);
        }
    }

    /// Both: the attribute is interpolated and J is used as a value.
    #[test]
    fn interpolation_and_a_barycentric_value_together() {
        let [e0, e1] = exp_mrt0(3);
        let words = [
            0xC808_0000,        // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001,        // v_interp_p2_f32 v2, v1, attr0.x
            v_add_f32(3, 2, 1), // v3 = attribute + j
            e0,
            e1,
            0xBF81_0000,
        ];
        let shader = pixel(&words, PERSP_CENTER).unwrap();
        let (features, inputs) = parts(&shader);
        assert_ne!(features & 0x2_0000, 0);
        assert_eq!(inputs.iter().filter(|e| e.kind == 28).count(), 1);
        validate(&shader);
    }
    /// The `PSV0` interpolation mode of TEXCOORD0 (row 0, x-w, float).
    fn texcoord_mode(shader: &HostShader) -> u8 {
        let c = Container::parse(&shader.dxil).unwrap();
        let psv = c.part(FourCC::PSV0).unwrap().data;
        let row = [1, 0, 0x44, 0, 3];
        let at = psv.windows(5).position(|w| w == row).unwrap();
        psv[at + 5]
    }

    const S_ENDPGM: u32 = 0xBF81_0000;

    /// An interpolation whose i is computed (`v_mul_f32 v3, 0.5, v0`) reads
    /// the attribute's vertices: TEXCOORD0 becomes constant, read with
    /// `attributeAtVertex`; i and j are `SV_Barycentrics` values.
    #[test]
    fn computed_barycentrics_read_the_vertices() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0x1006_00F0, // v_mul_f32 v3, 0.5, v0
            0xC808_0003, // v_interp_p1_f32 v2, v3, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            e0,
            e1,
            S_ENDPGM,
        ];
        let shader = pixel(&words, PERSP_CENTER).unwrap();
        let (features, inputs) = parts(&shader);
        assert_ne!(features & 0x2_0000, 0, "no Barycentrics feature");
        assert_eq!(inputs.iter().filter(|e| e.kind == 28).count(), 1);
        assert_eq!(texcoord_mode(&shader), 1);
        validate(&shader);
    }

    /// Weights from constants alone: the vertices, no `SV_Barycentrics`.
    #[test]
    fn constant_weights_need_no_barycentrics() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0x7E06_02F0, // v_mov_b32 v3, 0.5
            0xC808_0003, // v_interp_p1_f32 v2, v3, attr0.x
            0xC809_0003, // v_interp_p2_f32 v2, v3, attr0.x
            e0,
            e1,
            S_ENDPGM,
        ];
        let shader = pixel(&words, PERSP_CENTER).unwrap();
        let (features, inputs) = parts(&shader);
        assert_ne!(features & 0x2_0000, 0, "no Barycentrics feature");
        assert!(inputs.iter().all(|e| e.kind != 28), "{inputs:?}");
        assert_eq!(texcoord_mode(&shader), 1);
        validate(&shader);
    }

    /// The same attribute interpolated with the loaded i and j and with a
    /// computed j: both from its vertices, the first weighed with
    /// `SV_Barycentrics`.
    #[test]
    fn computed_and_loaded_barycentrics_on_one_attribute() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC810_0100, // v_interp_p1_f32 v4, v0, attr0.y
            0xC811_0101, // v_interp_p2_f32 v4, v1, attr0.y
            0x1006_02F0, // v_mul_f32 v3, 0.5, v1
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0003, // v_interp_p2_f32 v2, v3, attr0.x
            v_add_f32(2, 2, 4),
            e0,
            e1,
            S_ENDPGM,
        ];
        let shader = pixel(&words, PERSP_CENTER).unwrap();
        let (_, inputs) = parts(&shader);
        assert_eq!(inputs.iter().filter(|e| e.kind == 28).count(), 1);
        assert_eq!(texcoord_mode(&shader), 1);
        validate(&shader);
    }

    /// The `PSV0` interpolation mode of the element at `row`, `col` with `cols` columns and
    /// signature component type `comp` (1 u32, 3 f32).
    fn mode_at(shader: &HostShader, row: u8, col: u8, cols: u8, comp: u8) -> u8 {
        let c = Container::parse(&shader.dxil).unwrap();
        let psv = c.part(FourCC::PSV0).unwrap().data;
        let element = [1, row, 0x40 | col << 4 | cols, 0, comp];
        let at = psv.windows(5).position(|w| w == element).unwrap();
        psv[at + 5]
    }

    /// A program of `stage` with the signature parts `isg1`, `osg1` and stage data `io`.
    fn translate_with(
        stage: HwStage,
        words: &[u32],
        io: StageIo,
        isg1: &[SigElement],
        osg1: &[SigElement],
    ) -> HostShader {
        let native = NativeProgram {
            stage,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io,
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let isg1 = container::signature(isg1, true);
        let osg1 = container::signature(osg1, false);
        let bytes = crate::dxbc::build(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, None).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        emit(&c, &native, &p, &st, &io, &res).unwrap()
    }

    fn packed(name: &str, kind: u8, comp: u8, row: u32, col: u8, cols: u8) -> SigElement {
        SigElement {
            col,
            cols,
            comp,
            ..element(name, kind, row)
        }
    }

    /// VS_Transform_VfxVBuffer_AlphaClip's register 1: a float2 the console declared
    /// `nointerpolation` beside a uint2. The float's default, linear, packed two modes into
    /// the register (refused by the validator); it takes the uint's.
    #[test]
    fn a_vertex_output_beside_an_integer_takes_its_mode() {
        use crate::native::VsOutput;
        let words = [
            0x7E02_02F2, // v_mov_b32 v1, 1.0
            0x7E04_0287, // v_mov_b32 v2, 7
            0xF800_020F,
            0x0202_0101, // exp param0, v1, v1, v2, v2
            0xF800_08CF,
            0x0101_0101, // exp pos0, v1, v1, v1, v1 done
            S_ENDPGM,
        ];
        let io = StageIo::Vs {
            inputs: vec![],
            outputs: vec![VsOutput {
                usage: Usage::Generic,
                row: 1,
                param: 0,
                fp16: false,
                pack2: false,
            }],
        };
        let osg1 = [
            element("SV_Position", 3, 0),
            packed("TEXCOORD", 0, 9, 1, 0, 2),
            SigElement {
                index: 14,
                ..packed("TEXCOORD", 0, 5, 1, 2, 2)
            },
        ];
        let shader = translate_with(HwStage::Vs, &words, io, &[], &osg1);
        assert_eq!(mode_at(&shader, 1, 0, 2, 3), 1, "the float2");
        assert_eq!(mode_at(&shader, 1, 2, 2, 1), 1, "the uint2");
        validate(&shader);
    }

    /// The pixel side of that register: the program reads the uint2 at its provoking vertex
    /// (`v_interp_mov`, constant) and never the float2, whose default would be linear.
    #[test]
    fn an_unread_pixel_input_takes_the_mode_of_its_register() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC80A_0202, // v_interp_mov_f32 v2, p0, attr0.z
            e0,
            e1,
            S_ENDPGM,
        ];
        let io = StageIo::Ps {
            inputs: vec![PsInput {
                usage: Usage::Generic,
                row: 0,
                mask: 0b1100,
                attr: 0,
                flat: true,
                fp16: false,
                pack2: false,
                row2: 0,
            }],
            input_addr: 0,
            db_shader_control: 0,
            patches: vec![],
        };
        let isg1 = [
            packed("TEXCOORD", 0, 9, 0, 0, 2),
            SigElement {
                index: 14,
                ..packed("TEXCOORD", 0, 5, 0, 2, 2)
            },
        ];
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        let shader = translate_with(HwStage::Ps, &words, io, &isg1, &[target]);
        assert_eq!(mode_at(&shader, 0, 0, 2, 3), 1, "the float2");
        assert_eq!(mode_at(&shader, 0, 2, 2, 1), 1, "the uint2");
        validate(&shader);
    }

    /// PS_PropFoliageDeferred_Fire_ExportDepth's register 9: a float the program never reads
    /// beside the front face (constant).
    #[test]
    fn an_unread_pixel_input_beside_the_front_face_is_constant() {
        let words = [
            0x7E04_02F2, // v_mov_b32 v2, 1.0
            0xF800_180F,
            0x0202_0202, // exp mrt0, v2, v2, v2, v2 done vm
            S_ENDPGM,
        ];
        let io = StageIo::Ps {
            inputs: vec![],
            input_addr: 1 << 12,
            db_shader_control: 0,
            patches: vec![],
        };
        let isg1 = [
            packed("TEXCOORD", 0, 9, 0, 0, 1),
            packed("SV_IsFrontFace", 13, 5, 0, 1, 1),
        ];
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        let shader = translate_with(HwStage::Ps, &words, io, &isg1, &[target]);
        assert_eq!(mode_at(&shader, 0, 0, 1, 3), 1, "the float");
        validate(&shader);
    }

    /// P10 of an attribute the program interpolates is P1 - P0.
    #[test]
    fn p10_of_an_interpolated_attribute() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            0xC812_0000, // v_interp_mov_f32 v4, p10, attr0.x
            v_add_f32(2, 2, 4),
            e0,
            e1,
            S_ENDPGM,
        ];
        let shader = pixel(&words, PERSP_CENTER).unwrap();
        assert_eq!(texcoord_mode(&shader), 1);
        validate(&shader);
        // Alone, what the hardware holds is not known.
        let [e0, e1] = exp_mrt0(4);
        let words = [0xC812_0000, e0, e1, S_ENDPGM];
        assert!(pixel(&words, PERSP_CENTER).is_err());
    }

    /// A pixel program with TEXCOORD0..2 in rows 0..2 that reads only attribute 2's z
    /// (`v_interp_mov_f32 v2, p0, attr2.z`), translated with `linked` (`link_mesh`'s pixel
    /// side).
    fn linked_pixel(linked: link::Linked) -> Result<HostShader, ShaderError> {
        let [e0, e1] = exp_mrt0(2);
        let words = [0xC80A_0A02, e0, e1, S_ENDPGM];
        let input = |row: u8| PsInput {
            usage: Usage::Generic,
            row,
            mask: 0b1111,
            attr: row,
            flat: true,
            fp16: false,
            pack2: false,
            row2: row,
        };
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![input(0), input(1), input(2)],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let texcoord = |i: u32| SigElement {
            index: i,
            ..element("TEXCOORD", 0, i)
        };
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        let isg1 = container::signature(&[texcoord(0), texcoord(1), texcoord(2)], true);
        let osg1 = container::signature(&[target], false);
        let bytes = crate::dxbc::build(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
        let res = resource::track(&p, &native.layout, native.stage, None)?;
        let io = io::raise(&p, &native)?;
        let st = structure::structure(&p)?;
        link::with_linked(linked, || emit(&c, &native, &p, &st, &io, &res))
    }

    /// `link_mesh`'s pixel side: the inputs the program never reads leave its signature, the
    /// one it reads keeps its row and is read as the signature's first element now; leaving
    /// out an input it reads is refused.
    #[test]
    fn a_linked_pixel_program_declares_only_the_inputs_it_reads() {
        let rows = |s: &HostShader| -> Vec<(u32, Option<u32>)> {
            let c = Container::parse(&s.dxil).unwrap();
            let isg1 = signature(c.part(FourCC::ISG1).unwrap().data).unwrap();
            isg1.iter().map(|e| (e.index, e.row)).collect()
        };
        let whole = linked_pixel(link::Linked::default()).unwrap();
        assert_eq!(rows(&whole), [(0, Some(0)), (1, Some(1)), (2, Some(2))]);
        let unread = vec![("TEXCOORD".to_owned(), 0), ("texcoord".to_owned(), 1)];
        let linked = linked_pixel(link::Linked {
            inputs: unread,
            outputs: Vec::new(),
        })
        .unwrap();
        assert_eq!(rows(&linked), [(2, Some(2))]);
        // (dxil.dll checks each read against the element its id names)
        validate(&whole);
        validate(&linked);
        let read = linked_pixel(link::Linked {
            inputs: vec![("TEXCOORD".to_owned(), 2)],
            outputs: Vec::new(),
        });
        assert!(
            matches!(&read, Err(ShaderError::Unsupported { what, .. }) if *what == "an input left out of a linked pipeline read"),
            "{:?}",
            read.err()
        );
    }

    /// The other side, for a stage whose outputs a pixel program reads: an output left out is
    /// neither stored nor declared, the others keep their rows and are stored to their element
    /// as the signature numbers it now (SV_Position 0, TEXCOORD1 1).
    #[test]
    fn a_linked_stage_stores_only_the_outputs_it_keeps() {
        use crate::native::VsOutput;
        let words = [
            0x7E02_02F2, // v_mov_b32 v1, 1.0
            0x7E04_0287, // v_mov_b32 v2, 7
            0xF800_020F,
            0x0202_0101, // exp param0, v1, v1, v2, v2
            0xF800_021F,
            0x0101_0101, // exp param1, v1, v1, v1, v1
            0xF800_08CF,
            0x0101_0101, // exp pos0, v1, v1, v1, v1 done
            S_ENDPGM,
        ];
        let output = |row: u8, param: u8| VsOutput {
            usage: Usage::Generic,
            row,
            param,
            fp16: false,
            pack2: false,
        };
        let io = StageIo::Vs {
            inputs: vec![],
            outputs: vec![output(1, 0), output(2, 1)],
        };
        let osg1 = [
            element("SV_Position", 3, 0),
            element("TEXCOORD", 0, 1),
            SigElement {
                index: 1,
                ..element("TEXCOORD", 0, 2)
            },
        ];
        let linked = link::Linked {
            inputs: Vec::new(),
            outputs: vec![("TEXCOORD".to_owned(), 0)],
        };
        let shader = link::with_linked(linked, || translate_with(HwStage::Vs, &words, io, &[], &osg1));
        let c = Container::parse(&shader.dxil).unwrap();
        let out = signature(c.part(FourCC::OSG1).unwrap().data).unwrap();
        let rows: Vec<(&str, u32, Option<u32>)> = out.iter().map(|e| (e.name.as_str(), e.index, e.row)).collect();
        assert_eq!(rows, [("SV_Position", 0, Some(0)), ("TEXCOORD", 1, Some(2))]);
        validate(&shader);
    }
}

/// The geometry stage's first step: containers of the merged ES+GS stage
/// (synthetic programs) get past tracing and I/O raising and stop at
/// [`GS_EMIT`] in emission.
#[cfg(test)]
mod merged_gs_tests {
    use super::*;
    use crate::native::tests::container;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    const S_ENDPGM: u32 = 0xBF81_0000;

    /// `s_load_dword s0, s[<base>:<base + 1>], 0xb8`, then the end.
    fn load(base: u32) -> Vec<u8> {
        bytes(&[0xF400_0000 | base >> 1, 0xFA00_00B8, S_ENDPGM])
    }

    #[test]
    fn a_geometry_container_alone_does_not_trace() {
        // The load through the runtime table pointer (user data 0 and 1) is
        // "an address in an SGPR that is no root parameter" until the ES
        // half's layout says nothing is there.
        let gs = container(&[(HwStage::Gs, &load(8))]);
        assert!(matches!(
            translate(&gs),
            Err(ShaderError::Unsupported { what, .. })
                if what == "an address in an SGPR that is no root parameter"
        ));
    }

    /// The ES half of a pair: record of five dwords, then the handover.
    fn es_half() -> Vec<u8> {
        bytes(&crate::handoff::tests::es_words())
    }

    /// The GS half: reads of two vertices' records, then the load through
    /// the runtime table.
    fn gs_half() -> Vec<u8> {
        let mut words = crate::handoff::tests::gs_words();
        words.pop();
        words.extend([0xF400_0004, 0xFA00_00B8, S_ENDPGM]);
        bytes(&words)
    }

    #[test]
    fn an_es_and_a_gs_container_pair_and_translate() {
        let es = gs::tests::es_container();
        let gs = gs::tests::gs_container(3);
        let out = translate_merged_gs(&es, &gs).unwrap();
        assert_eq!(out.record_dwords, 5);
        let (_, h) = merged_halves(&es, &gs).unwrap();
        assert_eq!((h.es.dwords, h.vertices), (5, 2));
    }

    #[test]
    fn a_gs_half_without_output_records_is_refused_with_its_reason() {
        let es = container(&[(HwStage::Es, &es_half()), (HwStage::Vs, &bytes(&[S_ENDPGM]))]);
        let gs = container(&[(HwStage::Gs, &gs_half())]);
        let r = translate_merged_gs(&es, &gs);
        assert!(matches!(r, Err(ShaderError::Unsupported { what, .. })
            if what == "a geometry program that emits no vertex"), "{r:?}");
    }

    #[test]
    fn an_es_half_that_does_not_hand_over_stops_the_pair_before_the_gs_half() {
        let es = container(&[(HwStage::Es, &bytes(&[S_ENDPGM]))]);
        let gs = container(&[(HwStage::Gs, &gs_half())]);
        let r = translate_merged_gs(&es, &gs);
        assert!(matches!(r, Err(ShaderError::Unsupported { what, .. })
            if what == "an ES half that does not hand over to the GS half"), "{r:?}");
    }

    #[test]
    fn containers_that_are_not_an_es_and_a_gs_do_not_pair() {
        let es = container(&[(HwStage::Es, &es_half())]);
        let vs = container(&[(HwStage::Vs, &bytes(&[S_ENDPGM]))]);
        let gs = container(&[(HwStage::Gs, &gs_half())]);
        for (a, b) in [(&gs, &gs), (&vs, &gs), (&es, &vs), (&gs, &es)] {
            assert!(translate_merged_gs(a, b).is_err(), "paired");
        }
    }

    #[test]
    fn other_unbound_pointers_still_fail_where_they_did() {
        // Not the runtime table: user data 12 and 13 hold no parameter.
        let gs = container(&[(HwStage::Gs, &load(20))]);
        assert!(matches!(
            translate(&gs),
            Err(ShaderError::Unsupported { what, .. })
                if what == "an address in an SGPR that is no root parameter"
        ));
        // The same load in the hardware VS, whose SGPR 8 is no parameter either.
        let vs = container(&[(HwStage::Vs, &load(8))]);
        assert!(matches!(
            translate(&vs),
            Err(ShaderError::Unsupported { what, .. })
                if what == "an address in an SGPR that is no root parameter"
        ));
    }
}
