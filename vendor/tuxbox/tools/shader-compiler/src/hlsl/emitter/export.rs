//! Position, parameter and colour exports.
use super::*;

impl<'a> Emitter<'a> {
    pub(in crate::hlsl) fn emit_exp(&mut self, ins: &Insn, is_ps: bool) -> Result<(), String> {
        let target = ins.aux.imm as u32;
        let en = ins.aux.mask;
        let compr = ins.aux.imm2 & 1 != 0;
        let mut comp = vec![];
        if compr {
            for i in 0..4usize {
                let src = &ins.src[i / 2];
                let on = (en >> (i / 2 * 2)) & 1 != 0 || (en >> i) & 1 != 0;
                comp.push(if on {
                    let word = format!("(({} >> {}) & 0xffffu)", r(src)?, (i % 2) * 16);
                    if self.cur_idx.checked_sub(1).and_then(|idx| self.insns.get(idx))
                        .is_some_and(|prior| prior.mnemonic == "v_cvt_pknorm_u16_f32" && prior.dst.first() == Some(src))
                    {
                        format!("((float){word} / 65535.0)")
                    } else {
                        format!("f16tof32({word})")
                    }
                } else {
                    "0.0".into()
                });
            }
        } else {
            for i in 0..4usize {
                comp.push(if (en >> i) & 1 != 0 { f(&ins.src[i])? } else { "0.0".into() });
            }
        }
        let v4 = format!("float4({})", comp.join(", "));
        // The exported registers as raw 32-bit values, when they are available unpacked.
        let raw: Option<Vec<String>> = if compr {
            self.epilog_exp.get(&self.cur_idx).copied().map(|(a, b)| {
                (0..4usize)
                    .map(|i| {
                        let id = if i < 2 { a } else { b };
                        let on = (en >> (i / 2 * 2)) & 1 != 0 || (en >> i) & 1 != 0;
                        if on && id != usize::MAX { format!("tb_pk{id}{}", if i % 2 == 0 { "a" } else { "b" }) } else { "0u".to_string() }
                    })
                    .collect()
            })
        } else {
            let mut v = vec![];
            for i in 0..4usize {
                v.push(if (en >> i) & 1 != 0 { r(&ins.src[i])? } else { "0u".into() });
            }
            Some(v)
        };
        if is_ps && ins.aux.imm2 & 4 != 0 {
            // Valid-mask export: lanes that are inactive here are killed (pixel discard).
            self.line("if ((exec_lo & 1u) == 0u) discard;");
        }
        if is_ps && target == 9 {
            return Ok(()); // null export (no colour written)
        }
        if is_ps && target == 8 {
            // MRTZ exports depth in X and the per-fragment stencil reference in Y.
            // Keep their independent channel enables and their respective float/integer types.
            if compr || en == 0 || en & !3 != 0 {
                return Err(format!("unsupported MRTZ export format (mask={en}, compressed={compr})"));
            }
            if en & 1 != 0 {
                if !self.ex.output_sig.iter().any(|e| matches!(e.sysval, 65 | 67 | 68)) {
                    return Err("depth export without an SV_Depth output".into());
                }
                self.line(&format!("if (exec_lo & 1u) o.od = {};", f(&ins.src[0])?));
            }
            if en & 2 != 0 {
                let slot = self.ex.output_sig.iter().position(|e| e.sysval == 69 && e.comp_type == 1 && e.mask == 1)
                    .ok_or("stencil export without a scalar uint SV_StencilRef output")?;
                self.line(&format!("if (exec_lo & 1u) o.o{slot} = {};", r(&ins.src[1])?));
            }
            return Ok(());
        }
        if !is_ps && (32..64).contains(&target) && (target - 32) as usize >= self.out_params.len() {
            // Hardware needs at least one parameter export; with no matching output
            // element nothing downstream can read it.
            self.line("// (parameter export with no output-signature element: ignored)");
            return Ok(());
        }
        // (element index, swizzle of the exported register that feeds it)
        let osig = &self.ex.output_sig;
        let mut assigns: Vec<(usize, String)> = Vec::new();
        if is_ps {
            if target > 7 {
                return Err(format!("PS export target {target} unsupported"));
            }
            // MRT export k feeds the k-th declared SV_Target (outputs are compacted:
            // a shader writing SV_Target0 and SV_Target4 exports MRT 0 and MRT 1).
            let ei = osig
                .iter()
                .enumerate()
                .filter(|(_, e)| e.sysval == 64)
                .nth(target as usize)
                .map(|(i, _)| i)
                .ok_or_else(|| format!("MRT export {target} has no SV_Target output element"))?;
            assigns.push((ei, "xyzw"[..sig_type(&osig[ei]).1].to_string()));
        } else if (12..=15).contains(&target) {
            // Clip distances follow POS0 when there is no miscellaneous position
            // export (point size, layer, viewport, etc.). The hardware mask is
            // in plane order, independent of the D3D output register number.
            if target == 13 && self.ex.stage == Stage::Vertex && !compr
                && osig.iter().any(|e| e.sysval == 1)
                && !osig.iter().any(|e| e.name.eq_ignore_ascii_case("PSIZE"))
            {
                let mut extra = osig.iter().enumerate().filter(|(_, e)| !matches!(e.sysval, 0 | 1));
                if let Some((ei, clip)) = extra.next() {
                    // The export enables all four channels; only the declared planes are used.
                    if matches!(clip.sysval, 2 | 3) && clip.index == 0 && clip.comp_type == 3
                        && en & clip.mask == clip.mask && matches!(clip.mask, 1 | 3 | 7 | 15) && extra.next().is_none()
                    {
                        let swizzle = &"xyzw"[..clip.mask.count_ones() as usize];
                        self.line(&format!("if (exec_lo & 1u) o.o{ei} = {v4}.{swizzle};"));
                        return Ok(());
                    }
                }
            }
            // RDNA2's miscellaneous position export packs the render-target
            // array index into POS1.z[10:0]. Output-signature packing describes
            // D3D registers, not the component used by the hardware export.
            if target == 13 && self.ex.stage == Stage::Vertex && !compr && en & 4 != 0
                && osig.iter().all(|e| matches!(e.sysval, 0 | 1 | 4))
            {
                if let Some(ei) = osig.iter().position(|e| e.sysval == 4 && e.comp_type == 1) {
                    self.line(&format!("if (exec_lo & 1u) o.o{ei} = {} & 0x7ffu;", r(&ins.src[2])?));
                    return Ok(());
                }
            }
            if target != 12 {
                return Err(format!("position export {target} unsupported"));
            }
            match osig.iter().position(|e| e.sysval == 1) {
                Some(ei) => assigns.push((ei, "xyzw".to_string())),
                None => {
                    // A vertex shader feeding a geometry shader (entry "MainForGS") still
                    // carries a position export, but its D3D output signature has no
                    // SV_Position: the next stage consumes only the parameter exports.
                    self.line("// (position export with no SV_Position output element: ignored)");
                    return Ok(());
                }
            }
        } else if (32..64).contains(&target) {
            let param = self.out_params[(target - 32) as usize].clone();
            let Param::Full(reg) = param else {
                let raw = raw.filter(|_| !compr).ok_or("half-float parameter fed by a packed export")?;
                let mut lines = vec![];
                for (ei, e) in osig.iter().enumerate().filter(|(_, e)| e.sysval == 0 && param.registers().contains(&e.register)) {
                    let mut k = 0;
                    for c in (0..4usize).filter(|c| e.mask >> c & 1 != 0) {
                        let (chan, high) = param.channel(e.register, c).ok_or("unmapped half-float parameter")?;
                        let shift = if high == Some(true) { 16 } else { 0 };
                        let swizzle = if e.mask.count_ones() == 1 { String::new() } else { format!(".{}", XYZW[k]) };
                        lines.push(format!("if (exec_lo & 1u) o.o{ei}{swizzle} = f16tof32(({} >> {shift}) & 0xffffu);", raw[chan]));
                        k += 1;
                    }
                }
                for l in lines { self.line(&l); }
                return Ok(());
            };
            for (ei, e) in osig.iter().enumerate() {
                if e.sysval == 0 && e.register == reg {
                    let sw: String = (0..4).filter(|c| (e.mask >> c) & 1 != 0).map(|c| XYZW[c]).collect();
                    assigns.push((ei, sw));
                }
            }
        } else {
            return Err(format!("export target {target} unsupported"));
        }
        let mut lines = vec![];
        for (ei, sw) in assigns {
            let (ty, _) = sig_type(&osig[ei]);
            // Integer elements carry the register bits themselves; converting them through
            // float (or half) would destroy them.
            let val = match (osig[ei].comp_type, &raw) {
                (1, Some(rc)) => format!("uint4({})", rc.join(", ")),
                (2, Some(rc)) => format!("int4(uint4({}))", rc.join(", ")),
                (1 | 2, None) => return Err("integer output element fed by a packed export without a recognised pack epilogue".into()),
                (_, Some(rc)) if compr => format!("float4({})", rc.iter().map(|x| format!("asfloat({x})")).collect::<Vec<_>>().join(", ")),
                _ => v4.clone(),
            };
            lines.push(format!("if (exec_lo & 1u) o.o{ei} = ({ty})({val}.{sw});"));
        }
        for l in lines {
            self.line(&l);
        }
        Ok(())
    }
}
