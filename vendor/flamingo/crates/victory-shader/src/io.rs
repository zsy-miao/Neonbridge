//! I/O raising (decision 0003): what a program's input registers, its
//! interpolation and its exports mean.
//!
//! Pixel shaders get the VGPRs `SPI_PS_INPUT_ADDR` enables, in a fixed
//! order (barycentrics, then position, front face...). `v_interp_p1` and
//! `v_interp_p2` (and their f16 forms) evaluate an attribute from a pair of
//! barycentrics; `v_interp_mov` reads one vertex's value. After the 32-bit
//! export patch ([`NativeProgram::patched_code`]) exports carry plain
//! 32-bit values: `mrt<n>` is render target n, `mrtz` depth, stencil and
//! sample mask, `null` nothing. A lane whose EXEC bit is clear at the
//! export that ends the program (`done`) is discarded: that export's guard
//! after structurisation.
//!
//! Vertex shaders that run as the hardware VS get the vertex id in v0, the
//! primitive id in v2 and the instance id in v3; the fetch shader loads
//! channel k of each attribute into its VGPR + k. `exp pos0` is the
//! position, `exp param<n>` the output with parameter n.
//!
//! Attributes and outputs are named by their row: a register of the
//! container's signature part.
//!
//! Compute programs get the thread id in the group in v0-v2 and the group
//! id in the SGPRs after the user data, as `COMPUTE_PGM_RSRC2` enables them.
//!
//! Mesh programs run as one NGG stage. The program computes its thread
//! index from the wave index (s3 bits 24-27) and the lane (`v_mbcnt`), and
//! its group id from v5 of the first lane (`v_readfirstlane`: the group
//! times the group size). Lane n exports vertex n (`exp pos`, `exp param`)
//! and primitive n (`exp prim`: 9-bit vertex indices at bits 0, 10, 20,
//! each followed by its edge flag; bit 31 culls); `param` entries with bit 7 are per-primitive outputs. One
//! lane sends the output counts (`s_sendmsg` GS_ALLOC_REQ, m0 = vertices |
//! primitives << 12).

use crate::ShaderError;
use crate::gfx10::{Fields, Operand};
use crate::ir::{Exit, Inst, Program, Reg, Value, operand_reads};
use crate::native::{HwStage, NativeProgram, PsInput, StageIo, Usage};
use crate::resource::Site;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    VertexId,
    PrimitiveId,
    InstanceId,
    /// A vertex attribute the fetch shader loads.
    Attribute {
        row: u8,
        channel: u8,
    },
    /// `index` 0 is i, 1 is j (2 the pull model's w).
    Barycentric {
        kind: Barycentric,
        index: u8,
    },
    /// Pixel position channel (0-3: x, y, z, w).
    Position(u8),
    FrontFace,
    /// Sample id and other bits.
    Ancillary,
    SampleCoverage,
    FixedPosition,
    LineStipple,
    /// Compute: thread-group id component (an SGPR).
    GroupId(u8),
    /// Compute: thread id within the group, component (a VGPR).
    ThreadIdInGroup(u8),
    /// Compute: the thread-group info SGPR (wave index and count).
    GroupInfo,
    /// Mesh: v5, the group id times the group size (read as uniform).
    MeshGroup,
    /// Mesh: s3, the wave's index in the group in bits 24-27.
    MergedWaveInfo,
    /// Geometry: dword 0 (low) or 1 (high) of the pointer to the console
    /// runtime's table, in the first two user-data SGPRs
    /// ([`crate::merged::RUNTIME_TABLE`]).
    RuntimeTable(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Barycentric {
    PerspectiveSample,
    PerspectiveCenter,
    PerspectiveCentroid,
    PerspectivePullModel,
    LinearSample,
    LinearCenter,
    LinearCentroid,
}

/// The pixel input VGPRs in hardware order: one entry per
/// `SPI_PS_INPUT_ADDR` bit.
const PS_VGPRS: [(u8, Option<Barycentric>); 16] = [
    (2, Some(Barycentric::PerspectiveSample)),
    (2, Some(Barycentric::PerspectiveCenter)),
    (2, Some(Barycentric::PerspectiveCentroid)),
    (3, Some(Barycentric::PerspectivePullModel)),
    (2, Some(Barycentric::LinearSample)),
    (2, Some(Barycentric::LinearCenter)),
    (2, Some(Barycentric::LinearCentroid)),
    (1, None), // line stipple
    (1, None), // position x
    (1, None), // y
    (1, None), // z
    (1, None), // w
    (1, None), // front face
    (1, None), // ancillary
    (1, None), // sample coverage
    (1, None), // fixed-point position
];

fn ps_system(bit: usize) -> Input {
    match bit {
        7 => Input::LineStipple,
        8..=11 => Input::Position(bit as u8 - 8),
        12 => Input::FrontFace,
        13 => Input::Ancillary,
        14 => Input::SampleCoverage,
        _ => Input::FixedPosition,
    }
}

/// An attribute channel of the input signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attribute {
    pub usage: Usage,
    pub row: u8,
    pub channel: u8,
    /// The upper f16 half of the channel (packed fp16 inputs).
    pub high: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interp {
    /// This site's result (`v_interp_p2*`) is the attribute interpolated
    /// with `barycentric`; `first` are the `v_interp_p1*` halves it
    /// completes (one per path where the compiler shares a first half).
    Interpolated {
        attribute: Attribute,
        barycentric: Barycentric,
        first: Vec<Site>,
    },
    /// One vertex's value (`v_interp_mov_f32`), as encoded: 0 for P10,
    /// 1 for P20, 2 for P0 (the provoking vertex).
    Vertex { attribute: Attribute, vertex: u8 },
    /// A half of an interpolation whose i or j is computed rather than
    /// loaded: each half evaluates as the hardware does, from the
    /// attribute's vertex values (`v_interp_p1*`: P0 + i P10, with P10 =
    /// P1 - P0; `v_interp_p2*`: its old dst + j P20, with P20 = P2 - P0).
    Weighted { attribute: Attribute },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Color(u8),
    /// Depth, stencil, sample mask (channels as `DB_SHADER_CONTROL` enables).
    Depth,
    Null,
    Position(u8),
    Param {
        usage: Usage,
        row: u8,
    },
    /// `param0` of a vertex shader without outputs: the hardware wants one
    /// parameter export regardless.
    Unread,
    /// The primitive export of a merged or mesh stage.
    Primitive,
    /// A mesh program's per-primitive output (a row of `PSG1`).
    PrimitiveParam {
        usage: Usage,
        row: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub site: Site,
    pub target: Target,
    /// Per channel, as the instruction enables them.
    pub values: [Option<Value>; 4],
    /// Two f16 values per dword.
    pub compressed: bool,
    /// The program's last export (for pixels: kills the lanes not in EXEC).
    pub done: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Io {
    /// Input values the hardware loads, with their meaning; other VGPRs
    /// read before written hold undefined values.
    pub inputs: Vec<(Value, Input)>,
    pub interps: Vec<(Site, Interp)>,
    pub exports: Vec<Export>,
    /// Compute: how many SGPRs the hardware initializes
    /// ([`ComputeIo::initialized_sgprs`](crate::native::ComputeIo::initialized_sgprs)).
    pub initialized_sgprs: Option<u8>,
}

/// Raises the I/O of `p`, built from `native`'s patched code. Fails for
/// stages whose stage data is not read and on constructs not covered yet.
pub fn raise(p: &Program, native: &NativeProgram) -> Result<Io, ShaderError> {
    let unsupported = |offset, what| ShaderError::Unsupported { offset, what };
    let (loaded, ps_inputs, params) = match (&native.io, native.stage) {
        (
            StageIo::Ps {
                inputs, input_addr, ..
            },
            HwStage::Ps,
        ) => {
            // A pixel shader exports no parameters; `[]` never reaches `Unread`.
            let loaded = ps_vgprs(*input_addr)
                .into_iter()
                .map(|(r, i)| (Reg::V(r), i))
                .collect();
            (loaded, &inputs[..], &[][..])
        }
        // The ES half of a merged stage taken as a vertex shader whose
        // exports are the records it writes (`handoff`): its vertex id in v0
        // as the hardware VS gets it (not checked on the title's programs),
        // the runtime table pointer where the layout leaves its dwords free.
        (StageIo::Vs { inputs, outputs }, HwStage::Vs | HwStage::Es) => {
            let mut loaded = if native.stage == HwStage::Es {
                let first = native.stage.user_sgpr(crate::merged::RUNTIME_TABLE);
                let taken = native.layout.iter().any(|p| {
                    let at = u32::from(native.stage.user_sgpr(p.user_data));
                    at < u32::from(first) + 2 && u32::from(first) < at + p.sgprs
                });
                let mut v = vec![(Reg::V(0), Input::VertexId)];
                if !taken {
                    v.extend((0..2u8).map(|k| (Reg::S(first + k), Input::RuntimeTable(k))));
                }
                v
            } else {
                vec![
                    (Reg::V(0), Input::VertexId),
                    (Reg::V(2), Input::PrimitiveId),
                    (Reg::V(3), Input::InstanceId),
                ]
            };
            for i in inputs {
                for channel in (0..4).filter(|c| i.mask >> c & 1 != 0) {
                    let input = Input::Attribute {
                        row: i.row,
                        channel,
                    };
                    loaded.push((Reg::V(i.vgpr + channel), input));
                }
            }
            (loaded, &[][..], &outputs[..])
        }
        // Compute: thread ids in v0-v2, group ids and the group info in the
        // SGPRs after the user data.
        (StageIo::Cs(cs), HwStage::Cs) => {
            let (ids, info) = cs.system_sgprs();
            let mut loaded: Vec<(Reg, Input)> = (0..cs.thread_id_vgprs())
                .map(|c| (Reg::V(c), Input::ThreadIdInGroup(c)))
                .collect();
            for (c, s) in ids.into_iter().enumerate() {
                if let Some(s) = s {
                    loaded.push((Reg::S(s), Input::GroupId(c as u8)));
                }
            }
            loaded.extend(info.map(|s| (Reg::S(s), Input::GroupInfo)));
            (loaded, &[][..], &[][..])
        }
        (StageIo::Ms(ms), HwStage::Ms) => {
            // The group input, and the phis that may carry it past a join.
            let mut group: Vec<Value> = p
                .inputs
                .iter()
                .filter(|i| i.0 == Reg::V(5))
                .map(|i| i.1)
                .collect();
            // No lane is active over the EXEC-zero edge of s_cbranch_execz or
            // _execnz, the same case as a partial-EXEC write below.
            let no_lanes = |q: usize, b: usize| match (
                p.blocks[q].insts.last().map(Inst::name),
                p.blocks[q].exit,
            ) {
                (Some("s_cbranch_execz"), Exit::Branch { taken, not_taken }) => {
                    taken == b && not_taken != b
                }
                (Some("s_cbranch_execnz"), Exit::Branch { taken, not_taken }) => {
                    not_taken == b && taken != b
                }
                _ => false,
            };
            while let Some(v) = p
                .blocks
                .iter()
                .enumerate()
                .flat_map(|(b, k)| k.phis.iter().map(move |f| (b, f)))
                .find_map(|(b, f)| {
                    let mut args = f.args.iter().zip(&p.blocks[b].preds);
                    let carried = args.any(|(a, &q)| group.contains(a) && !no_lanes(q, b));
                    (carried && !group.contains(&f.value)).then_some(f.value)
                })
            {
                group.push(v);
            }
            // The group input holds the same value in every lane only as the
            // first lane reads it. A write to v5 reads its old value too, which
            // only lanes outside EXEC keep.
            // shortcut: writes that may keep the old v5 in active lanes
            // (v_fmac_f32's accumulator, a 16-bit half or d16 load,
            // v_writelane_b32, v_movreld_b32, DPP that skips lanes) and
            // partial-EXEC writes pass; check those too if a mesh program
            // writes v5 that way.
            let per_lane = p.blocks.iter().flat_map(|b| &b.insts).find(|i| {
                i.read(Reg::V(5)).is_some_and(|v| group.contains(&v))
                    && i.name() != "v_readfirstlane_b32"
                    && (i.write(Reg::V(5)).is_none() || operand_reads(p, i).contains(&Reg::V(5)))
            });
            if let Some(i) = per_lane {
                return Err(unsupported(
                    i.machine.offset,
                    "the mesh group input read per lane",
                ));
            }
            let loaded = vec![
                (Reg::V(5), Input::MeshGroup),
                (Reg::S(3), Input::MergedWaveInfo),
            ];
            (loaded, &[][..], &ms.outputs[..])
        }
        // The halves of a merged ES+GS stage share the SGPRs: no exports (the
        // ES half hands its records over through LDS, the GS half writes its
        // output vertices there); the runtime table pointer is the one input
        // known. The root layout wins where it covers those dwords.
        (StageIo::Gs(_), HwStage::Gs) | (StageIo::Es(_), HwStage::Es) => {
            let first = native.stage.user_sgpr(crate::merged::RUNTIME_TABLE);
            let taken = native.layout.iter().any(|p| {
                let at = u32::from(native.stage.user_sgpr(p.user_data));
                // (the count is read from the container: no overflow)
                at < u32::from(first) + 2 && u32::from(first) < at.saturating_add(p.sgprs)
            });
            let loaded = (0..2u8)
                .filter(|_| !taken)
                .map(|k| (Reg::S(first + k), Input::RuntimeTable(k)))
                .collect();
            (loaded, &[][..], &[][..])
        }
        _ => return Err(unsupported(0, "I/O of this stage")),
    };
    let inputs: Vec<(Value, Input)> = p
        .inputs
        .iter()
        .filter_map(|&(reg, v)| loaded.iter().find(|l| l.0 == reg).map(|l| (v, l.1)))
        .collect();
    let input_of = |v: Value| inputs.iter().find(|i| i.0 == v).map(|i| i.1);
    let attribute = |inst: &Inst, attr: &Operand| match *attr {
        Operand::Attr { index, chan, high } => ps_inputs
            .iter()
            .find(|i: &&PsInput| i.attr == index)
            .map(|i| Attribute {
                usage: i.usage,
                row: i.row,
                channel: chan,
                high,
            })
            .ok_or(unsupported(
                inst.machine.offset,
                "an interpolated attribute the stage data does not list",
            )),
        _ => Err(unsupported(inst.machine.offset, "an interpolation operand")),
    };
    // A VGPR value's barycentric kind and index, if it is one as loaded.
    let barycentric = |v: Option<Value>| match v.and_then(input_of) {
        Some(Input::Barycentric { kind, index }) => Some((kind, index)),
        _ => None,
    };

    let mut io = Io {
        inputs: inputs.clone(),
        initialized_sgprs: match &native.io {
            StageIo::Cs(cs) => Some(cs.initialized_sgprs()),
            _ => None,
        },
        ..Io::default()
    };
    let defs: std::collections::HashMap<Value, Site> = p
        .blocks
        .iter()
        .enumerate()
        .flat_map(|(b, block)| {
            block
                .insts
                .iter()
                .enumerate()
                .flat_map(move |(i, inst)| inst.writes.iter().map(move |w| (w.1, (b, i))))
        })
        .collect();
    let phis: std::collections::HashMap<Value, (usize, usize)> = p
        .blocks
        .iter()
        .enumerate()
        .flat_map(|(b, block)| {
            block
                .phis
                .iter()
                .enumerate()
                .map(move |(i, phi)| (phi.value, (b, i)))
        })
        .collect();
    let halves = Halves {
        p,
        defs: &defs,
        phis: &phis,
    };
    let mut preserved = Vec::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            let site = (b, i);
            let at = inst.machine.offset;
            let name = inst.name();
            match &inst.fields {
                Fields::Vintrp { dst, src, attr } => {
                    let attribute = attribute(inst, attr)?;
                    match (name, *src) {
                        ("v_interp_mov_f32", Operand::Const(vertex)) => {
                            io.interps
                                .push((site, Interp::Vertex { attribute, vertex }));
                        }
                        ("v_interp_p1_f32", _) => {}
                        ("v_interp_p2_f32", Operand::Vector { reg, .. }) => {
                            let Operand::Vector { reg: d, .. } = *dst else {
                                return Err(unsupported(at, "an interpolation operand"));
                            };
                            // The first half's result is this one's old dst.
                            let interp = halves
                                .pair(
                                    attr,
                                    inst.read(Reg::V(d)),
                                    inst.read(Reg::V(reg)),
                                    &barycentric,
                                )
                                .map_err(|what| unsupported(at, what))?;
                            push_pair(&mut io, site, interp, attribute);
                        }
                        _ => return Err(unsupported(at, "an interpolation form")),
                    }
                }
                Fields::Valu(v) if name.starts_with("v_interp_") => match name {
                    "v_interp_p1ll_f16" => {}
                    "v_interp_p2_f16" => {
                        let attribute = attribute(inst, &v.src[0])?;
                        let read = |k: usize| match v.src.get(k) {
                            Some(&Operand::Vector { reg, .. }) => inst.read(Reg::V(reg)),
                            _ => None,
                        };
                        let interp = halves
                            .pair(&v.src[0], read(2), read(1), &barycentric)
                            .map_err(|what| unsupported(at, what))?;
                        push_pair(&mut io, site, interp, attribute);
                    }
                    _ => return Err(unsupported(at, "an interpolation form")),
                },
                Fields::Valu(v)
                    if native.stage == HwStage::Ps
                        && matches!(name, "v_fma_mixlo_f16" | "v_fma_mixhi_f16") =>
                {
                    if let Some(Operand::Vector { reg, .. }) = v.dst
                        && let Some(old) = inst.read(Reg::V(reg))
                    {
                        preserved.push(old);
                    }
                }
                Fields::Exp {
                    target,
                    src,
                    compr,
                    done,
                    ..
                } => {
                    let target = match *target {
                        t @ 0..=7 => Target::Color(t),
                        8 => Target::Depth,
                        9 => Target::Null,
                        t @ 12..=16 => Target::Position(t - 12),
                        20 => Target::Primitive,
                        32 if params.is_empty() => Target::Unread,
                        t @ 32..=63 => {
                            // Bit 7: a mesh program's per-primitive output.
                            let o = params.iter().find(|o| o.param & 0x7F == t - 32).ok_or(
                                unsupported(at, "a parameter export the stage data does not list"),
                            )?;
                            let (usage, row) = (o.usage, o.row);
                            if o.param & 0x80 != 0 {
                                Target::PrimitiveParam { usage, row }
                            } else {
                                Target::Param { usage, row }
                            }
                        }
                        _ => return Err(unsupported(at, "an export target")),
                    };
                    let values = src.map(|s| match s {
                        Some(Operand::Vector { reg, .. }) => inst.read(Reg::V(reg)),
                        _ => None,
                    });
                    io.exports.push(Export {
                        site,
                        target,
                        values,
                        compressed: *compr,
                        done: *done,
                    });
                }
                _ => {}
            }
        }
    }
    // A mixed-f16 write preserves the other half of its old destination.
    // If that value came from an elided P1, evaluate P1 as well as its P2.
    let mut seen = std::collections::HashSet::new();
    while let Some(v) = preserved.pop() {
        if !seen.insert(v) {
            continue;
        }
        if let Some(&(b, i)) = phis.get(&v) {
            preserved.extend_from_slice(&p.blocks[b].phis[i].args);
        } else if let Some(&site @ (b, i)) = defs.get(&v) {
            let inst = &p.blocks[b].insts[i];
            let attr = match &inst.fields {
                Fields::Vintrp { attr, .. } if inst.name() == "v_interp_p1_f32" => attr,
                Fields::Valu(v) if inst.name() == "v_interp_p1ll_f16" => &v.src[0],
                _ => continue,
            };
            if !io.interps.iter().any(|(s, _)| *s == site) {
                let attribute = attribute(inst, attr)?;
                io.interps.push((site, Interp::Weighted { attribute }));
            }
        }
    }
    Ok(io)
}

/// The pixel input VGPRs that `addr` enables, with their meaning.
fn ps_vgprs(addr: u32) -> Vec<(u8, Input)> {
    let mut out = Vec::new();
    let mut next = 0u8;
    for (bit, &(count, kind)) in PS_VGPRS.iter().enumerate() {
        if addr >> bit & 1 == 0 {
            continue;
        }
        for index in 0..count {
            let input = match kind {
                Some(kind) => Input::Barycentric { kind, index },
                None => ps_system(bit),
            };
            out.push((next + index, input));
        }
        next += count;
    }
    out
}

/// An interpolation before its attribute is known: `barycentric` none
/// when i or j is computed.
struct Pair {
    barycentric: Option<Barycentric>,
    first: Vec<Site>,
}

impl Pair {
    fn with(self, attribute: Attribute) -> Interp {
        match self.barycentric {
            Some(barycentric) => Interp::Interpolated {
                attribute,
                barycentric,
                first: self.first,
            },
            None => Interp::Weighted { attribute },
        }
    }
}

/// Records interpolation `pair` at `site`, and its first halves too when
/// they evaluate on their own (computed barycentrics).
fn push_pair(io: &mut Io, site: Site, pair: Pair, attribute: Attribute) {
    if pair.barycentric.is_none() {
        for &f in &pair.first {
            if !io.interps.iter().any(|(s, _)| *s == f) {
                io.interps.push((f, Interp::Weighted { attribute }));
            }
        }
    }
    io.interps.push((site, pair.with(attribute)));
}

/// Finds the first halves of interpolations.
struct Halves<'a> {
    p: &'a Program,
    defs: &'a std::collections::HashMap<Value, Site>,
    phis: &'a std::collections::HashMap<Value, (usize, usize)>,
}

impl Halves<'_> {
    /// The interpolation whose second half reads the first half's result
    /// `acc`, the barycentric `j`, and attribute `attr`: every first half
    /// must read the same attribute channel. With j and every first half's
    /// i loaded as one kind's, the pair interpolates with that kind;
    /// otherwise each half evaluates on its own (`Interp::Weighted`).
    fn pair(
        &self,
        attr: &Operand,
        acc: Option<Value>,
        j: Option<Value>,
        barycentric: &impl Fn(Option<Value>) -> Option<(Barycentric, u8)>,
    ) -> Result<Pair, &'static str> {
        let mut first = Vec::new();
        self.sites(
            acc.ok_or("an interpolation without its first half")?,
            &mut Vec::new(),
            &mut first,
        )?;
        let mut kind = match barycentric(j) {
            Some((kind, 1)) => Some(kind),
            _ => None,
        };
        for &(b, i) in &first {
            let inst = &self.p.blocks[b].insts[i];
            let (a, i) = match &inst.fields {
                Fields::Vintrp {
                    src: Operand::Vector { reg, .. },
                    attr,
                    ..
                } if inst.name() == "v_interp_p1_f32" => (*attr, inst.read(Reg::V(*reg))),
                Fields::Valu(v) if inst.name() == "v_interp_p1ll_f16" => match v.src.get(1) {
                    Some(&Operand::Vector { reg, .. }) => (v.src[0], inst.read(Reg::V(reg))),
                    _ => return Err("an interpolation without its first half"),
                },
                _ => return Err("an interpolation without its first half"),
            };
            if a != *attr {
                return Err("interpolation halves of different attributes");
            }
            if kind.is_some_and(|k| barycentric(i) != Some((k, 0))) {
                kind = None;
            }
        }
        Ok(Pair {
            barycentric: kind,
            first,
        })
    }

    /// The instructions `v` comes from, through phis (`seen`: phis on the
    /// way; a loop back to one adds nothing).
    fn sites(
        &self,
        v: Value,
        seen: &mut Vec<Value>,
        out: &mut Vec<Site>,
    ) -> Result<(), &'static str> {
        if let Some(&site) = self.defs.get(&v) {
            if !out.contains(&site) {
                out.push(site);
            }
            return Ok(());
        }
        let &(b, i) = self
            .phis
            .get(&v)
            .ok_or("an interpolation without its first half")?;
        if !seen.contains(&v) {
            seen.push(v);
            for &a in &self.p.blocks[b].phis[i].args {
                self.sites(a, seen, out)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::ExportPatch;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn program(words: &[u32]) -> Program {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        crate::ir::build(&code, false, 256).unwrap()
    }

    fn pixel(inputs: Vec<PsInput>, input_addr: u32) -> NativeProgram {
        NativeProgram {
            stage: HwStage::Ps,
            wave64: false,
            vgprs: 256,
            layout: vec![],
            used: vec![],
            io: StageIo::Ps {
                inputs,
                input_addr,
                db_shader_control: 0,
                patches: Vec::<ExportPatch>::new(),
            },
            code: vec![],
        }
    }

    const TEXCOORD: PsInput = PsInput {
        usage: Usage::Generic,
        row: 1,
        mask: 3,
        attr: 0,
        flat: false,
        fp16: false,
        pack2: false,
        row2: 1,
    };

    #[test]
    fn interpolation_pairs_and_color_exports() {
        // Blit's pixel shader: TEXCOORD.xy from the perspective-center i/j
        // in v0/v1, then exported unconverted to MRT0.
        let p = program(&[
            0xC808_0000, // v_interp_p1_f32 v2, v0, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            0xF800_180F,
            0x0302_0102, // exp mrt0, v2, v1, v2, v3 done vm
            S_ENDPGM,
        ]);
        let io = raise(&p, &pixel(vec![TEXCOORD], 2)).unwrap();
        let input = |reg| p.inputs.iter().find(|i| i.0 == Reg::V(reg)).unwrap().1;
        let center = |index| Input::Barycentric {
            kind: Barycentric::PerspectiveCenter,
            index,
        };
        assert!(io.inputs.contains(&(input(0), center(0))));
        assert!(io.inputs.contains(&(input(1), center(1))));
        // v2 and v3 are read before written but hold nothing.
        assert!(!io.inputs.iter().any(|i| i.0 == input(3)));
        assert_eq!(
            io.interps,
            [(
                (0, 1),
                Interp::Interpolated {
                    attribute: Attribute {
                        usage: Usage::Generic,
                        row: 1,
                        channel: 0,
                        high: false
                    },
                    barycentric: Barycentric::PerspectiveCenter,
                    first: vec![(0, 0)],
                }
            )]
        );
        let [e] = &io.exports[..] else {
            panic!("{:?}", io.exports)
        };
        assert_eq!(
            (e.target, e.done, e.compressed),
            (Target::Color(0), true, false)
        );
        assert_eq!(e.values[0], p.blocks[0].insts[1].write(Reg::V(2)));
    }

    #[test]
    fn unpaired_or_unknown_interpolation_fails() {
        // p2 alone: its old dst is no p1's result.
        let lone = program(&[0xC809_0001, S_ENDPGM]); // v_interp_p2_f32 v2, v1, attr0.x
        assert!(raise(&lone, &pixel(vec![TEXCOORD], 2)).is_err());
        // attr1 is not in the stage data.
        let other = program(&[0xC808_0400, S_ENDPGM]); // v_interp_p1_f32 v2, v0, attr1.x
        assert!(raise(&other, &pixel(vec![TEXCOORD], 2)).is_err());
    }

    #[test]
    fn computed_barycentrics_evaluate_each_half() {
        // i scaled before the first half: both halves stand on their own.
        let p = program(&[
            0x1006_00F0, // v_mul_f32 v3, 0.5, v0
            0xC808_0003, // v_interp_p1_f32 v2, v3, attr0.x
            0xC809_0001, // v_interp_p2_f32 v2, v1, attr0.x
            S_ENDPGM,
        ]);
        let io = raise(&p, &pixel(vec![TEXCOORD], 2)).unwrap();
        let attribute = Attribute {
            usage: Usage::Generic,
            row: 1,
            channel: 0,
            high: false,
        };
        let mut interps = io.interps.clone();
        interps.sort_by_key(|i| i.0);
        assert_eq!(
            interps,
            [
                ((0, 1), Interp::Weighted { attribute }),
                ((0, 2), Interp::Weighted { attribute }),
            ]
        );
    }

    #[test]
    fn preserved_interpolation_half_can_reach_a_mixed_write_through_a_phi() {
        let p = program(&[
            0xBF84_0004, // s_cbranch_scc0 .join
            0xD742_0002,
            0x0002_0000, // v_interp_p1ll_f16 v2, v0, attr0.x
            0xD75A_0003,
            0x040A_0200, // v_interp_p2_f16 v3, v1, attr0.x, v2
            // .join: the other predecessor holds the entry value of v2.
            0xCC21_0002,
            0x03C9_E4F2, // v_fma_mixlo_f16 v2, 1.0, 1.0, 1.0
            S_ENDPGM,
        ]);
        let entry = p
            .inputs
            .iter()
            .find(|(reg, _)| *reg == Reg::V(2))
            .unwrap()
            .1;
        assert!(
            p.blocks
                .iter()
                .flat_map(|b| &b.phis)
                .any(|phi| { phi.reg == Reg::V(2) && phi.args.contains(&entry) })
        );
        let io = raise(
            &p,
            &pixel(
                vec![PsInput {
                    fp16: true,
                    ..TEXCOORD
                }],
                2,
            ),
        )
        .unwrap();
        assert!(io.interps.iter().any(|&((b, i), ref interp)| {
            p.blocks[b].insts[i].machine.offset == 4 && matches!(interp, Interp::Weighted { .. })
        }));
    }

    #[test]
    fn mesh_inputs_and_primitive_export() {
        let mesh = |words: &[u32]| {
            let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let p = crate::ir::build(&code, true, 256).unwrap();
            let native = NativeProgram {
                stage: HwStage::Ms,
                wave64: true,
                vgprs: 256,
                layout: vec![],
                used: vec![],
                io: StageIo::Ms(crate::native::MeshIo {
                    outputs: vec![],
                    clip_cntl: 0,
                    threads: 128,
                    max_vertices: 128,
                    max_primitives: 128,
                    topology: 2,
                    lds_bytes: 0,
                }),
                code,
            };
            raise(&p, &native)
        };
        let io = mesh(&[
            0x7E04_0505, // v_readfirstlane_b32 s2, v5
            0xF800_0941,
            0x0000_0009, // exp prim, v9, off, off, off done
            S_ENDPGM,
        ])
        .unwrap();
        assert!(io.inputs.iter().any(|i| i.1 == Input::MeshGroup));
        assert_eq!(io.exports[0].target, Target::Primitive);
        // The group input is uniform only as the first lane reads it.
        assert!(mesh(&[0x7E02_0305, S_ENDPGM]).is_err()); // v_mov_b32 v1, v5
        // Also as an operand of a write to v5, or after a join.
        let per_lane: [&[u32]; 5] = [
            &[0x4A0A_0305, S_ENDPGM],              // v_add_nc_u32 v5, v5, v1
            &[0x7E0A_0D05, S_ENDPGM],              // v_cvt_f32_u32 v5, v5
            &[0xDC30_8000, 0x0500_0005, S_ENDPGM], // global_load_dword v5, v5, s[0:1]
            &[0x7E0A_CB01, S_ENDPGM],              // v_swap_b32 v5, v1
            &[
                0xBF06_8001, // s_cmp_eq_u32 s1, 0
                0xBF85_0001, // s_cbranch_scc1 -> 0xc
                0x7E0A_0280, // v_mov_b32 v5, 0
                0x7E02_0305, // 0xc: v_mov_b32 v1, v5
                S_ENDPGM,
            ],
        ];
        let accepted: Vec<_> = per_lane.iter().filter(|w| mesh(w).is_ok()).collect();
        assert!(accepted.is_empty(), "accepted: {accepted:08X?}");
        // Overwriting v5 reads the group input only for lanes outside EXEC:
        // v_mov_b32 v5, 0; v_mov_b32 v1, v5.
        assert!(mesh(&[0x7E0A_0280, 0x7E02_0305, S_ENDPGM]).is_ok());
        // Skipping that write with s_cbranch_execz is the same case: under the
        // same mask, the read sees only the 0.
        let reuse = [
            0x7E04_0505, // v_readfirstlane_b32 s2, v5
            0x7D88_0084, // v_cmp_gt_u32 vcc, 4, v0
            0xBE86_246A, // s_and_saveexec_b64 s[6:7], vcc
            0xBF88_0001, // s_cbranch_execz -> 0x14
            0x7E0A_0280, // v_mov_b32 v5, 0
            0x88FE_067E, // 0x14: s_or_b64 exec, exec, s[6:7]
            0xBE86_246A, // s_and_saveexec_b64 s[6:7], vcc
            0xBF88_0001, // s_cbranch_execz -> 0x24
            0x7E02_0305, // v_mov_b32 v1, v5
            0x88FE_067E, // 0x24: s_or_b64 exec, exec, s[6:7]
            S_ENDPGM,
        ];
        assert_eq!(mesh(&reuse).map(|_| ()), Ok(()));
    }

    #[test]
    fn pixel_vgprs_follow_the_input_address() {
        // Linear center (bit 5), then position x and y (bits 8, 9).
        let v = ps_vgprs(1 << 5 | 1 << 8 | 1 << 9);
        assert_eq!(
            v,
            [
                (
                    0,
                    Input::Barycentric {
                        kind: Barycentric::LinearCenter,
                        index: 0
                    }
                ),
                (
                    1,
                    Input::Barycentric {
                        kind: Barycentric::LinearCenter,
                        index: 1
                    }
                ),
                (2, Input::Position(0)),
                (3, Input::Position(1)),
            ]
        );
    }

    fn geometry(layout: Vec<crate::native::RootParam>) -> NativeProgram {
        NativeProgram {
            stage: HwStage::Gs,
            wave64: true,
            vgprs: 256,
            layout,
            used: vec![],
            io: StageIo::Gs(Default::default()),
            code: vec![],
        }
    }

    #[test]
    fn a_geometry_program_gets_the_runtime_table_and_exports_nothing() {
        // s_load_dword s0, s[8:9], 0xb8
        let p = program(&[0xF400_0004, 0xFA00_00B8, S_ENDPGM]);
        let io = raise(&p, &geometry(vec![])).unwrap();
        let mut got: Vec<_> = io.inputs.iter().map(|i| i.1).collect();
        got.sort_by_key(|i| format!("{i:?}"));
        assert_eq!(got, [Input::RuntimeTable(0), Input::RuntimeTable(1)]);
        assert!(io.exports.is_empty() && io.interps.is_empty());
        // Where the title's root layout covers the dwords, they are its own.
        let own = crate::native::RootParam {
            root: crate::native::RootType::Descriptor,
            table_offset_dwords: 0,
            sgprs: 2,
            base_register: 0,
            user_data: 0,
            slot: 0,
        };
        assert!(raise(&p, &geometry(vec![own])).unwrap().inputs.is_empty());
    }

    #[test]
    fn geometry_io_is_for_the_geometry_stage_only() {
        // Stage data of a geometry program on another stage's program.
        let p = program(&[S_ENDPGM]);
        let mut native = geometry(vec![]);
        native.stage = HwStage::Vs;
        assert!(raise(&p, &native).is_err());
    }
}
