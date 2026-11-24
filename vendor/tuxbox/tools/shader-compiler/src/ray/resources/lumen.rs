//! Vector buffer and image operations used by Lumen's translucency-volume rays.
use super::{Family, Origin, Registers, Srv, Usage};
use crate::rdna2::{Enc, Insn, Opnd};
use std::fmt::Write;

// RDNA addresses a cube by face and projected S/T in [1,2]. DXR samples by
// direction; invert the major-axis projection once S,T and face are known.
pub(super) const CUBE_DIRECTION: &str = r#"
float3 tb_cube_direction(float s, float t, uint face) {
    float x = (s - 1.5) * 2.0, y = (t - 1.5) * 2.0;
    switch(face) {
        case 0: return float3(1, -y, -x);
        case 1: return float3(-1, -y, x);
        case 2: return float3(x, 1, y);
        case 3: return float3(x, -1, -y);
        case 4: return float3(x, -y, 1);
        default: return float3(-x, -y, -1);
    }
}
"#;

fn vgpr(op: &Opnd) -> Option<usize> {
    match op {
        Opnd::Vgpr(n) if *n < 128 => Some(usize::from(*n)),
        _ => None,
    }
}

// On GFX10 NSA the first coordinate is in the regular instruction word and
// the remaining coordinates are individual bytes in its extension words.
pub(in crate::ray) fn coords(ins: &Insn, count: usize) -> Option<Vec<usize>> {
    let first = vgpr(ins.src.first()?)?;
    match ins.aux.imm2 {
        0 if first + count <= 128 => Some((first..first + count).collect()),
        words @ 1..=3
            if count <= 1 + 4 * words as usize && ins.src.len() == 3 + 4 * words as usize =>
        {
            ins.src[..count].iter().map(vgpr).collect()
        }
        _ => None,
    }
}

pub(in crate::ray) fn save_components(dst: usize, mask: u8, value: &str) -> Option<String> {
    if mask == 0 || dst + mask.count_ones() as usize > 128 {
        return None;
    }
    let mut out = format!("{{ uint4 sampleBits = {value};");
    let mut reg = dst;
    for component in 0..4 {
        if mask & (1 << component) != 0 {
            write!(out, " v[{reg}] = sampleBits[{component}];").ok()?;
            reg += 1;
        }
    }
    out.push_str(" }");
    Some(out)
}

impl Usage {
    pub(super) fn clipmap_constants(&mut self) {
        // The three clipmap arrays in b0 have six float4s each, starting at
        // words 124, 148 and 172. Scalar byte addressing and vector IDXEN
        // select elements in these same arrays.
        let words = self.constants.entry(0).or_default();
        *words = (*words).max(if self.family == Family::Reflection {
            65 * 4
        } else {
            49 * 4
        });
    }

    pub(super) fn lumen_memory(&mut self, ins: &Insn, regs: &Registers) -> Option<String> {
        if ins.enc == Enc::Mubuf {
            return self.vector_buffer(ins, regs);
        }
        let dst = vgpr(ins.dst.first()?)?;
        let image = regs.address(ins.src.get(ins.src.len().checked_sub(2)?)?)?;
        let dim = ins.aux.imm;
        match (ins.op, dim, image) {
            (0, 2, Origin::Descriptor(0, index)) if ins.aux.mask == 1 => {
                let c = coords(ins, 3)?;
                self.srv(index, Srv::Volume)?;
                Some(format!(
                    "v[{dst}] = volume{index}.Load(int4(v[{}], v[{}], v[{}], 0));",
                    c[0], c[1], c[2]
                ))
            }
            (8, 2, Origin::Descriptor(3, index)) if ins.aux.mask == 15 && dst + 4 <= 128 => {
                let c = coords(ins, 3)?;
                self.volume_outputs.insert(index);
                let value = match index {
                    1 => format!("asfloat(uint3(v[{dst}],v[{}],v[{}]))", dst + 1, dst + 2),
                    2 => format!("asfloat(v[{dst}])"),
                    _ => return None,
                };
                Some(format!(
                    "target{index}[uint3(v[{}], v[{}], v[{}])] = {value};",
                    c[0], c[1], c[2]
                ))
            }
            (71, 1, Origin::Descriptor(0, index)) if self.family == Family::Radiosity => {
                let Origin::Descriptor(2, sampler) = regs.address(ins.src.last()?)? else {
                    return None;
                };
                let channel = match ins.aux.mask {
                    1 => "Red",
                    2 => "Green",
                    _ => return None,
                };
                if ins.raw.get(1)? & (1 << 30) != 0 {
                    return None;
                }
                let c = coords(ins, 2)?;
                self.srv(index, Srv::Texture)?;
                self.samplers.insert(sampler);
                save_components(
                    dst,
                    15,
                    &format!(
                        "asuint(tex{index}.Gather{channel}(sampler{sampler}, asfloat(uint2(v[{}],v[{}]))))",
                        c[0], c[1]
                    ),
                )
            }
            (36 | 39, 1 | 3, Origin::Descriptor(0, index)) => {
                let Origin::Descriptor(2, samp) = regs.address(ins.src.last()?)? else {
                    return None;
                };
                let n = if dim == 3 { 3 } else { 2 };
                let c = coords(ins, n + usize::from(ins.op == 36))?;
                let (name, coordinate) = if dim == 3 {
                    self.srv(index, Srv::Cube)?;
                    (
                        format!("sky{index}"),
                        format!(
                            "tb_cube_direction(asfloat(v[{}]), asfloat(v[{}]), uint(asfloat(v[{}])))",
                            c[0], c[1], c[2]
                        ),
                    )
                } else {
                    self.srv(index, Srv::Texture)?;
                    (
                        format!("tex{index}"),
                        format!("asfloat(uint2(v[{}],v[{}]))", c[0], c[1]),
                    )
                };
                let level = if ins.op == 36 {
                    format!("asfloat(v[{}])", c[n])
                } else {
                    "0.0".into()
                };
                self.samplers.insert(samp);
                save_components(
                    dst,
                    ins.aux.mask,
                    &format!("asuint({name}.SampleLevel(sampler{samp}, {coordinate}, {level}))"),
                )
            }
            _ => None,
        }
    }

    fn record_word(
        &mut self,
        ins: &Insn,
        index: u32,
        words: usize,
        addr: usize,
        data: usize,
    ) -> Option<String> {
        let offset = usize::try_from(ins.aux.imm).ok()?;
        if !offset.is_multiple_of(4) || offset / 4 >= words {
            return None;
        }
        self.record_outputs.insert(index, words);
        let field = format!("records{index}[v[{addr}]].words[{}]", offset / 4);
        match ins.op {
            12 => Some(format!("v[{data}] = {field};")),
            28 => Some(format!("{field} = v[{data}];")),
            _ => None,
        }
    }

    fn vector_buffer(&mut self, ins: &Insn, regs: &Registers) -> Option<String> {
        let [addr, base, Opnd::ConstInt(0)] = ins.src.as_slice() else {
            return None;
        };
        let at = vgpr(addr)?;
        let dst = vgpr(ins.dst.first()?)?;
        let Origin::Descriptor(table, index) = regs.address(base)? else {
            return None;
        };
        match (self.family, table, index, ins.op, ins.aux.mask, ins.aux.imm) {
            // The shadow atlas stores 32 independent bit tests in each DWORD.
            (Family::CardDirectLighting, 3, 0, 58, 2, 0) => {
                self.buffer_outputs.insert(index);
                return Some(format!(
                    "InterlockedOr(writable{index}[v[{at}]], v[{dst}]);"
                ));
            }
            (Family::CardDirectLighting, 3, 0, 12, 2, 0) => {
                self.buffer_outputs.insert(index);
                return Some(format!("v[{dst}] = writable{index}[v[{at}]];"));
            }
            // Native GLC requests the pre-add value. The following x2 store
            // uses that returned index to append a surface-cache feedback pair.
            (Family::Radiosity | Family::Reflection, 3, 1, 50, 4, 0) => {
                self.buffer_outputs.insert(index);
                return Some(format!(
                    "{{ uint previous; InterlockedAdd(writable{index}[0], v[{dst}], previous); v[{dst}] = previous; }}"
                ));
            }
            (Family::Radiosity | Family::Reflection, 3, 2, 29, 2, 0) if dst + 1 < 128 => {
                self.record_outputs.insert(index, 2);
                return Some(format!(
                    "records{index}[v[{at}]].words[0] = v[{dst}]; records{index}[v[{at}]].words[1] = v[{}];",
                    dst + 1
                ));
            }
            _ => {}
        }
        if self.family == Family::Reflection && table == 3 && index == 3 && ins.aux.mask == 2 {
            return self.record_word(ins, index, 5, at, dst);
        }
        if let Family::RadianceCache { traces: tracing } = self.family {
            if table == 3 && (index != 0 || !tracing) && ins.aux.mask == 2 {
                // The title allocates TraceTileResult with stride 8 and
                // FTraceDataPacked with stride 20. Packing-only permutations
                // omit the page-use UAV and shift these two bindings down.
                let words = match index.checked_sub(u32::from(tracing))? {
                    0 => 2,
                    1 => 5,
                    _ => return None,
                };
                return self.record_word(ins, index, words, at, dst);
            }
            if table == 0 && ins.aux.imm == 0 && ins.op < 4 {
                let element = match ins.aux.mask {
                    0 => "0".into(),
                    2 => format!("v[{at}]"),
                    _ => return None,
                };
                return match (ins.op, tracing, index) {
                    (0, true, 12) | (0, false, 1 | 2 | 4) => {
                        self.srv(index, Srv::PageIndices)?;
                        Some(format!("v[{dst}] = pages{index}.Load({element});"))
                    }
                    (1, true, 7) | (1, false, 3) => {
                        self.srv(index, Srv::BufferUint2)?;
                        save_components(
                            dst,
                            3,
                            &format!("uint4(pairs{index}.Load({element}), 0, 0)"),
                        )
                    }
                    (3, true, 6) | (3, false, 2) => {
                        self.srv(index, Srv::BufferFloat4)?;
                        save_components(dst, 15, &format!("asuint(values{index}.Load({element}))"))
                    }
                    _ => None,
                };
            }
        }
        if let Family::ScreenProbe { traces } = self.family {
            if table == 3 && index == 2 + u32::from(traces) && ins.aux.mask == 2 {
                return self.record_word(ins, index, 5, at, dst);
            }
            if table == 0 && ins.aux.imm == 0 {
                match (traces, index, ins.op, ins.aux.mask) {
                    (false, 5 | 6, 0, 0) | (true, 14..=17, 0, 0) => {
                        self.srv(index, Srv::PageIndices)?;
                        return Some(format!("v[{dst}] = pages{index}.Load(0);"));
                    }
                    // AdaptiveScreenProbeData is an R32_UINT view. Each
                    // element supplies the two packed 16-bit screen coordinates.
                    (false, 3, 12, 2) | (true, 12 | 14, 12, 2) => {
                        self.srv(index, Srv::PageIndices)?;
                        return Some(format!("v[{dst}] = pages{index}.Load(v[{at}]);"));
                    }
                    _ => {}
                }
            }
        }
        if (ins.op, table, index, ins.aux.mask, ins.aux.imm) == (28, 3, 0, 2, 0) {
            self.buffer_outputs.insert(index);
            return Some(format!("writable{index}[v[{at}]] = v[{dst}];"));
        }
        if self.family == Family::Reflection && table == 0 && ins.aux.imm == 0 {
            match (ins.op, ins.aux.mask, index) {
                (0, 2, 2) | (0, 0, 8 | 11 | 13 | 16) => {
                    let element = if ins.aux.mask == 0 {
                        "0".into()
                    } else {
                        format!("v[{at}]")
                    };
                    self.srv(index, Srv::PageIndices)?;
                    return Some(format!("v[{dst}] = pages{index}.Load({element});"));
                }
                (1, 2, 9 | 12 | 14 | 17) if dst + 1 < 128 => {
                    self.srv(index, Srv::BufferUint2)?;
                    return Some(format!(
                        "{{ uint2 pair=pairs{index}.Load(v[{at}]); v[{dst}]=pair.x; v[{}]=pair.y; }}",
                        dst + 1
                    ));
                }
                _ => {}
            }
        }
        let n = match ins.op {
            12 => 1,
            13 => 2,
            14 => 4,
            15 => 3,
            _ => return None,
        };
        let offset = usize::try_from(ins.aux.imm).ok()?;
        if !offset.is_multiple_of(4) || dst + n > 128 {
            return None;
        }
        let mut out = format!("{{ uint at = v[{at}];");
        // Both the near and far reflection buffers allocate 20-byte
        // FTraceDataPacked records. These programs read words 0 and 2.
        if self.family == Family::Reflection
            && (table, index, ins.op, ins.aux.mask) == (3, 0, 12, 2)
            && matches!(offset, 0 | 8)
        {
            return self.record_word(ins, index, 5, at, dst);
        }

        match (table, index, ins.aux.mask) {
            (0, _, 1)
                if matches!((self.family, index), (Family::Radiosity, 3 | 4))
                    || (self.family != Family::Radiosity && matches!(index, 2 | 3)) =>
            {
                self.srv(index, Srv::Raw)?;
                for lane in 0..n {
                    write!(
                        out,
                        " v[{}] = raw{index}.Load(at + {}u);",
                        dst + lane,
                        offset + lane * 4
                    )
                    .ok()?;
                }
            }
            (1, 0, 2) if offset == 0 => {
                // IDXEN on the CBV selects a 16-byte cbuffer element. The
                // validated dynamic clipmap index is supplied by the title.
                self.clipmap_constants();
                for lane in 0..n {
                    write!(out, " v[{}] = cb{index}Words[at][{lane}];", dst + lane).ok()?;
                }
            }
            (0, _, 2) => {
                // CardData, MeshCardsData, GPUScenePrimitiveSceneData and ProbeWorldOffset are StructuredBuffer<float4> in the title.
                let words = match (self.family, index) {
                    (Family::RadianceCache { traces: true }, 8) => 4,
                    (Family::Radiosity, 2 | 9) => 4,
                    (Family::CardDirectLighting, 5 | 6) | (Family::Radiosity, 14) => 1,
                    (Family::ScreenProbe { traces: true }, 5..=7) => 4,
                    // CompactedTraceTexelData packs the probe ID and tile
                    // coordinates in the first DWORD of a uint2 record.
                    (Family::ScreenProbe { traces: false }, 4 | 5)
                    | (Family::ScreenProbe { traces: true }, 13..=16)
                        if n == 2 && offset == 0 =>
                    {
                        2
                    }
                    (Family::Reflection, 0 | 1 | 3 | 6 | 10 | 15) => 4,
                    (family, 0 | 1 | 5 | 11) if family != Family::Reflection => 4,
                    _ => return None,
                };
                if offset / 4 + n > words as usize {
                    return None;
                }
                self.srv(index, Srv::Structured(words))?;
                for lane in 0..n {
                    write!(
                        out,
                        " v[{}] = structured{index}[at].words[{}];",
                        dst + lane,
                        offset / 4 + lane
                    )
                    .ok()?;
                }
            }
            _ => return None,
        }
        out.push_str(" }");
        Some(out)
    }
}
