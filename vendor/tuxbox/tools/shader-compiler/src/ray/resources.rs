//! Recover ray-program resource bindings from RDNA2 root-table loads. Each
//! continuation starts with the global root block in s[6:7]. Only bindings
//! agreed upon by all predecessors can be lowered to a static DXR use.
mod lumen;
pub(super) use lumen::{coords, save_components};
mod material;

/// Retail's static local root signature. The first 32 bytes of its arguments
/// are the two geometry addresses and four system constants. The remaining
/// fields are the four eight-byte descriptor-table handles. Compiling these
/// subobjects into DXIL preserves the layout of the guest shader records.
pub(super) fn local_record(export: &str) -> String {
    format!(
        r#"LocalRootSignature tbLocal_{export} = {{
        "SRV(t0,space=2),SRV(t1,space=2),RootConstants(num32BitConstants=4,b0,space=2),"
        "DescriptorTable(SRV(t0,numDescriptors=64,space=0,flags=DESCRIPTORS_VOLATILE|DATA_STATIC_WHILE_SET_AT_EXECUTE)),"
        "DescriptorTable(CBV(b0,numDescriptors=16,space=0,flags=DESCRIPTORS_VOLATILE|DATA_STATIC_WHILE_SET_AT_EXECUTE)),"
        "DescriptorTable(Sampler(s0,numDescriptors=16,space=0,flags=DESCRIPTORS_VOLATILE)),"
        "DescriptorTable(UAV(u0,numDescriptors=16,space=0,flags=DESCRIPTORS_VOLATILE|DATA_STATIC_WHILE_SET_AT_EXECUTE))"
    }};
    SubobjectToExportsAssociation tbLocalExports_{export} = {{ "tbLocal_{export}", "{export}" }};
"#
    )
}

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write,
    ops::Range,
};

use crate::rdna2::{Enc, Insn, Opnd, instructions};
use crate::rootsig::parse_rts0;
use tuxbox_dxbc::{Dxbc, ray::State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Root,
    Table(u32),
    Descriptor(u32, u32),
    ImageRed(u32),
    ConstantWord(usize), // byte offset from b0
    AllChannels,
}

#[derive(Clone)]
struct Registers([Option<Origin>; 384]);

impl Registers {
    fn index(operand: &Opnd) -> Option<usize> {
        match operand {
            Opnd::Sgpr(index) => Some(*index as usize),
            Opnd::Vgpr(index) => Some(128 + *index as usize),
            _ => None,
        }
    }

    fn address(&self, operand: &Opnd) -> Option<Origin> {
        if matches!(operand, Opnd::ConstInt(-1) | Opnd::Literal(u32::MAX)) {
            Some(Origin::AllChannels)
        } else {
            self.0[Self::index(operand)?]
        }
    }

    fn merge(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (left, right) in self.0.iter_mut().zip(other.0) {
            // A disabled light-channel texture contributes all bits set.
            // Keep its use visible at the join with the texture-enabled path.
            let joined = match (*left, right) {
                (a, b) if a == b => a,
                (Some(Origin::ImageRed(index)), Some(Origin::AllChannels))
                | (Some(Origin::AllChannels), Some(Origin::ImageRed(index))) => {
                    Some(Origin::ImageRed(index))
                }
                _ => None,
            };
            if *left != joined {
                *left = joined;
                changed = true;
            }
        }
        changed
    }
}

#[cfg_attr(test, derive(Default))]
pub(super) struct Bindings {
    pub(super) declarations: String,
    instructions: BTreeMap<usize, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Srv {
    Texture,
    Scene,
    NodeRecords,
    PageIndices,
    BufferUint2,
    BufferFloat4,
    Volume,
    Cube,
    Raw,
    Structured(u32), // words per element
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Family {
    #[default]
    General,
    Reflection,
    CardDirectLighting,
    Radiosity,
    RadianceCache {
        traces: bool,
    },
    ScreenProbe {
        traces: bool,
    },
}

#[derive(Clone, Copy)]
enum Output {
    Uint,
    Float,
    Float3,
    Float4,
}

impl Output {
    fn ty(self) -> &'static str {
        match self {
            Self::Uint => "uint",
            Self::Float => "float",
            Self::Float3 => "float3",
            Self::Float4 => "float4",
        }
    }

    fn from_registers(self, first: usize) -> Option<String> {
        let lanes = match self {
            Self::Uint | Self::Float => 1,
            Self::Float3 => 3,
            Self::Float4 => 4,
        };
        if first + lanes > 128 {
            return None;
        }
        let v = |n: usize| format!("v[{}]", first + n);
        Some(match self {
            Self::Uint => v(0),
            Self::Float => format!("asfloat({})", v(0)),
            Self::Float3 => format!("asfloat(uint3({}, {}, {}))", v(0), v(1), v(2)),
            Self::Float4 => format!("asfloat(uint4({}, {}, {}, {}))", v(0), v(1), v(2), v(3)),
        })
    }
}

#[derive(Default)]
struct Usage {
    family: Family,
    record_outputs: BTreeMap<u32, usize>, // DWORDs in each element
    srvs: BTreeMap<u32, Srv>,
    outputs: BTreeSet<u32>,
    volume_outputs: BTreeSet<u32>,
    samplers: BTreeSet<u32>,
    buffer_outputs: BTreeSet<u32>,
    uint_textures: BTreeSet<u32>,
    constants: BTreeMap<u32, usize>, // CBV index => highest accessed word + 1
}

impl Usage {
    fn output(&self, index: u32) -> Option<Output> {
        match self.family {
            Family::Reflection => match index {
                1 | 4 => Some(Output::Float3),
                2 | 5 => Some(Output::Float),
                _ => None,
            },
            Family::ScreenProbe { traces } => match index.checked_sub(u32::from(traces))? {
                0 => Some(Output::Uint),
                1 => Some(Output::Float3),
                _ => None,
            },
            _ => Some(Output::Float4),
        }
    }

    fn srv(&mut self, index: u32, kind: Srv) -> Option<()> {
        if self.srvs.insert(index, kind).is_some_and(|was| was != kind) {
            return None;
        }
        Some(())
    }

    fn declarations(&self, state: &State) -> Option<String> {
        let dxbc = Dxbc::parse(state.root).ok()?;
        let part = dxbc.chunk(b"RTS0")?;
        let root = parse_rts0(state.root.get(part.offset..part.offset + part.size)?).ok()?;
        let mut out = String::new();
        if self
            .srvs
            .values()
            .filter(|&&kind| kind == Srv::Scene)
            .count()
            != usize::from(!matches!(
                self.family,
                Family::RadianceCache { traces: false } | Family::ScreenProbe { traces: false }
            ))
        {
            return None;
        }
        if self.srvs.values().any(|&kind| kind == Srv::NodeRecords) {
            // FPackedVirtualVoxelNodeDesc has two float3 bounds interleaved
            // with packed words at bytes 12 and 28. Keep the native bit layout.
            writeln!(out, "struct NodeRecord {{ uint words[8]; }};").ok()?;
        }
        for (&index, &kind) in &self.srvs {
            let register = super::binding(&root, 0, index, 0)?;
            match kind {
                Srv::Cube => writeln!(out,"TextureCube<float4> sky{index} : {register};"),
                Srv::Raw => writeln!(out,"ByteAddressBuffer raw{index} : {register};"),
                Srv::Structured(words) => writeln!(out,"struct Elem{index} {{ uint words[{words}]; }}; StructuredBuffer<Elem{index}> structured{index} : {register};"),
                Srv::Scene => writeln!(out, "RaytracingAccelerationStructure scene : {register};"),
                Srv::Texture => {
                    let scalar = if self.uint_textures.contains(&index) {
                        "uint"
                    } else {
                        "float"
                    };
                    writeln!(out, "Texture2D<{scalar}4> tex{index} : {register};")
                }
                Srv::NodeRecords => writeln!(
                    out,
                    "StructuredBuffer<NodeRecord> nodes{index} : {register};"
                ),
                Srv::BufferFloat4 => writeln!(out,"Buffer<float4> values{index} : {register};"),
                Srv::BufferUint2 => writeln!(out, "Buffer<uint2> pairs{index} : {register};"),
                Srv::PageIndices => writeln!(out, "Buffer<uint> pages{index} : {register};"),
                Srv::Volume => writeln!(out, "Texture3D<uint> volume{index} : {register};"),
            }
            .ok()?;
        }
        for &index in &self.samplers {
            let register = super::binding(&root, 2, index, 3)?;
            writeln!(out, "SamplerState sampler{index} : {register};").ok()?;
        }
        if self.srvs.values().any(|&kind| kind == Srv::Cube) {
            out.push_str(lumen::CUBE_DIRECTION);
        }
        for (&index, &words) in &self.record_outputs {
            let register = super::binding(&root, 3, index, 1)?;
            writeln!(out, "struct RayRecord{index} {{ uint words[{words}]; }}; RWStructuredBuffer<RayRecord{index}> records{index} : {register};").ok()?;
        }
        for &index in &self.buffer_outputs {
            if self.outputs.contains(&index) || self.volume_outputs.contains(&index) {
                return None;
            }
            let register = super::binding(&root, 3, index, 1)?;
            writeln!(
                out,
                "RWStructuredBuffer<uint> writable{index} : {register};"
            )
            .ok()?;
        }
        for &index in &self.volume_outputs {
            if self.outputs.contains(&index) {
                return None;
            }
            let register = super::binding(&root, 3, index, 1)?;
            let lanes = match index {
                1 => "float3",
                2 => "float",
                _ => return None,
            };
            writeln!(out, "RWTexture3D<{lanes}> target{index} : {register};").ok()?;
        }
        for &index in &self.outputs {
            let register = super::binding(&root, 3, index, 1)?;
            let value = self.output(index)?.ty();
            writeln!(out, "RWTexture2D<{value}> target{index} : {register};").ok()?;
        }
        for (&index, &words) in &self.constants {
            let register = super::binding(&root, 1, index, 2)?;
            writeln!(
                out,
                "cbuffer Constants{index} : {register} {{ uint4 cb{index}Words[{}]; }};",
                words.div_ceil(4)
            )
            .ok()?;
        }
        Some(out)
    }

    fn lower(&mut self, ins: &Insn, regs: &Registers) -> Option<(String, Option<Origin>)> {
        let offset = usize::try_from(ins.aux.imm).ok()?;
        match ins.enc {
            Enc::Smem => {
                // The observed immediate form uses the NULL SOFFSET selector.
                let scalar_offset = ins.raw.get(1)? >> 25;
                let addr = regs.address(ins.src.first()?)?;
                if let (8, Origin::Descriptor(1, cb), [Opnd::VccLo]) =
                    (ins.op, addr, ins.dst.as_slice())
                {
                    if scalar_offset != 125 || !offset.is_multiple_of(4) || offset >= 65536 {
                        return None;
                    }
                    let word = offset / 4;
                    let used = self.constants.entry(cb).or_default();
                    *used = (*used).max(word + 1);
                    return Some((
                        format!("vcc = cb{cb}Words[{}][{}];", word / 4, word % 4),
                        None,
                    ));
                }
                let &Opnd::Sgpr(dst) = ins.dst.first()? else {
                    return None;
                };
                match (ins.op, addr) {
                    (0, Origin::Root)
                        if scalar_offset == 125 && [0, 4, 8, 12].contains(&offset) =>
                    {
                        Some((String::new(), Some(Origin::Table((offset / 4) as u32))))
                    }
                    (2 | 3, Origin::Table(table)) if scalar_offset == 125 => {
                        let stride = if table == 2 { 16 } else { 32 };
                        if !offset.is_multiple_of(stride) {
                            return None;
                        }
                        Some((
                            String::new(),
                            Some(Origin::Descriptor(table, (offset / stride) as u32)),
                        ))
                    }
                    (8..=12, Origin::Descriptor(1, cb))
                        if scalar_offset == 125 && offset.is_multiple_of(4) =>
                    {
                        let count = 1 << (ins.op - 8);
                        let end = offset / 4 + count;
                        if end > 16384 || usize::from(dst) + count > 128 {
                            return None;
                        }
                        let used = self.constants.entry(cb).or_default();
                        *used = (*used).max(end);
                        let mut out = String::new();
                        for lane in 0..count {
                            let word = offset / 4 + lane;
                            write!(
                                out,
                                "s[{}] = cb{cb}Words[{}][{}]; ",
                                usize::from(dst) + lane,
                                word / 4,
                                word % 4
                            )
                            .ok()?;
                        }
                        let origin = (cb == 0).then_some(Origin::ConstantWord(offset));
                        Some((out, origin))
                    }
                    (8 | 10, Origin::Descriptor(1, cb @ 0))
                        if scalar_offset < 106 && offset.is_multiple_of(4) =>
                    {
                        self.clipmap_constants();
                        let mut body =
                            format!("{{ uint w = (s[{scalar_offset}] + {offset}u) >> 2;");
                        let count = 1usize << (ins.op - 8);
                        for lane in 0..count {
                            write!(
                                body,
                                " s[{}] = cb{cb}Words[(w + {lane}u) >> 2][(w + {lane}u) & 3u];",
                                usize::from(dst) + lane
                            )
                            .ok()?;
                        }
                        body.push_str(" }");
                        Some((body, None))
                    }
                    // CardTileAllocator is a StructuredBuffer<uint>. Indirect
                    // batches use a byte offset from the light's scalar index.
                    (8, Origin::Descriptor(0, index))
                        if matches!(
                            (self.family, index),
                            (Family::CardDirectLighting, 4) | (Family::Radiosity, 13)
                        ) && offset.is_multiple_of(4) =>
                    {
                        self.srv(index, Srv::Structured(1))?;
                        let element = match scalar_offset {
                            125 => format!("{}u", offset / 4),
                            0..=105 => format!("(s[{scalar_offset}] + {offset}u) >> 2"),
                            _ => return None,
                        };
                        Some((
                            format!("s[{dst}] = structured{index}[{element}].words[0];"),
                            None,
                        ))
                    }
                    // The occlusion permutations that march hair voxels fetch
                    // 32-byte packed nodes through a scalar SRV descriptor.
                    (8 | 10, Origin::Descriptor(0, index @ 1))
                        if scalar_offset < 106 && offset.is_multiple_of(4) =>
                    {
                        self.srv(index, Srv::NodeRecords)?;
                        let count = 1usize << (ins.op - 8);
                        let mut out = format!("{{ uint w = (s[{scalar_offset}] + {offset}u) >> 2;");
                        for lane in 0..count {
                            write!(out, " s[{}] = nodes{index}[(w + {lane}u) >> 3].words[(w + {lane}u) & 7u];", usize::from(dst) + lane).ok()?;
                        }
                        out.push_str(" }");
                        Some((out, None))
                    }
                    _ => None,
                }
            }
            Enc::Mimg if matches!(ins.op, 0 | 8) && ins.aux.imm == 1 && ins.aux.imm2 == 0 => {
                let [Opnd::Vgpr(coord), base, _] = ins.src.as_slice() else {
                    return None;
                };
                let &[Opnd::Vgpr(data)] = ins.dst.as_slice() else {
                    return None;
                };
                let Origin::Descriptor(table, index) = regs.address(base)? else {
                    return None;
                };
                let xy = if ins.raw.get(1)? & (1 << 30) != 0 {
                    // GFX10 A16 places two unnormalized integer texel coordinates in one VGPR.
                    format!("uint2(v[{coord}] & 0xffffu, v[{coord}] >> 16)")
                } else {
                    format!("uint2(v[{coord}], v[{}])", u32::from(*coord) + 1)
                };
                match (ins.op, table) {
                    (8, 3) if ins.aux.mask == 15 => {
                        self.outputs.insert(index);
                        let value = self.output(index)?.from_registers(usize::from(data))?;
                        Some((format!("target{index}[{xy}] = {value};"), None))
                    }
                    (0, 0) if ins.aux.mask != 0 => {
                        self.srv(index, Srv::Texture)?;
                        let mut out = format!(
                            "{{ uint4 sampleBits = asuint(tex{index}.Load(int3({xy}, 0)));"
                        );
                        let mut dest = data;
                        for component in 0..4 {
                            if ins.aux.mask & (1 << component) != 0 {
                                write!(out, " v[{dest}] = sampleBits[{component}];").ok()?;
                                dest += 1;
                            }
                        }
                        out.push_str(" }");
                        Some((out, (ins.aux.mask == 1).then_some(Origin::ImageRed(index))))
                    }
                    _ => None,
                }
            }
            Enc::Mubuf
                if self.family == Family::General
                    && ins.op == 0
                    && ins.aux.mask == 2
                    && offset == 0 =>
            {
                let [Opnd::Vgpr(at), base, Opnd::ConstInt(0)] = ins.src.as_slice() else {
                    return None;
                };
                let &[Opnd::Vgpr(dst)] = ins.dst.as_slice() else {
                    return None;
                };
                let Origin::Descriptor(0, index @ 0) = regs.address(base)? else {
                    return None;
                };
                self.srv(index, Srv::PageIndices)?;
                Some((format!("v[{dst}] = pages{index}.Load(v[{at}]);"), None))
            }
            Enc::Mimg if ins.op == 1 && ins.aux.imm == 2 && ins.aux.mask == 1 => {
                let &[Opnd::Vgpr(dst)] = ins.dst.as_slice() else {
                    return None;
                };
                let base = ins.src.get(ins.src.len().checked_sub(2)?)?;
                let Origin::Descriptor(0, index @ 2) = regs.address(base)? else {
                    return None;
                };
                let coords = match ins.src.as_slice() {
                    [Opnd::Vgpr(first), _, _] if ins.aux.imm2 == 0 && *first < 253 => {
                        [*first, first + 1, first + 2, first + 3]
                    }
                    [
                        Opnd::Vgpr(x),
                        Opnd::Vgpr(y),
                        Opnd::Vgpr(z),
                        Opnd::Vgpr(mip),
                        _,
                        _,
                        _,
                    ] if ins.aux.imm2 == 1 => [*x, *y, *z, *mip],
                    _ => return None,
                };
                self.srv(index, Srv::Volume)?;
                let [x, y, z, mip] = coords;
                Some((
                    format!(
                        "v[{dst}] = volume{index}.Load(int4(v[{x}], v[{y}], v[{z}], v[{mip}]));"
                    ),
                    None,
                ))
            }
            // These vector-only FLAT loads fetch the AS descriptor GPUVA.
            // Its only consumer is the traversal scheduler replaced by TraceRay.
            Enc::Flat
                if ins.op == 13
                    && ins.aux.mask == 2
                    && offset.is_multiple_of(32)
                    && ins.src.get(2) == Some(&Opnd::Null)
                    && regs.address(ins.src.first()?) == Some(Origin::Table(0)) =>
            {
                let &[Opnd::Vgpr(dst)] = ins.dst.as_slice() else {
                    return None;
                };
                let index = (offset / 32) as u32;
                self.srv(index, Srv::Scene)?;
                Some((
                    format!("v[{dst}] = {}u; v[{}] = 0u;", (index + 1) << 8, dst + 1),
                    None,
                ))
            }
            Enc::Mimg | Enc::Mubuf => self.lumen_memory(ins, regs).map(|body| (body, None)),
            _ => None,
        }
    }
}

impl Bindings {
    pub(super) fn candidate(state: &State<'_>, span: &Range<usize>) -> Result<Self, String> {
        material::analyze(state, span, true)
    }
    pub(super) fn analyze(state: &State<'_>, entries: &[Range<usize>]) -> Result<Self, String> {
        if state.kind == 2 {
            let [span] = entries else {
                return Err("unexpected material continuation count".into());
            };
            return material::analyze(state, span, false);
        }
        if let [span] = entries {
            if super::lighting::matches(state) {
                let (declarations, instructions) = super::lighting::bindings(state, span)?;
                return Ok(Self {
                    declarations,
                    instructions,
                });
            }
        }
        let mut instructions = BTreeMap::new();
        let mut usage = Usage {
            family: match state.name.rsplit_once('_').map(|(name, _)| name) {
                Some("LumenReflectionHardwareRayTracingRGS") => Family::Reflection,
                Some("LumenRadiosityHardwareRayTracingRGS") => Family::Radiosity,
                Some("LumenSceneDirectLightingHardwareRayTracingRGS") => Family::CardDirectLighting,
                Some("LumenRadianceCacheHardwareRayTracingRGS") => Family::RadianceCache {
                    traces: entries.len() > 1,
                },
                Some("LumenScreenProbeGatherHardwareRayTracingRGS") => Family::ScreenProbe {
                    traces: entries.len() > 1,
                },
                _ => Family::General,
            },
            ..Usage::default()
        };
        for span in entries {
            Self::continuation(state.program, span, &mut usage, &mut instructions)?;
        }
        let declarations = usage
            .declarations(state)
            .ok_or("unresolved ray shader resources")?;
        Ok(Self {
            declarations,
            instructions,
        })
    }

    fn continuation(
        isa: &[u8],
        span: &Range<usize>,
        usage: &mut Usage,
        emitted: &mut BTreeMap<usize, String>,
    ) -> Result<(), String> {
        let mut code = Vec::new();
        let mut starts = BTreeSet::from([span.start]);
        for mut ins in instructions(isa.get(span.clone()).ok_or("invalid ray symbol range")?) {
            ins.offset += span.start;
            let next = ins.offset + ins.dwords * 4;
            if is_branch(&ins) {
                let dest = next
                    .checked_add_signed(ins.aux.imm as isize * 4)
                    .ok_or("invalid ray branch")?;
                if !span.contains(&dest) {
                    return Err(format!(
                        "ray resource branch outside {span:?} at {:#x}",
                        ins.offset
                    ));
                }
                starts.insert(dest);
                if next < span.end {
                    starts.insert(next);
                }
            }
            if ins.mnemonic == "s_setpc_b64" && next < span.end {
                starts.insert(next);
            }
            code.push(ins);
        }
        // Compute register origins at block entries. Joining a back-edge can
        // only discard a binding, so cycles converge without unrolling.
        let mut blocks = BTreeMap::new();
        for start in starts {
            let index = code
                .binary_search_by_key(&start, |i| i.offset)
                .map_err(|_| "ray branch into instruction")?;
            blocks.insert(start, index);
        }
        let mut initial = Registers([None; 384]);
        initial.0[6] = Some(Origin::Root);
        let mut inputs = BTreeMap::from([(span.start, initial)]);
        let mut pending = VecDeque::from([span.start]);
        while let Some(start) = pending.pop_front() {
            let mut regs = inputs[&start].clone();
            let mut queue = |pc, regs: &Registers| -> Result<(), String> {
                if pc == span.end {
                    return Ok(());
                }
                if !blocks.contains_key(&pc) {
                    return Err("invalid ray control-flow edge".into());
                }
                if let Some(existing) = inputs.get_mut(&pc) {
                    if !existing.merge(regs) {
                        return Ok(());
                    }
                } else {
                    inputs.insert(pc, regs.clone());
                }
                pending.push_back(pc);
                Ok(())
            };
            for ins in &code[blocks[&start]..] {
                if ins.offset != start && blocks.contains_key(&ins.offset) {
                    queue(ins.offset, &regs)?;
                    break;
                }
                if ins.mnemonic == "s_setpc_b64" {
                    break;
                }
                if is_branch(ins) {
                    let next = ins.offset + ins.dwords * 4;
                    let dest = next
                        .checked_add_signed(ins.aux.imm as isize * 4)
                        .ok_or("invalid ray branch")?;
                    queue(dest, &regs)?;
                    if ins.mnemonic != "s_branch" {
                        queue(next, &regs)?;
                    }
                    break;
                }
                // Scene and hair light-channel textures are integer views. The
                // shader masks their red component directly with the light's
                // channel bits (b0 byte 40), before testing the intersection.
                if matches!(ins.mnemonic.as_str(), "v_and_b32" | "v_and_b32_e32") {
                    if let [a, b, ..] = ins.src.as_slice() {
                        if usage.family == Family::Reflection {
                            if let (Opnd::Literal(0x7fff), Some(Origin::ImageRed(index))) =
                                (a, regs.address(b))
                            {
                                // The low 15 bits carry a positive half-float
                                // trace distance and the high bit is a flag.
                                usage.uint_textures.insert(index);
                            }
                        }
                        match (regs.address(a), regs.address(b)) {
                            (Some(Origin::ConstantWord(0x28)), Some(Origin::ImageRed(i)))
                            | (Some(Origin::ImageRed(i)), Some(Origin::ConstantWord(0x28))) => {
                                usage.uint_textures.insert(i);
                            }
                            _ => (),
                        }
                    }
                }
                // StructuredImportanceSampledRayInfosForTracing packs two
                // six-bit coordinates and a four-bit level into a UINT texel.
                if matches!(usage.family, Family::ScreenProbe { .. }) && ins.mnemonic == "v_bfe_u32"
                {
                    if let [source, Opnd::ConstInt(12), Opnd::ConstInt(4)] = ins.src.as_slice() {
                        if let Some(Origin::ImageRed(index)) = regs.address(source) {
                            usage.uint_textures.insert(index);
                        }
                    }
                }
                let addr = if matches!(ins.enc, Enc::Smem | Enc::Mimg | Enc::Flat)
                    || (ins.enc == Enc::Mubuf && !super::continuation::scratch(ins))
                {
                    let (body, addr) = usage.lower(ins, &regs).ok_or_else(|| {
                        format!(
                            "unresolved ray resource {} at {:#x}",
                            ins.mnemonic, ins.offset
                        )
                    })?;
                    emitted.insert(ins.offset, body);
                    addr
                } else if matches!(
                    ins.mnemonic.as_str(),
                    "s_mov_b32" | "v_mov_b32" | "v_mov_b32_e32"
                ) {
                    ins.src.first().and_then(|src| regs.address(src))
                } else {
                    None
                };
                if let Some(dst) = ins.dst.first().and_then(Registers::index) {
                    let count = match (ins.enc, ins.op) {
                        (Enc::Smem, 0..=4) => 1 << ins.op,
                        (Enc::Smem, 8..=12) => 1 << (ins.op - 8),
                        (Enc::Mimg, 71) => 4,
                        (Enc::Mimg, 0) => ins.aux.mask.count_ones() as usize,
                        (Enc::Flat, 13) => 2,
                        _ if ins.mnemonic.ends_with("_b64") => 2,
                        _ => 1,
                    };
                    let target = regs
                        .0
                        .get_mut(dst..dst + count)
                        .ok_or("ray register out of range")?;
                    if let Some(Origin::ConstantWord(first)) = addr {
                        for (index, word) in target.iter_mut().enumerate() {
                            *word = Some(Origin::ConstantWord(first + index * 4));
                        }
                    } else {
                        target.fill(None);
                        target[0] = addr;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn memory(&self, ins: &Insn) -> Result<String, String> {
        self.instructions
            .get(&ins.offset)
            .cloned()
            .ok_or_else(|| format!("unrecognized ray memory instruction at {:#x}", ins.offset))
    }
}

fn is_branch(ins: &Insn) -> bool {
    ins.mnemonic == "s_branch" || ins.mnemonic.starts_with("s_cbranch_")
}
