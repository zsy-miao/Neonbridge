//! DXIL emission (decision 0003, step 3): a console vertex, pixel, compute
//! or mesh shader as a PC DXIL container (SM 6.6) that host D3D12 accepts.
//! Compute shaders keep the console's thread-group size; one that depends
//! on the lane layout (cross-lane operations) declares the console's wave
//! size (`WaveSize`), so the host device must support it: [`Target`] says
//! which wave sizes the host runs, and a shader that needs another one does
//! not translate for it (rather than being refused at pipeline creation).
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
//!   address the title sets is the buffer's. An indexed access through a
//!   descriptor the code builds around one reads at the index times the
//!   stride the code puts in the descriptor.
//! * **Vertex fetch** (added 2026-11-23). Grand Theft Auto VI's mesh and
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

mod bitcode;
mod container;
#[cfg(all(test, windows))]
mod diff_tests;
#[cfg(test)]
mod memory_tests;
#[cfg(all(test, windows))]
mod image_tests;
#[cfg(all(windows, any(test, feature = "host")))]
pub mod host;
#[cfg(all(test, windows))]
mod lds_tests;
#[cfg(all(test, windows))]
mod lane_read_loop_tests;
mod lower;
mod rectangle;
#[cfg(test)]
mod opcode_audit;
#[cfg(all(test, windows))]
mod opcode_tests;
mod sampler;
#[cfg(all(test, windows))]
mod scratch_tests;
mod shader;
#[cfg(all(test, windows))]
mod split_tests;
mod threading;
#[cfg(windows)]
pub mod validator;
#[cfg(all(test, windows))]
mod valu_tests;
mod wave;

use crate::dxbc::{Container, FourCC};
use crate::native::{HwStage, NativeProgram, RootType};
use crate::{ShaderError, io, ir, resource, structure};

pub use rectangle::rectangle_geometry;
pub use sampler::StaticSampler;
pub use shader::{ROOT_CBV_SPACE, SAMPLER_SPACE, STRIDE_SPACE, USER_DATA_SPACE};

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

/// The clean-up passes that run over a translated program before it is
/// written (`bitcode::opt`): a set of that module's pass bits. Every pass
/// is exact, so the switch is for telling a translator fault from a
/// clean-up fault, and for measuring what the clean-up is worth.
/// `VICTORY_SHADER_OPT`:
/// - unset: every pass.
/// - `0`: none (the module as the lowering made it).
/// - a number (decimal or `0x` hex): those passes; 1 dead code, 2 constants
///   and identities, 4 one instruction for one value, 8 what a branch taken
///   says about its condition, 16 one read for one place within a block.
///
/// Part of the translation: the shader cache keys on it (`victory-gpu`'s
/// does).
pub fn shader_opt() -> u32 {
    static PASSES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *PASSES.get_or_init(|| parse_shader_opt(std::env::var("VICTORY_SHADER_OPT").ok().as_deref()))
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

/// Whether programs that reach memory by address (ray tracing and what
/// builds its structures) translate: `VICTORY_RAYS=1`.
// shortcut: off until they run without stopping the host GPU.
pub fn rays() -> bool {
    static RAYS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *RAYS.get_or_init(|| cfg!(test) || std::env::var("VICTORY_RAYS").is_ok_and(|v| v == "1"))
}

/// Slots the host's view heaps have at least (`victory-gpu` pads them); the
/// last is always null. The title's heap is 1,245,184 slots, with
/// `victory-gpu`'s own slots after it.
pub const VIEW_SLOTS: u32 = 2_294_784;

/// Those before the views of memory by address, which programs without
/// such views stay below.
pub const OWN_SLOTS: u32 = VIEW_SLOTS - 1024;


/// Whether heap indices are kept below [`VIEW_SLOTS`]: skinning programs
/// take them from title data, and one past the heap stops the host GPU.
fn limits_indices() -> bool {
    !cfg!(test)
}

/// A translated shader and what the host binds for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShader {
    pub stage: HwStage,
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
    /// True when the shader reads the runtime stride table (`t0` in
    /// [`STRIDE_SPACE`]).
    pub stride_table: bool,
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

/// The wave sizes (lanes per wave) the host GPU runs, as D3D12 reports them
/// (`D3D12_FEATURE_DATA_D3D12_OPTIONS1::WaveLaneCountMin` / `Max`). A
/// compute shader that declares the console's wave size translates only
/// for a host whose range holds it: NVIDIA's range is 32..32, so a console
/// wave64 program that reads other lanes needs 64-lane waves it cannot get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Target {
    pub lanes_min: u32,
    pub lanes_max: u32,
}

impl Target {
    /// No limit known: every wave size is taken to run (the host refuses
    /// a pipeline it cannot run).
    pub const ANY: Target = Target {
        lanes_min: 4,
        lanes_max: 128,
    };

    /// Whether the host runs waves of `lanes`.
    pub fn runs(self, lanes: u32) -> bool {
        (self.lanes_min..=self.lanes_max).contains(&lanes)
    }

    /// The range as bytes, for cache keys.
    pub fn key(self) -> [u8; 8] {
        let mut k = [0; 8];
        k[..4].copy_from_slice(&self.lanes_min.to_le_bytes());
        k[4..].copy_from_slice(&self.lanes_max.to_le_bytes());
        k
    }
}

impl Default for Target {
    fn default() -> Self {
        Target::ANY
    }
}

/// Translates the vertex, pixel, compute or mesh program of a console
/// shader container, for any host ([`Target::ANY`]).
pub fn translate(container: &[u8]) -> Result<HostShader, ShaderError> {
    translate_for(container, Target::ANY)
}

/// [`translate`] for a host that runs the wave sizes of `target`.
pub fn translate_for(container: &[u8], target: Target) -> Result<HostShader, ShaderError> {
    let c = Container::parse(container)?;
    let native = crate::container_programs(&c)?
        .into_iter()
        .find(|p| {
            matches!(
                p.stage,
                HwStage::Vs | HwStage::Ps | HwStage::Cs | HwStage::Ms
            )
        })
        .ok_or(ShaderError::Unsupported {
            offset: 0,
            what: "a container without a vertex, pixel, compute or mesh program",
        })?;
    let mut p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
    // Spilled user-data tables belong to the ray-tracing BVH builder
    // (`XboxBvhBuilder.dll`). Running only a subset of its stages while
    // `VICTORY_RAYS` is off lets later stages (e.g. `c42a31951201f9a0`) read
    // uninitialized data from dropped stages and page-fault the GPU.
    if rays() {
        crate::unspill::unspill(&mut p, &native.layout, native.stage);
    }
    crate::sgpr_spill::forward(&mut p);
    let traces = resource::traces_rays(&p);
    let io = io::raise(&p, &native)?;
    let st = structure::structure(&p)?;
    let with = |addresses: bool| {
        let res = resource::track_with(&p, &native.layout, native.stage, addresses)?;
        emit(&c, &native, &p, &st, &io, &res, target)
    };
    let compute = native.stage == HwStage::Cs && rays();
    let plain = match with(traces && compute) {
        Err(ShaderError::Unsupported { .. }) if compute && !traces => with(true),
        Err(ShaderError::Unsupported { .. }) if compute => with(false),
        done => return done,
    };
    plain
}

/// Emits a program whose earlier stages already ran, for `target`.
pub fn emit(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
    target: Target,
) -> Result<HostShader, ShaderError> {
    let original = emit_program(c, native, p, st, io, res, target);
    // Retry unsupported programs only. Threading can separate an unknown
    // register value from its apparent use on a mutually exclusive path.
    if !matches!(original, Err(ShaderError::Unsupported { .. })) {
        return original;
    }
    let edges = threading::constant_edges(p, st);
    if edges.is_empty() {
        return original;
    }
    // Renaming from registers undoes forwarded reloads: forward them again.
    let mut p = ir::thread_edges(p, &edges)?;
    crate::sgpr_spill::forward(&mut p);
    let st = structure::structure(&p)?;
    let io = io::raise(&p, native)?;
    let res = resource::track_with(&p, &native.layout, native.stage, res.addresses)?;
    emit_program(c, native, &p, &st, &io, &res, target)
}

fn emit_program(
    c: &Container,
    native: &NativeProgram,
    p: &ir::Program,
    st: &structure::Structured,
    io: &io::Io,
    res: &resource::Resources,
    target: Target,
) -> Result<HostShader, ShaderError> {
    let sig = |fourcc| -> Result<Vec<container::SigElement>, ShaderError> {
        match c.part(fourcc) {
            Some(part) => signature(part.data),
            None => Ok(Vec::new()),
        }
    };
    let pixel = native.stage == HwStage::Ps;
    let mut inputs = sig(FourCC::ISG1)?;
    let mut outputs = sig(FourCC::OSG1)?;
    // Interpolation as DXC declares it by default; pixel inputs that are
    // read get the mode their barycentrics say. Conservative depth outputs
    // (`SV_DepthGreaterEqual`/`SV_DepthLessEqual`) require centroid or sample
    // interpolation on `SV_Position`.
    let conservative_depth = pixel && outputs.iter().any(|o| matches!(o.kind, 18 | 19));
    let psv_modes = c
        .part(FourCC::PSV0)
        .and_then(|p| psv_interp(p.data, pixel));
    let interpolated = if pixel { &mut inputs } else { &mut outputs };
    for e in interpolated.iter_mut() {
        e.interp = match (e.kind, e.comp) {
            (3, _) if conservative_depth => 5,
            (3, _) => 4,
            // Constant, as DXC declares it (the AMD compiler crashed on a
            // front face without a mode).
            (13, _) => 1,
            // Until a read says which mode (`lower`).
            (28, _) => 2,
            (0 | 6 | 7, 9) => 2,
            (0, _) => 1,
            // Integer system values (primitive id, array indices) are
            // constant pixel inputs, as DXC declares them, so they share
            // a row with the front face or a constant attribute.
            (_, c) if pixel && c != 9 => 1,
            _ => 0,
        };
        if let Some(modes) = &psv_modes
            && let Some(&(_, _, _, m)) = modes
                .iter()
                .find(|&&(row, col, kind, m)| row == e.row && col == e.col && kind == e.kind && m != 0)
        {
            e.interp = if e.kind == 3 && conservative_depth && m == 4 {
                5
            } else {
                m
            };
        }
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
    // when the HLSL group fills its waves (Grand Theft Auto VI's do).
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
    let mut parts = lower::Parts {
        psv: c.part(FourCC::PSV0).map(|p| p.data),
        native,
        p,
        st,
        io,
        res,
        inputs,
        outputs,
        prims,
        mesh_stride,
        target,
        console_cf: false,
        per_lane_only: false,
    };
    let l = match lower::lower(&parts) {
        Err(ShaderError::Unsupported { what, .. }) if what == lower::CONSOLE_FLOW => {
            let console = structure::structure_console(p);
            let lowered = match &console {
                Ok(console) => {
                    parts.st = console;
                    parts.console_cf = true;
                    lower::lower(&parts)
                }
                Err(e) => Err(ShaderError::Unsupported {
                    offset: 0,
                    what: match e {
                        ShaderError::Unsupported { what, .. } => what,
                        _ => "a program the console structure cannot hold",
                    },
                }),
            };
            match lowered {
                Ok(l) => l,
                // What the console flow cannot lower keeps the per-lane form
                // where that translates; else the console flow's reason.
                Err(e) => {
                    parts.st = st;
                    parts.console_cf = false;
                    parts.per_lane_only = true;
                    lower::lower(&parts).map_err(|_| e)?
                }
            }
        }
        l => l?,
    };
    Ok(HostShader {
        stage: native.stage,
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
        stride_table: l.stride_table,
    })
}

/// Interpolation modes of the input (for pixel) or output (for vertex/mesh)
/// signature elements carried in a container's `PSV0` part: `(row, col, kind,
/// interp)`.
fn psv_interp(part: &[u8], pixel: bool) -> Option<Vec<(Option<u32>, u8, u8, u8)>> {
    use crate::dxbc::u32_at;
    let info_sz = u32_at(part, 0).ok()? as usize;
    if info_sz < 32 {
        return None;
    }
    let info = part.get(4..4 + info_sz)?;
    let (n_in, n_out, n_prim) = (info[28] as usize, info[29] as usize, info[30] as usize);
    let mut pos = 4 + info_sz;
    let n_bind = u32_at(part, pos).ok()? as usize;
    pos += 4;
    if n_bind > 0 {
        let bsz = u32_at(part, pos).ok()? as usize;
        pos = pos.checked_add(4)?.checked_add(n_bind.checked_mul(bsz)?)?;
    }
    let str_sz = u32_at(part, pos).ok()? as usize;
    pos = pos.checked_add(4)?.checked_add(str_sz)?;
    let idx_cnt = u32_at(part, pos).ok()? as usize;
    pos = pos.checked_add(4)?.checked_add(idx_cnt.checked_mul(4)?)?;
    if n_in + n_out + n_prim == 0 {
        return Some(Vec::new());
    }
    let esz = u32_at(part, pos).ok()? as usize;
    pos += 4;
    if esz < 14 {
        return None;
    }
    let (start_i, count) = if pixel { (0, n_in) } else { (n_in, n_out) };
    let mut out = Vec::with_capacity(count);
    for i in start_i..start_i + count {
        let off = pos.checked_add(i.checked_mul(esz)?)?;
        let e = part.get(off..off + esz)?;
        let alloc = e[10] & 0x40 != 0;
        let row = alloc.then_some(u32::from(e[9]));
        let col = if alloc { (e[10] >> 4) & 3 } else { 0 };
        out.push((row, col, e[11], e[13]));
    }
    Some(out)
}

/// The native binding map gives a register but no space. Require every
/// covering declaration of that register class to describe a structured buffer.
fn declares_structured_buffer(part: &[u8], register: u32, uav: bool) -> bool {
    use crate::dxbc::u32_at;
    let table = || {
        let info = u32_at(part, 0).ok()? as usize;
        let header = part.get(4..)?.get(info..)?;
        let count = u32_at(header, 0).ok()? as usize;
        let stride = u32_at(header, 4).ok()? as usize;
        if stride < 16 {
            return None;
        }
        Some(
            header
                .get(8..)?
                .get(..count.checked_mul(stride)?)?
                .chunks_exact(stride),
        )
    };
    let Some(entries) = table() else { return false };
    let mut found = false;
    for entry in entries {
        let kind = u32_at(entry, 0).expect("binding header");
        let class = if uav { 6..=9 } else { 3..=5 };
        if class.contains(&kind)
            && (u32_at(entry, 8).unwrap()..=u32_at(entry, 12).unwrap()).contains(&register)
        {
            if kind != if uav { 8 } else { 5 } {
                return false;
            }
            found = true;
        }
    }
    found
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
        // DXBC encodes HLSL min16*/min10* types with a 32-bit ComponentType
        // (1..=3) and a non-zero D3D_MIN_PRECISION at +28 (1 FLOAT_16,
        // 2 FLOAT_2_8, 4 SINT_16, 5 UINT_16, 240 ANY_16, 241 ANY_10).
        // DXIL declares them as 32-bit with MinPrecision = 0.
        let min_prec = u32_at(part, at + 28)?;
        if !matches!(min_prec, 0 | 1 | 2 | 4 | 5 | 240 | 241) {
            return Err(ShaderError::Unsupported {
                offset: 0,
                what: "an unknown minimum-precision signature element",
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
    use super::Target;
    use super::bitcode::Attr;
    use super::lower::LOOP_LIMIT;
    use super::parse_loop_limit;

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
        let res = resource::track(&p, &native.layout, native.stage).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        let out = emit(&c, &native, &p, &st, &io, &res, Target::ANY).unwrap();
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
    /// (`s_mov_b32 exec_hi, 0`) before a vector instruction. The new mask is
    /// opaque and keyed by the unchanged low dword, so building its lane bit
    /// needed the mask's own value: lowering recursed until the stack
    /// overflowed, which takes the whole process down (found by
    /// random-input testing). Now the instruction fails like any other that
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
        let res = resource::track(&p, &native.layout, native.stage).unwrap();
        let io = io::raise(&p, &native).unwrap();
        let st = structure::structure(&p).unwrap();
        let container = crate::dxbc::build(&[]);
        let c = Container::parse(&container).unwrap();
        if let Err(e) = emit(&c, &native, &p, &st, &io, &res, Target::ANY) {
            assert!(e.to_string().contains("its own value"), "{e}");
        }
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
            stage: crate::HwStage::Ps,
            dxil: constant_color(),
            user_data: Vec::new(),
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: Vec::new(),
            root_srvs: Vec::new(),
            root_uavs: Vec::new(),
            stride_table: false,
        };
        let file = |var: &str, stage| {
            std::env::var(var).ok().map(|p| super::HostShader {
                stage,
                dxil: std::fs::read(p).unwrap(),
                user_data: Vec::new(),
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: Vec::new(),
                root_srvs: Vec::new(),
                root_uavs: Vec::new(),
                stride_table: false,
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
            let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
            let io = crate::io::raise(&p, &native).unwrap();
            let st = crate::structure::structure(&p).unwrap();
            let out = super::emit(&c, &native, &p, &st, &io, &res, Target::ANY).unwrap();
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
        let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
        let io = crate::io::raise(&p, &native).unwrap();
        let st = crate::structure::structure(&p).unwrap();
        // Translated or refused with a reason; it must not panic.
        let _ = super::emit(&c, &native, &p, &st, &io, &res, Target::ANY);
    }

    /// A compute program of 64 threads in one group: `words` as GFX10 code.
    pub(super) fn compute(words: &[u32], wave64: bool) -> crate::NativeProgram {
        compute_of(words, wave64, 64)
    }

    /// [`compute`] with `threads` threads in the group.
    pub(super) fn compute_of(words: &[u32], wave64: bool, threads: u32) -> crate::NativeProgram {
        crate::NativeProgram {
            stage: crate::HwStage::Cs,
            wave64,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: crate::native::StageIo::Cs(crate::native::ComputeIo {
                rsrc2: 0,
                threads: [threads, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }

    pub(super) fn emit_for(
        native: &crate::NativeProgram,
        target: Target,
    ) -> Result<super::HostShader, crate::ShaderError> {
        let c = crate::Container {
            digest: [0; 16],
            version: (1, 0),
            parts: vec![],
        };
        let p = crate::ir::build(&native.code, native.wave64, native.vgprs)?;
        let res = crate::resource::track(&p, &native.layout, native.stage)?;
        let io = crate::io::raise(&p, native)?;
        let st = crate::structure::structure(&p)?;
        super::emit(&c, native, &p, &st, &io, &res, target)
    }

    const NVIDIA: Target = Target {
        lanes_min: 32,
        lanes_max: 32,
    };
    const RDNA: Target = Target {
        lanes_min: 32,
        lanes_max: 64,
    };

    #[test]
    fn console_lanes_translate_only_for_hosts_with_that_wave_size() {
        let words = [
            0xD760_0000,
            0x0001_0B00, // v_readlane_b32 s0, v0, 5
            0x7E02_0200, // v_mov_b32 v1, s0
            0xBF81_0000, // s_endpgm
        ];
        // One console wave per group: in the console flow on NVIDIA.
        let wave64 = compute(&words, true);
        assert!(emit_for(&wave64, Target::ANY).is_ok());
        assert!(emit_for(&wave64, RDNA).is_ok());
        assert!(emit_for(&wave64, NVIDIA).is_ok());
        // Two per group: the halves cannot meet at a group barrier yet.
        let two = compute_of(&words, true, 128);
        assert!(emit_for(&two, RDNA).is_ok());
        assert_eq!(
            emit_for(&two, NVIDIA),
            Err(crate::ShaderError::Unsupported {
                offset: 0,
                what: "console wave64 lanes on a host without 64-lane waves",
            })
        );
        // The same code in waves of 32 runs on either.
        let wave32 = compute(&words, false);
        assert!(emit_for(&wave32, NVIDIA).is_ok());
        assert!(emit_for(&wave32, RDNA).is_ok());
    }

    #[test]
    #[cfg(windows)]
    fn what_translates_for_the_host_creates_a_pipeline() {
        let Ok(host) = super::host::Host::new(false) else {
            eprintln!("no SM 6.6 device; skipped");
            return;
        };
        let target = host.target();
        eprintln!("host waves {}-{}", target.lanes_min, target.lanes_max);
        let words = [0xD760_0000, 0x0001_0B00, 0x7E02_0200, 0xBF81_0000];
        for wave64 in [false, true] {
            match emit_for(&compute_of(&words, wave64, 128), target) {
                Ok(cs) => {
                    if let Err(e) = host.compute_pipeline(&cs) {
                        panic!("wave64 {wave64}: translated for the host, refused by it: {e}");
                    }
                }
                Err(e) => {
                    assert!(wave64 && !target.runs(64), "{e}");
                    // What it replaces: the host refuses the console's
                    // wave size at pipeline creation.
                    let any = emit_for(&compute_of(&words, true, 128), Target::ANY).unwrap();
                    assert!(host.compute_pipeline(&any).is_err());
                }
            }
        }
    }

    /// On every hardware adapter: a wave64 program that reads other lanes
    /// translates in groups of one console wave (natively or in the console
    /// flow) and the adapter takes it; in groups of two it translates
    /// exactly when the adapter runs 64-lane waves. On a machine with an AMD
    /// and an NVIDIA GPU this checks both answers.
    #[test]
    #[cfg(windows)]
    fn each_adapter_gets_what_it_runs() {
        let Ok(names) = super::host::adapters() else {
            eprintln!("no DXGI; skipped");
            return;
        };
        let words = [0xD760_0000, 0x0001_0B00, 0x7E02_0200, 0xBF81_0000];
        for name in names {
            let Ok(host) = super::host::Host::on(&name) else {
                eprintln!("{name}: no SM 6.6 device; skipped");
                continue;
            };
            let target = host.target();
            eprintln!("{name}: waves {}-{}", target.lanes_min, target.lanes_max);
            let one =
                emit_for(&compute(&words, true), target).unwrap_or_else(|e| panic!("{name}: {e}"));
            host.compute_pipeline(&one)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            match emit_for(&compute_of(&words, true, 128), target) {
                Ok(cs) => {
                    assert!(target.runs(64), "{name}");
                    host.compute_pipeline(&cs)
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                }
                Err(_) => assert!(!target.runs(64), "{name}"),
            }
        }
    }

    #[test]
    fn programs_reading_no_other_lanes_translate_for_any_wave_size() {
        let words = [
            0x7E02_0300, // v_mov_b32 v1, v0
            0xBF81_0000, // s_endpgm
        ];
        assert!(emit_for(&compute(&words, true), NVIDIA).is_ok());
    }

    /// The VALU instructions added for packing, dot products and ldexp
    /// translate in a pixel shader whose colour depends on their result
    /// (their values are checked on a GPU by `valu_tests`).
    #[test]
    fn packing_dot_and_ldexp_instructions_translate() {
        let vop1 = |vdst: u32, src0: u32| 0x7E00_0000 | vdst << 17 | 1 << 9 | src0;
        // v1..v3 = 1.0, 2.0, 0.5; then the instruction into v4.
        let prologue = [vop1(1, 242), vop1(2, 244), vop1(3, 240)];
        let operands = 257 | 258 << 9 | 259 << 18;
        let vop3 = |op: u32| [0x35 << 26 | op << 16 | 4, operands];
        let vop3p = |op: u32| [0x33 << 26 | op << 16 | 1 << 14 | 4, operands | 3 << 27];
        let cases: [(&str, [u32; 2]); 10] = [
            ("v_mul_hi_i32_i24", vop3(0x10a)),
            ("v_mul_hi_u32_u24", vop3(0x10c)),
            ("v_ldexp_f32", vop3(0x362)),
            ("v_cvt_pk_u16_u32", vop3(0x36a)),
            ("v_cvt_pk_i16_i32", vop3(0x36b)),
            ("v_dot2_i32_i16", vop3p(0x14)),
            ("v_dot2_u32_u16", vop3p(0x15)),
            ("v_dot4_i32_i8", vop3p(0x16)),
            ("v_dot4_u32_u8", vop3p(0x17)),
            ("v_dot8_u32_u4", vop3p(0x19)),
        ];
        let target = SigElement {
            name: "SV_Target".into(),
            index: 0,
            kind: 16,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 0,
            half: false,
        };
        let osg1 = super::container::signature(&[target], false);
        let bytes = crate::dxbc::build(&[(super::FourCC::OSG1, &osg1)]);
        let c = super::Container::parse(&bytes).unwrap();
        for (name, inst) in cases {
            // exp mrt0 v4, v4, v4, v4 done vm; s_endpgm
            let words = [
                &prologue[..],
                &inst,
                &[0xF800_180F, 0x0404_0404, 0xBF81_0000],
            ]
            .concat();
            let native = super::NativeProgram {
                stage: super::HwStage::Ps,
                wave64: false,
                vgprs: 8,
                layout: vec![],
                used: vec![],
                io: crate::native::StageIo::Ps {
                    inputs: vec![],
                    input_addr: 2,
                    db_shader_control: 0,
                    patches: vec![],
                },
                code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            };
            let p = crate::ir::build(&native.code, false, native.vgprs).unwrap();
            let decoded = crate::gfx10::decode(&native.code).unwrap();
            assert_eq!(decoded[3].name(), Some(name));
            let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
            let io = crate::io::raise(&p, &native).unwrap();
            let st = crate::structure::structure(&p).unwrap();
            if let Err(e) = super::emit(&c, &native, &p, &st, &io, &res, crate::emit::Target::ANY) {
                panic!("{name}: {e}");
            }
        }
    }

    #[test]
    fn scalar_bitops_b64_execute_on_gpu() {
        let words: &[u32] = &[
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0x0
            0xBE8C_0400, // s_mov_b64 s[12:13], s[0:1]
            0xBE8C_1EA1, // s_bitset1_b64 s[12:13], 33
            0xBE8C_1CBF, // s_bitset0_b64 s[12:13], 63
            0xBE8C_1E82, // s_bitset1_b64 s[12:13], 2
            0xBE8C_1C80, // s_bitset0_b64 s[12:13], 0
            0xBE8E_0C02, // s_brev_b64 s[14:15], s[2:3]
            0xBE90_3B02, // s_bitreplicate_b64_b32 s[16:17], s2
            0x7E00_020C, // v_mov_b32 v0, s12
            0x7E02_020D, // v_mov_b32 v1, s13
            0x7E04_020E, // v_mov_b32 v2, s14
            0x7E06_020F, // v_mov_b32 v3, s15
            0x7E08_0280, // v_mov_b32 v4, 0
            0xE078_1000,
            0x8002_0004, // buffer_store_dwordx4 v[0:3], v4, s[8:11], 0 offen
            0x7E00_0210, // v_mov_b32 v0, s16
            0x7E02_0211, // v_mov_b32 v1, s17
            0x7E04_0280, // v_mov_b32 v2, 0
            0x7E06_0280, // v_mov_b32 v3, 0
            0x7E08_0290, // v_mov_b32 v4, 16
            0xE078_1000,
            0x8002_0004, // buffer_store_dwordx4 v[0:3], v4, s[8:11], 0 offen
            0xBF81_0000, // s_endpgm
        ];
        let native = super::NativeProgram {
            stage: super::HwStage::Cs,
            wave64: false,
            vgprs: 5,
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
            used: vec![crate::UsedResource {
                kind: crate::ResourceType::Uav,
                root: crate::RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 4,
                dwords: 4,
                user_data: 4,
                slot: 1,
            }],
            io: crate::native::StageIo::Cs(crate::native::ComputeIo {
                rsrc2: 5 << 1,
                threads: [1, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = crate::ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
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
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
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
                stage: super::HwStage::Cs,
                dxil,
                user_data: l.user_data,
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: l.root_cbvs,
                root_srvs: l.root_srvs,
                root_uavs: l.root_uavs,
                stride_table: l.stride_table,
            };
            let (root, pso) = host.compute_pipeline(&cs).unwrap();
            let out = host
                .dispatch(
                    &root,
                    &pso,
                    &super::host::Dispatch {
                        roots: vec![(
                            0,
                            super::host::Root::Constants(vec![
                                0x0000_0001,
                                0x8000_0000,
                                0x8000_0001,
                                0x0000_0003,
                                0,
                            ]),
                        )],
                        views: &[super::host::View::RawUav(64)],
                        samplers: &[],
                        groups: [1, 1, 1],
                    },
                )
                .unwrap();
            assert_eq!(
                &out[0][..6],
                &[
                    0x0000_0004,
                    0x0000_0002,
                    0xC000_0000,
                    0x8000_0001,
                    0x0000_0003,
                    0xC000_0000,
                ]
            );
        }
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
        let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
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
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
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
                stage: super::HwStage::Cs,
                dxil,
                user_data: l.user_data,
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: l.root_cbvs,
                root_srvs: l.root_srvs,
                root_uavs: l.root_uavs,
                stride_table: l.stride_table,
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

    #[test]
    fn structured_buffer_unknown_stride_uses_runtime_stride_table() {
        let words: &[u32] = &[
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0x0
            0xF408_0302,
            0xFA00_0020, // s_load_dwordx4 s[12:15], s[4:5], 0x20
            0x7E00_0282, // v_mov_b32 v0, 2
            0x7E02_0284, // v_mov_b32 v1, 4
            0xE030_3000,
            0x8002_0200, // buffer_load_dword v2, v[0:1], s[8:11], 0 idxen offen
            0x7E00_0281, // v_mov_b32 v0, 1
            0x7E02_0288, // v_mov_b32 v1, 8
            0xE070_3000,
            0x8003_0200, // buffer_store_dword v2, v[0:1], s[12:15], 0 idxen offen
            0x7E00_0282, // v_mov_b32 v0, 2
            0x7E02_0280, // v_mov_b32 v1, 0
            0x7E06_028A, // v_mov_b32 v3, 10
            0xE0C8_3000,
            0x8003_0300, // buffer_atomic_add v3, v[0:1], s[12:15], 0 idxen offen
            0xE0C8_3000,
            0x8003_0300, // buffer_atomic_add v3, v[0:1], s[12:15], 0 idxen offen
            0xBF81_0000, // s_endpgm
        ];
        let native = super::NativeProgram {
            stage: super::HwStage::Cs,
            wave64: false,
            vgprs: 4,
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
                    stride: 0,
                    dwords: 4,
                    user_data: 4,
                    slot: 1,
                },
                crate::UsedResource {
                    kind: crate::ResourceType::Uav,
                    root: crate::RootType::Table,
                    table_offset_dwords: 8,
                    lower_bound: 0,
                    stride: 0,
                    dwords: 4,
                    user_data: 4,
                    slot: 1,
                },
            ],
            io: crate::native::StageIo::Cs(crate::native::ComputeIo {
                rsrc2: 5 << 1,
                threads: [1, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = crate::ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
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
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
        })
        .unwrap();
        assert!(l.stride_table);
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
                stage: super::HwStage::Cs,
                dxil,
                user_data: l.user_data,
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: l.root_cbvs,
                root_srvs: l.root_srvs,
                root_uavs: l.root_uavs,
                stride_table: l.stride_table,
            };
            let (root, pso) = host.compute_pipeline(&cs).unwrap();
            let srv_data: Vec<u32> = (100..112).collect();
            let out = host
                .dispatch(
                    &root,
                    &pso,
                    &super::host::Dispatch {
                        roots: vec![
                            (0, super::host::Root::Constants(vec![0, 0, 0, 0, 0])),
                            (
                                1,
                                super::host::Root::View {
                                    srv: true,
                                    data: vec![12, 12],
                                },
                            ),
                        ],
                        views: &[
                            super::host::View::Buffer {
                                data: &srv_data,
                                stride: super::host::Stride::Raw,
                            },
                            super::host::View::RawUav(64),
                        ],
                        samplers: &[],
                        groups: [1, 1, 1],
                    },
                )
                .unwrap();
            assert_eq!(out[0][5], 107);
            assert_eq!(out[0][6], 20);
        }
    }

    #[test]
    fn valu_mad64_executes_on_gpu() {
        let words: &[u32] = &[
            0xBE85_0380, // s_mov_b32 s5, 0
            0xF408_0202,
            0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0x0
            0x7E14_0200, // v_mov_b32 v10, s0
            0x7E16_0283, // v_mov_b32 v11, 3
            0x7E18_0201, // v_mov_b32 v12, s1
            0x7E1A_0202, // v_mov_b32 v13, s2
            0xD576_0C00,
            0x0432_170A, // v_mad_u64_u32 v[0:1], s12, v10, v11, v[12:13]
            0xD501_0002,
            0x0031_0280, // v_cndmask_b32_e64 v2, 0, 1, s12
            0x7E1C_02C3, // v_mov_b32 v14, -3
            0x7E1E_0285, // v_mov_b32 v15, 5
            0x7E20_0299, // v_mov_b32 v16, 25
            0x7E22_0280, // v_mov_b32 v17, 0
            0xD577_0E04,
            0x0442_1F0E, // v_mad_i64_i32 v[4:5], s14, v14, v15, v[16:17]
            0xD501_0006,
            0x0039_0280, // v_cndmask_b32_e64 v6, 0, 1, s14
            0x7E24_0280, // v_mov_b32 v18, 0
            0x7E26_0203, // v_mov_b32 v19, s3
            0xD577_1008,
            0x044A_1F0E, // v_mad_i64_i32 v[8:9], s16, v14, v15, v[18:19]
            0xD501_0007,
            0x0041_0280, // v_cndmask_b32_e64 v7, 0, 1, s16
            0x7E06_0306, // v_mov_b32 v3, v6
            0x7E28_0280, // v_mov_b32 v20, 0
            0xE078_1000,
            0x8002_0014, // buffer_store_dwordx4 v[0:3], v20, s[8:11], 0 offen
            0x7E28_0290, // v_mov_b32 v20, 16
            0x7E0C_0307, // v_mov_b32 v6, v7
            0xE078_1000,
            0x8002_0414, // buffer_store_dwordx4 v[4:7], v20, s[8:11], 0 offen
            0x7E26_0280, // v_mov_b32 v19, 0
            0xD577_1008,
            0x044A_1F0E, // v_mad_i64_i32 v[8:9], s16, v14, v15, v[18:19]
            0xD501_000A,
            0x0041_0280, // v_cndmask_b32_e64 v10, 0, 1, s16
            0x7E28_02A0, // v_mov_b32 v20, 32
            0xE078_1000,
            0x8002_0814, // buffer_store_dwordx4 v[8:11], v20, s[8:11], 0 offen
            0xBF81_0000, // s_endpgm
        ];
        let native = super::NativeProgram {
            stage: super::HwStage::Cs,
            wave64: false,
            vgprs: 21,
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
            used: vec![crate::UsedResource {
                kind: crate::ResourceType::Uav,
                root: crate::RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 4,
                dwords: 4,
                user_data: 4,
                slot: 1,
            }],
            io: crate::native::StageIo::Cs(crate::native::ComputeIo {
                rsrc2: 5 << 1,
                threads: [1, 1, 1],
            }),
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let p = crate::ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let res = crate::resource::track(&p, &native.layout, native.stage).unwrap();
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
            target: crate::emit::Target::ANY,
            psv: None,
            console_cf: false,
            per_lane_only: false,
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
                stage: super::HwStage::Cs,
                dxil,
                user_data: l.user_data,
                samplers: Vec::new(),
                static_samplers: Vec::new(),
                root_cbvs: l.root_cbvs,
                root_srvs: l.root_srvs,
                root_uavs: l.root_uavs,
                stride_table: l.stride_table,
            };
            let (root, pso) = host.compute_pipeline(&cs).unwrap();
            let out = host
                .dispatch(
                    &root,
                    &pso,
                    &super::host::Dispatch {
                        roots: vec![(
                            0,
                            super::host::Root::Constants(vec![
                                0x8000_0002,
                                0xFFFF_FFFC,
                                0xFFFF_FFFE,
                                0x8000_0000,
                                0,
                            ]),
                        )],
                        views: &[super::host::View::RawUav(64)],
                        samplers: &[],
                        groups: [1, 1, 1],
                    },
                )
                .unwrap();
            assert_eq!(&out[0][..7], &[0x8000_0002, 0, 1, 0, 10, 0, 1]);
            assert_eq!(&out[0][8..11], &[(-15i32) as u32, u32::MAX, 1]);
        }
    }
}

#[cfg(test)]
mod barycentric_tests {
    use super::container::SigElement;
    use super::*;
    use crate::native::{PsInput, StageIo, Usage};

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
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        pixel_sig(words, input_addr, &[element("TEXCOORD", 0, 0)], &[target])
    }

    /// The same with the console signatures `inputs` and `outputs`.
    fn pixel_sig(
        words: &[u32],
        input_addr: u32,
        inputs: &[SigElement],
        outputs: &[SigElement],
    ) -> Result<HostShader, ShaderError> {
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![PsInput {
                    usage: Usage::Generic,
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
        let isg1 = container::signature(inputs, true);
        let osg1 = container::signature(outputs, false);
        let bytes = crate::dxbc::build(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
        let res = resource::track(&p, &native.layout, native.stage)?;
        let io = io::raise(&p, &native)?;
        let st = structure::structure(&p)?;
        emit(&c, &native, &p, &st, &io, &res, crate::emit::Target::ANY)
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

    /// The `PSV0` input elements: (kind, start row if allocated, mode).
    fn psv_inputs(shader: &HostShader) -> Vec<(u8, Option<u8>, u8)> {
        let c = Container::parse(&shader.dxil).unwrap();
        let psv = c.part(FourCC::PSV0).unwrap().data;
        let u = |at: usize| u32::from_le_bytes(psv[at..at + 4].try_into().unwrap()) as usize;
        let info = u(0);
        let n_in = psv[4 + 28] as usize;
        let mut at = 4 + info;
        let n_bind = u(at);
        at += 4;
        if n_bind > 0 {
            at += 4 + n_bind * u(at);
        }
        at += 4 + u(at);
        at += 4 + 4 * u(at);
        let size = u(at);
        at += 4;
        (0..n_in)
            .map(|k| {
                let e = &psv[at + k * size..];
                let row = (e[10] & 0x40 != 0).then_some(e[9]);
                (e[11], row, e[13])
            })
            .collect()
    }

    /// A conservative depth output makes the position centroid, which the
    /// validator wants (14 programs were rejected with the center mode).
    #[test]
    fn conservative_depth_declares_a_centroid_position() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            e0,
            e1,
            S_ENDPGM,
        ];
        let mut position = element("SV_Position", 3, 1);
        position.usage = 0;
        let inputs = [element("TEXCOORD", 0, 0), position];
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        for (kind, name) in [(18, "SV_DepthLessEqual"), (19, "SV_DepthGreaterEqual")] {
            let mut depth = element(name, kind, 0);
            (depth.row, depth.cols) = (None, 1);
            let shader =
                pixel_sig(&words, PERSP_CENTER, &inputs, &[target.clone(), depth]).unwrap();
            let pos: Vec<_> = psv_inputs(&shader)
                .into_iter()
                .filter(|e| e.0 == 3)
                .collect();
            assert_eq!(pos, [(3, Some(1), 5)], "{name}");
        }
        // Plain depth or none: noperspective center, as DXC declares it.
        let shader = pixel_sig(&words, PERSP_CENTER, &inputs, &[target]).unwrap();
        assert!(psv_inputs(&shader).contains(&(3, Some(1), 4)));
    }

    /// Every packed row of a pixel shader's inputs has one mode.
    fn assert_one_mode_per_row(shader: &HostShader) {
        let els = psv_inputs(shader);
        for &(kind, row, mode) in &els {
            let Some(row) = row else { continue };
            for &(k, r, m) in &els {
                assert!(
                    r != Some(row) || m == mode,
                    "row {row}: {kind}/{mode} vs {k}/{m}"
                );
            }
        }
    }

    fn texcoord(index: u32, col: u8, cols: u8, comp: u8) -> SigElement {
        SigElement {
            index,
            col,
            cols,
            comp,
            ..element("TEXCOORD", 0, 0)
        }
    }

    /// An attribute read with centroid barycentrics shares its row with
    /// one nobody reads: the unread one takes the centroid mode.
    #[test]
    fn an_unread_element_takes_its_rows_mode() {
        const PERSP_CENTROID: u32 = 1 << 2;
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            e0,
            e1,
            S_ENDPGM,
        ];
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        let inputs = [texcoord(0, 0, 2, 9), texcoord(1, 2, 2, 9)];
        let shader = pixel_sig(&words, PERSP_CENTROID, &inputs, &[target]).unwrap();
        assert_one_mode_per_row(&shader);
        let modes: Vec<u8> = psv_inputs(&shader).iter().map(|e| e.2).collect();
        assert_eq!(modes, [3, 3]);
        validate(&shader);
    }

    /// An interpolated float packed with an integer (constant) element: the
    /// row stays where it is and becomes constant, the float weighed from
    /// its vertices with `SV_Barycentrics`.
    #[test]
    fn an_integer_in_the_row_makes_it_constant() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            e0,
            e1,
            S_ENDPGM,
        ];
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        let mut face = element("SV_IsFrontFace", 13, 0);
        (face.col, face.cols, face.comp) = (3, 1, 5);
        for other in [texcoord(1, 2, 1, 5), face] {
            let inputs = [texcoord(0, 0, 2, 9), other];
            let shader = pixel_sig(&words, PERSP_CENTER, &inputs, &[target.clone()]).unwrap();
            assert_one_mode_per_row(&shader);
            let els = psv_inputs(&shader);
            assert!(els.iter().any(|e| e.0 == 28), "{els:?}");
            assert!(
                els.iter().filter(|e| e.1 == Some(0)).all(|e| e.2 == 1),
                "{els:?}"
            );
            validate(&shader);
        }
    }

    /// A row read both flat (P0) and interpolated becomes constant.
    #[test]
    fn a_flat_and_an_interpolated_read_make_the_row_constant() {
        let [e0, e1] = exp_mrt0(2);
        let words = [
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            0xC816_0202, // v_interp_mov_f32 v5, p0, attr0.z
            v_add_f32(2, 2, 5),
            e0,
            e1,
            S_ENDPGM,
        ];
        let mut target = element("SV_Target", 16, 0);
        target.usage = 15;
        let inputs = [texcoord(0, 0, 2, 9), texcoord(1, 2, 2, 9)];
        let shader = pixel_sig(&words, PERSP_CENTER, &inputs, &[target]).unwrap();
        assert_one_mode_per_row(&shader);
        assert!(
            psv_inputs(&shader)
                .iter()
                .filter(|e| e.0 == 0)
                .all(|e| e.2 == 1)
        );
        validate(&shader);
    }

    /// The settling rules on their own: unread floats follow the fixed
    /// element; two fixed modes in a pixel row are an error; vertex outputs
    /// packing an integer make their floats constant.
    #[test]
    fn row_modes_settle() {
        use super::lower::settle_row_modes;
        let mut read = texcoord(0, 0, 2, 9);
        (read.interp, read.usage) = (6, 1);
        let mut unread = texcoord(1, 2, 2, 9);
        unread.interp = 2;
        let mut els = vec![read.clone(), unread.clone()];
        settle_row_modes(&mut els, true).unwrap();
        assert_eq!((els[0].interp, els[1].interp), (6, 6));
        let mut int = texcoord(1, 2, 1, 5);
        int.interp = 1;
        assert!(settle_row_modes(&mut [read, int.clone()], true).is_err());
        let mut els = vec![unread, int];
        settle_row_modes(&mut els, false).unwrap();
        assert_eq!((els[0].interp, els[1].interp), (1, 1));
    }

    /// Per sample, the conservative-depth position is per sample too.
    #[test]
    fn conservative_depth_per_sample_declares_a_sample_position() {
        let mut position = element("SV_Position", 3, 0);
        position.interp = 5;
        let mut sample = element("SV_SampleIndex", 12, 0);
        (sample.comp, sample.row, sample.cols) = (5, None, 1);
        let mut inputs = vec![position.clone(), sample];
        super::lower::settle_position_mode(&mut inputs);
        assert_eq!(inputs[0].interp, 7);
        // A per-sample interpolation alone does the same.
        let mut texcoord = element("TEXCOORD", 0, 1);
        texcoord.interp = 6;
        let mut inputs = vec![position.clone(), texcoord];
        super::lower::settle_position_mode(&mut inputs);
        assert_eq!(inputs[0].interp, 7);
        // Without either, centroid stays.
        let mut alone = vec![position];
        super::lower::settle_position_mode(&mut alone);
        assert_eq!(alone[0].interp, 5);
    }

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
                &resource::track(&p, &native.layout, native.stage).unwrap(),
                Target::ANY,
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
mod packed_half_tests {
    use super::container::SigElement;
    use super::*;
    use crate::native::{PsInput, StageIo, Usage};

    fn input(row: u8, mask: u8, attr: u8, flat: bool, fp16: bool) -> PsInput {
        PsInput {
            usage: Usage::Generic,
            row,
            mask,
            attr,
            flat,
            fp16,
            pack2: fp16,
            // The console repeats the row when it names no partner.
            row2: row,
        }
    }

    fn element(index: u32, row: u32, cols: u8, comp: u8, half: bool) -> SigElement {
        SigElement {
            name: "TEXCOORD".into(),
            index,
            kind: 0,
            comp,
            interp: 0,
            row: Some(row),
            col: 0,
            cols,
            usage: 0,
            half,
        }
    }

    /// A pixel program with the stage data of shaders that failed with "a
    /// packed input half no element holds": the only 16-bit input (attr1,
    /// row 3, two dwords of halves, pack2 with row2 == row) and 32-bit
    /// inputs in rows 2, 4, 5 and 7. The signature declares the 16-bit
    /// half2 at row 3 and its partner, the half2 at row 6 no input names
    /// (as the vertex side packs it, `half_outputs`): dword 0 of the
    /// attribute holds row 3's x and y, dword 1 row 6's x and y.
    fn pixel(words: &[u32]) -> Result<HostShader, ShaderError> {
        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 8,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs: vec![
                    input(2, 0xf, 0, true, false),
                    input(3, 0x3, 1, false, true),
                    input(4, 0xf, 2, false, false),
                    input(5, 0xf, 3, false, false),
                    input(7, 0x1, 4, true, false),
                ],
                // PERSP_CENTER, POS_X, POS_Y, FRONT_FACE.
                input_addr: 4866,
                db_shader_control: 80,
                patches: vec![],
            },
            code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let isg1 = [
            element(0, 2, 4, 5, false),
            element(1, 3, 2, 9, true),
            element(2, 4, 4, 9, false),
            element(3, 5, 4, 9, false),
            element(4, 6, 2, 9, true),
            element(5, 7, 1, 5, false),
        ];
        let mut isg1 = container::signature(&isg1, true);
        // Declare rows 3 and 6 float16 (`signature` writes only 32-bit types).
        for k in 0..6 {
            let at = 8 + 32 * k;
            if matches!(isg1[at + 20], 3 | 6) {
                isg1[at + 16..at + 20].copy_from_slice(&6u32.to_le_bytes());
            }
        }
        let target = SigElement {
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
        };
        let osg1 = container::signature(&[target], false);
        let bytes = crate::dxbc::build(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)]);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
        let res = resource::track(&p, &native.layout, native.stage)?;
        let io = io::raise(&p, &native)?;
        let st = structure::structure(&p)?;
        emit(&c, &native, &p, &st, &io, &res, crate::emit::Target::ANY)
    }

    /// Every half of the packed attribute resolves: the high half of dword
    /// 0 to row 3's y, both halves of dword 1 to row 6's x and y.
    #[test]
    fn packed_halves_with_row2_equal_to_row_read_the_unlisted_partner() {
        let words = [
            0xD742_0002, // v_interp_p1ll_f16 v2, v0, attr1.y
            0x0002_0041,
            0xD75A_0002, // v_interp_p2_f16 v2, v1, attr1.y, v2
            0x040A_0241,
            0xD742_0003, // v_interp_p1ll_f16 v3, v0, attr1.y high
            0x0002_0141,
            0xD75A_0003, // v_interp_p2_f16 v3, v1, attr1.y, v3 high
            0x040E_0341,
            0xD742_0004, // v_interp_p1ll_f16 v4, v0, attr1.x high
            0x0002_0101,
            0xD75A_0004, // v_interp_p2_f16 v4, v1, attr1.x, v4 high
            0x0412_0301,
            0x7E04_1702, // v_cvt_f32_f16_e32 v2, v2
            0x7E06_1703, // v_cvt_f32_f16_e32 v3, v3
            0x7E08_1704, // v_cvt_f32_f16_e32 v4, v4
            0xF800_180F, // exp mrt0 v2, v3, v4, v4 done vm
            0x0404_0302,
            0xBF81_0000, // s_endpgm
        ];
        let shader = pixel(&words).unwrap();
        let c = Container::parse(&shader.dxil).unwrap();
        let isg1 = c.part(FourCC::ISG1).unwrap().data;
        let n = crate::dxbc::u32_at(isg1, 0).unwrap() as usize;
        // Read channels per register.
        let rw = |row: u32| {
            (0..n)
                .map(|k| 8 + 32 * k)
                .filter(|&at| crate::dxbc::u32_at(isg1, at + 20).unwrap() == row)
                .map(|at| isg1[at + 25])
                .fold(0, |a, b| a | b)
        };
        assert_eq!(rw(3), 0b10, "row 3 reads only y");
        assert_eq!(rw(6), 0b11, "row 6 reads x and y");
        #[cfg(windows)]
        if let Ok(mut v) =
            validator::Validator::load(validator::Validator::default_path().as_deref())
        {
            v.validate(&mut shader.dxil.clone()).unwrap();
        }
    }

    #[test]
    fn signature_accepts_known_min_precision_and_rejects_unknown() {
        let base = SigElement {
            name: "TEXCOORD".into(),
            index: 0,
            kind: 0,
            comp: 9,
            interp: 0,
            row: Some(0),
            col: 0,
            cols: 4,
            usage: 0,
            half: false,
        };
        for (min_prec, comp_type, expected_comp) in [
            (1u32, 3u32, 9u8), // D3D_MIN_PRECISION_FLOAT_16
            (2, 3, 9),         // D3D_MIN_PRECISION_FLOAT_2_8
            (4, 2, 4),         // D3D_MIN_PRECISION_SINT_16
            (5, 1, 5),         // D3D_MIN_PRECISION_UINT_16
            (240, 3, 9),       // D3D_MIN_PRECISION_ANY_16
            (241, 3, 9),       // D3D_MIN_PRECISION_ANY_10
        ] {
            let mut part = super::container::signature(std::slice::from_ref(&base), true);
            // Row starts at offset 8: +16 is ComponentType, +28 is MinPrecision.
            part[8 + 16..8 + 20].copy_from_slice(&comp_type.to_le_bytes());
            part[8 + 28..8 + 32].copy_from_slice(&min_prec.to_le_bytes());
            let parsed = super::signature(&part).unwrap();
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0].comp, expected_comp);
            assert!(!parsed[0].half);
        }

        let mut bad = super::container::signature(&[base], true);
        bad[8 + 28..8 + 32].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(
            super::signature(&bad),
            Err(ShaderError::Unsupported {
                offset: 0,
                what: "an unknown minimum-precision signature element",
            })
        );
    }
}

/// Tests from #306 (its fixes were already here: the centroid position under
/// conservative depth, the validator's signature order, a16 biased samples).
#[cfg(test)]
mod pr306_tests {
    use super::container::SigElement;
    use super::*;
    use crate::native::{RootType, StageIo};

    #[cfg(windows)]
    fn validate_if_available(dxil: &[u8]) {
        let path = super::validator::Validator::default_path();
        let Ok(mut v) = super::validator::Validator::load(path.as_deref()) else {
            eprintln!("no dxil.dll; skipped");
            return;
        };
        let mut copy = dxil.to_vec();
        let ours = copy[4..20].to_vec();
        v.validate(&mut copy)
            .unwrap_or_else(|e| panic!("dxil.dll rejected shader:\n{e}"));
        assert_eq!(copy[4..20], ours[..], "container hash disagrees");
    }

    fn emit_synthetic(
        parts: &[(FourCC, &[u8])],
        native: &NativeProgram,
    ) -> Result<HostShader, ShaderError> {
        let bytes = crate::dxbc::build(parts);
        let c = Container::parse(&bytes).unwrap();
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let res = resource::track(&p, &native.layout, native.stage).unwrap();
        let io = io::raise(&p, native).unwrap();
        let st = structure::structure(&p).unwrap();
        emit(&c, native, &p, &st, &io, &res, crate::emit::Target::ANY)
    }

    fn sig_elem(
        name: &str,
        index: u32,
        kind: u8,
        comp: u8,
        row: Option<u32>,
        col: u8,
        cols: u8,
    ) -> SigElement {
        SigElement {
            name: name.into(),
            index,
            kind,
            comp,
            interp: 0,
            row,
            col,
            cols,
            usage: 0,
            half: false,
        }
    }

    #[test]
    fn conservative_depth_sets_position_centroid_and_validates() {
        for (depth_name, depth_kind) in
            [("SV_DepthLessEqual", 18u8), ("SV_DepthGreaterEqual", 19u8)]
        {
            // v_mov_b32 v0, 0.5; exp mrtz v0, off, off, off done; s_endpgm
            let words = [
                0x7E00_02F0u32, // v_mov_b32 v0, 0.5
                0xF800_0881,    // exp mrtz (target 8), en=0x1, done=1
                0x0000_0000,    // vsrc0=v0
                0xBF81_0000,    // s_endpgm
            ];
            let native = NativeProgram {
                stage: HwStage::Ps,
                wave64: false,
                vgprs: 4,
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
            let isg1 =
                container::signature(&[sig_elem("SV_Position", 0, 3, 9, Some(0), 0, 4)], true);
            let osg1 =
                container::signature(&[sig_elem(depth_name, 0, depth_kind, 9, None, 0, 1)], false);
            let host =
                emit_synthetic(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)], &native).unwrap();
            #[cfg(windows)]
            validate_if_available(&host.dxil);
        }
    }

    #[test]
    fn signature_part_preserves_element_order_within_row_and_validates() {
        // Two elements in row 1 where the earlier element has a higher column
        // (col=3, w) than the later element (col=0, xyz).
        let row1_elems = [
            sig_elem("SV_Position", 0, 3, 9, Some(0), 0, 4),
            sig_elem("TEXCOORD", 3, 0, 9, Some(1), 3, 1),
            sig_elem("NORMAL", 0, 0, 9, Some(1), 0, 3),
        ];
        let isg1 = container::signature(&row1_elems, true);
        let parsed = signature(&isg1).unwrap();
        assert_eq!(parsed[1].name, "TEXCOORD");
        assert_eq!(parsed[2].name, "NORMAL");

        // Validate through full PS emission so dxil.dll verifies ISG1 matches
        // the DXIL signature metadata byte for byte.
        // v_mov_b32 v0, 0; exp mrt0 v0, v0, v0, v0 done; s_endpgm
        let ps_words = [0x7E00_0280u32, 0xF800_080F, 0x0000_0000, 0xBF81_0000];
        let ps_native = NativeProgram {
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
            code: ps_words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let osg1 = container::signature(&[sig_elem("SV_Target", 0, 16, 9, Some(0), 0, 4)], false);
        let ps =
            emit_synthetic(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)], &ps_native).unwrap();
        #[cfg(windows)]
        validate_if_available(&ps.dxil);

        // Also validate a VS whose OSG1 has the same out-of-column row order.
        // v_mov_b32 v0, 0; exp pos0 v0..v0 done; exp param0 v0..v0; s_endpgm
        let vs_words = [
            0x7E00_0280u32,
            0xF800_00CF, // exp pos0 (target 12), en=0xF
            0x0000_0000,
            0xF800_0A0F, // exp param0 (target 32), en=0xF, done=1
            0x0000_0000,
            0xBF81_0000,
        ];
        let vs_native = NativeProgram {
            stage: HwStage::Vs,
            wave64: false,
            vgprs: 2,
            layout: vec![],
            used: vec![],
            io: StageIo::Vs {
                inputs: vec![],
                outputs: vec![crate::native::VsOutput {
                    usage: crate::native::Usage::Generic,
                    row: 1,
                    param: 0,
                    fp16: false,
                    pack2: false,
                }],
            },
            code: vs_words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        };
        let vs_osg1 = container::signature(&row1_elems, false);
        let vs = emit_synthetic(&[(FourCC::OSG1, &vs_osg1)], &vs_native).unwrap();
        #[cfg(windows)]
        validate_if_available(&vs.dxil);
    }

    #[test]
    fn biased_16bit_image_sample_unpacks_f32_bias_and_f16_coords_and_validates() {
        // Synthetic PS executing `image_sample_b v0, v[1:2], s[4:11], s[12:15]
        // dmask:0x1 dim:SQ_RSRC_IMG_2D a16` where v1 is a 32-bit f32 bias
        // (-1.0) and v2 holds packed f16 (u, v) = (0.5, 0.5), then exporting v0.
        let words = [
            0xBE81_0380u32, // s_mov_b32 s1, 0
            0xF40C_0100,    // s_load_dwordx8 s[4:11], s[0:1], 0
            0xFA00_0000,    // soffset=NULL, offset=0
            0xBE8C_0380,    // s_mov_b32 s12, 0
            0xBE8D_0380,    // s_mov_b32 s13, 0
            0xBE8E_0380,    // s_mov_b32 s14, 0
            0xBE8F_0380,    // s_mov_b32 s15, 0
            0x7E02_02F3,    // v_mov_b32 v1, -1.0 (32-bit f32 bias)
            0x7E04_02FF,    // v_mov_b32 v2, lit(0x3800_3800) (packed f16 0.5, 0.5)
            0x3800_3800,
            // MIMG image_sample_b (op=0x25): dmask=0x1, dim=1 (2D), a16=1 (bit 30 of w1)
            0xF000_0000 | (0x25 << 18) | (1 << 8) | (1 << 3),
            (1 << 30) | (3 << 21) | (1 << 16) | 1, // vaddr=v1 (v[1:2]), vdata=v0
            0xF800_080F,                           // exp mrt0 v0, v0, v0, v0 done
            0x0000_0000,
            0xBF81_0000, // s_endpgm
        ];
        // Sanity-check the synthetic encoding decodes as `image_sample_b ... a16`.
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let decoded = crate::gfx10::decode(&code).unwrap();
        let sample = decoded
            .iter()
            .find(|i| i.name() == Some("image_sample_b"))
            .expect("image_sample_b decoded");
        assert!(matches!(
            sample.fields(false),
            crate::gfx10::Fields::Image {
                a16: true,
                dim: 1,
                ..
            }
        ));

        let native = NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 4,
            layout: vec![crate::native::RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 0,
                slot: 0,
            }],
            used: vec![crate::native::UsedResource {
                kind: crate::native::ResourceType::Srv,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 8,
                dwords: 8,
                user_data: 0,
                slot: 0,
            }],
            io: StageIo::Ps {
                inputs: vec![],
                input_addr: 0,
                db_shader_control: 0,
                patches: vec![],
            },
            code,
        };
        let osg1 = container::signature(&[sig_elem("SV_Target", 0, 16, 9, Some(0), 0, 4)], false);
        let ps = emit_synthetic(&[(FourCC::OSG1, &osg1)], &native).unwrap();
        #[cfg(windows)]
        validate_if_available(&ps.dxil);
    }
}
