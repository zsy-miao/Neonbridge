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
#[path = "tests/constant_pixel.rs"]
mod tests;
