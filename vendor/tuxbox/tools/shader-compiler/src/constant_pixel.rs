//! Pixel shaders that write constant colors, forward rasterized depth or
//! discard every sample. AMD's RDNA 2 ISA manual describes compressed exports
//! and the EXEC valid mask in section 11.1.

use crate::rdna2::{instructions, Enc, Opnd};
use crate::{ShaderExtract, Stage};
use std::fmt::Write;

#[derive(Clone, Copy)]
enum Value {
    Bits(u32),
    Packed([u32; 2]),
}

fn value(op: Opnd, registers: &[Option<Value>; 256]) -> Option<Value> {
    match op {
        Opnd::ConstInt(n) => Some(Value::Bits(n as u32)),
        Opnd::ConstF32(f) => Some(Value::Bits(f.to_bits())),
        Opnd::Literal(bits) => Some(Value::Bits(bits)),
        Opnd::Vgpr(r) => registers[r as usize],
        _ => None,
    }
}

// For normal FP16 numbers exactly representable in FP32, packing and unpacking
// returns the original FP32 bits under every rounding and denormal mode.
fn exact_half(bits: u32) -> bool {
    let magnitude = bits & 0x7fff_ffff;
    magnitude == 0 || (113..=142).contains(&(magnitude >> 23)) && magnitude & 0x1fff == 0
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mask {
    Entry,
    Empty,
}

struct Wave {
    exec: Mask,
    pairs: [Option<Mask>; 53],
    scc_is_zero: bool,
}

impl Wave {
    fn read(&self, src: Opnd) -> Option<Mask> {
        match src {
            Opnd::ExecLo => Some(self.exec),
            Opnd::Sgpr(r) if r % 2 == 0 => *self.pairs.get(usize::from(r / 2))?,
            Opnd::ConstInt(0) => Some(Mask::Empty),
            _ => None,
        }
    }

    fn write(&mut self, dst: Opnd, mask: Mask) -> Option<()> {
        match dst {
            Opnd::ExecLo => self.exec = mask,
            Opnd::Sgpr(r) if r % 2 == 0 => *self.pairs.get_mut(usize::from(r / 2))? = Some(mask),
            _ => return None,
        }
        Some(())
    }
}

#[derive(Debug, PartialEq)]
enum PixelOutput {
    Discard,
    Export(Vec<[u32; 4]>),
}

fn exports(isa: &[u8], widths: &[u8]) -> Option<PixelOutput> {
    if !isa.len().is_multiple_of(4) {
        return None;
    }
    let mut registers = [None; 256];
    let mut colors = Vec::with_capacity(widths.len());
    let mut done = false;
    let mut wave = Wave {
        exec: Mask::Entry,
        pairs: [None; 53],
        scc_is_zero: false,
    };
    let mut stream = instructions(isa).peekable();
    let mut discard = false;
    while let Some(ins) = stream.next() {
        if !ins.is_complete() {
            return None;
        }
        if ins.raw.as_slice() == [0xbf81_0000] {
            return (done && stream.next().is_none()).then_some(if discard {
                PixelOutput::Discard
            } else {
                PixelOutput::Export(colors)
            });
        }
        if done {
            return None;
        }
        match (ins.enc, ins.op) {
            (Enc::Sopp, 0 | 32) | (Enc::Vop3, 0x180) => {} // NOPs and instruction prefetch.
            (Enc::Vop1, 1) if ins.aux.sdwa.is_none() && wave.exec == Mask::Entry => {
                let ([Opnd::Vgpr(dst)], &[src]) = (&ins.dst[..], &ins.src[..]) else {
                    return None;
                };
                registers[*dst as usize] = Some(value(src, &registers)?);
            }
            (Enc::Vop2, 47) if ins.aux.sdwa.is_none() && wave.exec == Mask::Entry => {
                let ([Opnd::Vgpr(dst)], &[lo, hi]) = (&ins.dst[..], &ins.src[..]) else {
                    return None;
                };
                let (Value::Bits(lo), Value::Bits(hi)) =
                    (value(lo, &registers)?, value(hi, &registers)?)
                else {
                    return None;
                };
                if !exact_half(lo) || !exact_half(hi) {
                    return None;
                }
                registers[*dst as usize] = Some(Value::Packed([lo, hi]));
            }
            (Enc::Sop1, _) if ins.mnemonic == "s_mov_b64" => {
                let (&[dst], &[src]) = (&ins.dst[..], &ins.src[..]) else {
                    return None;
                };
                wave.write(dst, wave.read(src)?)?;
            }
            (Enc::Sop2, _) if matches!(ins.mnemonic.as_str(), "s_andn2_b64" | "s_and_b64") => {
                let (&[dst], &[a, b]) = (&ins.dst[..], &ins.src[..]) else {
                    return None;
                };
                let (a, b) = (wave.read(a)?, wave.read(b)?);
                let mask = match (a, b, ins.mnemonic.as_str()) {
                    (Mask::Entry, Mask::Empty, "s_andn2_b64")
                    | (Mask::Entry, Mask::Entry, "s_and_b64") => Mask::Entry,
                    _ => Mask::Empty,
                };
                wave.write(dst, mask)?;
                // The entry mask may be empty. Only a known zero determines SCC.
                wave.scc_is_zero = mask == Mask::Empty;
            }
            (Enc::Sopp, 4) => {
                // S_CBRANCH_SCC0 is relative to the following instruction.
                // Skip only if the direction is proven and the destination
                // is a later instruction boundary within this program.
                if !wave.scc_is_zero {
                    return None;
                }
                let target =
                    (ins.offset + 4).checked_add(usize::try_from(ins.aux.imm).ok()? * 4)?;
                while stream.peek()?.offset < target {
                    if !stream.next()?.is_complete() {
                        return None;
                    }
                }
                if stream.peek()?.offset != target {
                    return None;
                }
            }
            (Enc::Exp, _) if ins.raw[0] == 0xf800_1890 && colors.is_empty() => {
                // With VM=1, EXEC is the valid pixel mask. Full coverage here
                // preserves rasterized depth, an empty mask suppresses it.
                discard = wave.exec == Mask::Empty;
                if !discard && !widths.is_empty() {
                    return None;
                }
                done = true;
            }
            (Enc::Exp, _) => {
                if wave.exec != Mask::Entry {
                    return None;
                }
                let target = colors.len();
                let &width = widths.get(target)?;
                let enabled = if width <= 2 { 3 } else { 15 };
                let last = target + 1 == widths.len();
                let flags = if last { 0x1c00 } else { 0x0400 };
                // Compressed EXP packs R/G and B/A into register pairs. DONE
                // and VM must occur on the last target, after all earlier MRTs.
                if ins.raw[0] != (0xf800_0000 | flags | (target as u32) << 4 | enabled) {
                    return None;
                }
                let &[rg, ba, _, _] = ins.src.as_slice() else {
                    return None;
                };
                let Value::Packed([r, g]) = value(rg, &registers)? else {
                    return None;
                };
                let [b, a] = if width > 2 {
                    let Value::Packed(channels) = value(ba, &registers)? else {
                        return None;
                    };
                    channels
                } else {
                    [0, 0]
                };
                colors.push([r, g, b, a]);
                done = last;
            }
            _ => return None,
        }
    }
    None
}

fn float_type(width: u8) -> &'static str {
    match width {
        1 => "float",
        2 => "float2",
        3 => "float3",
        4 => "float4",
        _ => unreachable!("signature width was checked"),
    }
}

fn color_literal(color: &[u32; 4], width: u8) -> String {
    let [r, g, b, a] = color;
    match width {
        1 => format!("asfloat(0x{r:08x}u)"),
        2 => format!("asfloat(uint2(0x{r:08x}u, 0x{g:08x}u))"),
        3 => format!("asfloat(uint3(0x{r:08x}u, 0x{g:08x}u, 0x{b:08x}u))"),
        4 => format!("asfloat(uint4(0x{r:08x}u, 0x{g:08x}u, 0x{b:08x}u, 0x{a:08x}u))"),
        _ => unreachable!("signature width was checked"),
    }
}

/// Emit an entry point once both the signature and native dataflow establish
/// that no incoming varyings participate in a value, kill, or side effect.
pub(super) fn hlsl(shader: &ShaderExtract) -> Option<String> {
    if shader.stage != Stage::Pixel
        // In particular, forced early depth/stencil changes the effect of
        // a discard on depth writes. Translate only the verified defaults.
        || shader.required_features != Some(0)
        || !shader.has_input_sig
        || !shader.has_output_sig
        || !shader.has_psv0
        || shader.input_sig.iter().any(|input| input.rw_mask != 0)
        || !shader.resources.is_empty()
        || shader.output_sig.len() > 8
    {
        return None;
    }
    let mut widths = [0; 8];
    for (i, out) in shader.output_sig.iter().enumerate() {
        if !out.name.eq_ignore_ascii_case("SV_Target")
            || out.index != i as u32
            || out.register != i as u32
            || out.sysval != 64
            || out.comp_type != 3
            || !matches!(out.mask, 1 | 3 | 7 | 15)
            || out.rw_mask != (15 ^ out.mask)
        {
            return None;
        }
        widths[i] = out.mask.count_ones() as u8;
    }
    let widths = &widths[..shader.output_sig.len()];
    let (kill, colors) = match exports(&shader.isa, widths)? {
        PixelOutput::Discard => ("discard; ", vec![[0; 4]; widths.len()]),
        PixelOutput::Export(colors) => ("", colors),
    };
    match colors.as_slice() {
        [] => Some(format!("void main() {{{kill}}}\n")),
        [color] => Some(format!(
            "{} main() : SV_Target0 {{\n    {kill}return {};\n}}\n",
            float_type(widths[0]),
            color_literal(color, widths[0])
        )),
        _ => {
            let mut src = String::from("struct Targets {\n");
            for (i, &width) in widths.iter().enumerate() {
                writeln!(src, "    {} color{i} : SV_Target{i};", float_type(width)).ok()?;
            }
            writeln!(src, "}};\nTargets main() {{\n    {kill}Targets outv;").ok()?;
            for (i, (color, &width)) in colors.iter().zip(widths).enumerate() {
                writeln!(src, "    outv.color{i} = {};", color_literal(color, width)).ok()?;
            }
            src.push_str("    return outv;\n}\n");
            Some(src)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // v_mov_b32 v2, 1.0; v_mov_b32 v3, 0; pack (v2,v3) and (v3,v2);
    // exp mrt0 v0,v1 compr done vm; s_endpgm.
    const RED: [u32; 7] = [
        0x7e04_02f2,
        0x7e06_0280,
        0x5e00_0702,
        0x5e02_0503,
        0xf800_1c0f,
        0x0000_0100,
        0xbf81_0000,
    ];

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn channels(isa: &[u8]) -> Option<[u32; 4]> {
        match exports(isa, &[4])? {
            PixelOutput::Export(colors) => colors.into_iter().next(),
            PixelOutput::Discard => None,
        }
    }

    #[test]
    fn packs_the_low_and_high_channels_in_export_order() {
        assert_eq!(
            channels(&bytes(&RED)),
            Some([1.0f32.to_bits(), 0, 0, 1.0f32.to_bits()])
        );
        assert!(exact_half(65504.0f32.to_bits()));
        assert!(!exact_half(65536.0f32.to_bits()));
        assert!(!exact_half(f32::NAN.to_bits()));
        assert!(!exact_half(1.0001f32.to_bits()));
    }

    #[test]
    fn depth_only_requires_a_valid_null_export_and_no_mask_changes() {
        assert_eq!(
            exports(&bytes(&[0xf800_1890, 0, 0xbf81_0000]), &[]),
            Some(PixelOutput::Export(vec![]))
        );
        for word in [0xf800_1090, 0xf800_0890, 0xf800_1891, 0xf800_1800] {
            assert!(exports(&bytes(&[word, 0, 0xbf81_0000]), &[]).is_none());
        }
        assert_eq!(
            exports(&bytes(&[0xbefe_0480, 0xf800_1890, 0, 0xbf81_0000]), &[]),
            Some(PixelOutput::Discard)
        );
        assert!(exports(&bytes(&[0xbf81_0000]), &[]).is_none());
    }

    #[test]
    fn follows_a_proven_lane_mask_to_a_discard() {
        // Save EXEC, subtract it from itself, branch past an unreachable
        // MRT, and deliver the empty mask to the rasterizer.
        let path = [
            0xbe84_047e,
            0x8a86_7e04,
            0xbf84_0003,
            0xf800_1c0f,
            0,
            0xbf81_0000,
            0xbefe_0406,
            0xf800_1890,
            0,
            0xbf81_0000,
        ];
        assert_eq!(exports(&bytes(&path), &[4]), Some(PixelOutput::Discard));
        for (i, word) in [
            (0, 0xbe84_0406),
            (1, 0x8786_7e04),
            (2, 0xbf84_ffff),
            (2, 0xbf84_0001),
            (6, 0xbefe_0404),
        ] {
            let mut invalid = path;
            invalid[i] = word;
            assert!(exports(&bytes(&invalid), &[4]).is_none());
        }
    }

    #[test]
    fn validates_mrt_order_final_flags_and_declared_width() {
        let mut shader = RED[..4].to_vec();
        shader.extend([0xf800_0403, 0, 0xf800_1c1f, 0x0000_0001, 0xbf81_0000]);
        let Some(PixelOutput::Export(colors)) = exports(&bytes(&shader), &[2, 4]) else {
            panic!("missing MRT outputs")
        };
        let one = 1.0f32.to_bits();
        assert_eq!(colors, [[one, 0, 0, 0], [0, one, one, 0]]);
        for widths in [&[4, 4][..], &[1], &[], &[2, 4, 4]] {
            assert!(exports(&bytes(&shader), widths).is_none());
        }
        for (i, word) in [(4, 0xf800_1c03), (6, 0xf800_041f), (6, 0xf800_1c0f)] {
            let mut bad = shader.clone();
            bad[i] = word;
            assert!(exports(&bytes(&bad), &[2, 4]).is_none());
        }
    }

    #[test]
    fn rejects_live_inputs_control_flow_and_other_export_modes() {
        for (i, word) in [
            (0, 0x7e04_0202),
            (1, 0xbf82_0000),
            (2, 0x5e00_0704),
            (3, 0xbe_fe_03_80),
            (4, 0xf800_180f),
            (4, 0xf800_1c1f),
            (4, 0xf800_1c03),
            (4, 0xf800_0c0f),
            (4, 0xf800_140f),
            (6, 0xbf80_0000),
        ] {
            let mut bad = RED;
            bad[i] = word;
            assert_eq!(channels(&bytes(&bad)), None, "word {i} = {word:#x}");
        }
        let mut extra = bytes(&RED);
        extra.extend(0xbf81_0000u32.to_le_bytes());
        assert_eq!(channels(&extra), None);
        assert_eq!(channels(&bytes(&RED)[..25]), None);
    }
}
