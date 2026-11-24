//! Typed texture access through a hit record's local SRV and sampler tables.
use super::{Insn, Layout, SAMPLER, SRV, Values, View};
use crate::ray::resources::lumen::{coords, save_components};
use crate::rdna2::{Opnd, instructions};

// An untyped MIMG load can address a float or integer view. These material
// shaders extract two consecutive 10-bit fields from the loaded alpha word.
pub(super) fn packed_load(ins: &Insn, following: &[u8]) -> bool {
    let [source] = ins.dst.as_slice() else {
        return false;
    };
    if ins.raw.first() != Some(&0xf0001808) {
        return false;
    }
    let mut fields = 0;
    for next in instructions(following).take(32) {
        if let [value, Opnd::ConstInt(shift @ (10 | 20)), Opnd::ConstInt(10)] = next.src.as_slice()
        {
            if value == source && next.mnemonic == "v_bfe_u32" {
                fields |= if *shift == 10 { 1 } else { 2 };
                if fields == 3 {
                    return true;
                }
            }
        }
        if next.dst.contains(source) {
            break;
        }
    }
    false
}

impl Layout {
    pub(super) fn image(&mut self, ins: &Insn, regs: &Values, dst: u8) -> Option<String> {
        if ins.raw.get(1)? & (3 << 30) != 0 {
            return None;
        }
        let base = ins.src.get(ins.src.len().checked_sub(2)?)?;
        let address = regs.get(base)?;
        let sample = match (ins.op, ins.aux.imm) {
            // The virtual texture page tables contain packed integer fields.
            (0 | 1, 1) => {
                let texture = self.resource(address, SRV, View::Page)?;
                let c = coords(ins, if ins.op == 1 { 3 } else { 2 })?;
                let lod = c.get(2).map_or("0".to_string(), |m| format!("v[{m}]"));
                format!("{texture}.Load(int3(v[{}],v[{}],{lod}))", c[0], c[1])
            }
            (34, 1) => {
                let texture = self.resource(address, SRV, View::Texture)?;
                let sampler = self.resource(regs.get(ins.src.last()?)?, SAMPLER, View::Sampler)?;
                let [dx, dy, ex, ey, x, y] = *coords(ins, 6)?.as_slice() else {
                    return None;
                };
                format!(
                    "asuint({texture}.SampleGrad({sampler},asfloat(uint2(v[{x}],v[{y}])),asfloat(uint2(v[{dx}],v[{dy}])),asfloat(uint2(v[{ex}],v[{ey}]))))"
                )
            }
            (36 | 39, dim @ (1 | 2 | 3 | 5)) => {
                let view = match dim {
                    1 => View::Texture,
                    2 => View::Volume,
                    3 => View::Cube,
                    5 => View::Array,
                    _ => unreachable!(),
                };
                let texture = self.resource(address, SRV, view)?;
                let sampler = self.resource(regs.get(ins.src.last()?)?, SAMPLER, View::Sampler)?;
                let n = if dim == 1 { 2 } else { 3 };
                let c = coords(ins, n + usize::from(ins.op == 36))?;
                let (x, y) = (c[0], c[1]);
                let xyz = match dim {
                    1 => format!("asfloat(uint2(v[{x}],v[{y}]))"),
                    3 => format!(
                        "tb_cube_direction(asfloat(v[{x}]),asfloat(v[{y}]),uint(asfloat(v[{}])))",
                        c[2]
                    ),
                    _ => format!("asfloat(uint3(v[{x}],v[{y}],v[{}]))", c[2]),
                };
                let lod = c
                    .get(n)
                    .map_or("0.0".to_string(), |r| format!("asfloat(v[{r}])"));
                format!("asuint({texture}.SampleLevel({sampler},{xyz},{lod}))")
            }
            _ => return None,
        };
        save_components(dst.into(), ins.aux.mask, &sample)
    }
}
