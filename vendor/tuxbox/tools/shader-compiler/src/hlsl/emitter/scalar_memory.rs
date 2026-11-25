//! Scalar loads and flat loads.
use super::*;

impl<'a> Emitter<'a> {
    /// Scalar memory loads.
    pub(in crate::hlsl) fn emit_smem(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        if ins.op <= 4 {
            if let Some(Res::HeapLoad(offset)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
                let value = ssrc_name(u32::from(offset))?;
                self.regs.insert(value.clone());
                let dst = d.ok_or("missing direct descriptor destination")?;
                self.touch(&dst);
                let byte_offset = if ins.aux.imm == 0 { value }
                    else { format!("({value} + {:#x}u)", ins.aux.imm as u32) };
                self.ws(&dst, &format!("tb_heap_index({byte_offset} >> 5)"))?;
                return Ok(());
            }
            if let Some(Res::HeapSamplerLoad(offset)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
                let value = ssrc_name(u32::from(offset))?;
                self.regs.insert(value.clone());
                let dst = d.ok_or("missing sampler descriptor destination")?;
                self.touch(&dst);
                // The guest sampler-heap pointer is the heap start (as for the static
                // path), so the byte offset over 16 is the descriptor index.
                self.ws(&dst, &format!("({value} >> 4)"))?;
                return Ok(());
            }
            let chosen = !self.chosen_samplers.is_empty() && ins.op == 2
                && matches!(ins.src.first(), Some(Opnd::Sgpr(base)) if self.sampler_heap_base == Some(*base));
            if let Some(Opnd::Sgpr(dst)) = d.as_ref().filter(|_| chosen) {
                for (k, value) in [ins.aux.imm as u32 / 16, 0, 0, 0].into_iter().enumerate() {
                    let dst = sreg_opnd(dst + k as u8);
                    self.touch(&dst);
                    self.ws(&dst, &format!("{value}u"))?;
                }
                return Ok(());
            }
            if let Some(Res::CodeTbl) = self.res.get(&idx).and_then(|v| v.first()) {
                // Constant table embedded in the code section: byte address = pointer
                // (relative to the start of the program) + immediate.
                let n = 1u32 << ins.op;
                let base = sreg_num(&d.ok_or("no dst")?).ok_or("bad dst")?;
                let ndw = ((self.ex.isa_data.len().saturating_sub(self.ex.isa.len())) / 4).max(1);
                let t = self.tmp;
                self.tmp += 1;
                if let Some(offset) = self.static_smem_offset(ins) {
                    self.line(&format!("uint t{t} = ({} + {offset:#x}u) >> 2;", r(&s[0])?));
                    for k in 0..n {
                        self.line(&format!("uint t{t}_{k} = tb_code[(t{t} + {k}u - {}u) % {ndw}u];", self.ex.isa.len() / 4));
                    }
                } else {
                    let reg = self.dynamic_code_offset(idx).ok_or("unverified dynamic code-table offset")?;
                    let offset = Opnd::Sgpr(reg);
                    self.touch(&offset);
                    self.line(&format!("uint t{t} = ({} + {} + {:#x}u - {}u) >> 2;", r(&s[0])?, r(&offset)?, ins.aux.imm, self.ex.isa.len()));
                    for k in 0..n {
                        self.line(&format!("uint t{t}_{k} = tb_code[t{t} + {k}u];"));
                    }
                }
                for k in 0..n {
                    let dst = sreg_opnd(base + k as u8);
                    self.touch(&dst);
                    self.ws(&dst, &format!("t{t}_{k}"))?;
                }
                return Ok(());
            }
            self.line("// (descriptor fetch from root table — resolved statically)");
            return Ok(());
        }
        let n = 1u32 << (ins.op - 8);
        let base = sreg_num(&d.ok_or("no dst")?).ok_or("bad dst")?;
        let soff = ins.raw.get(1).copied().unwrap_or(0) >> 25;
        // SOFFSET is not an operand of the decoded instruction. VCC and the other
        // special registers are always declared.
        if soff < 106 {
            self.regs.insert(ssrc_name(soff)?);
        }
        let slot = match self.res.get(&idx).and_then(|v| v.first()).copied() {
            Some(Res::HeapBuffer(src)) if self.compute_heap => {
                // A compute heap descriptor names a raw SRV (geometry and instance
                // data read by both scalar and vector loads), not a CBV.
                let t = self.tmp;
                self.tmp += 1;
                let offset = if soff != 0x7d { format!(" + {}", ssrc_name(soff)?) } else { String::new() };
                let buf = self.bind_heap(HeapView::Raw, src);
                self.line(&format!("uint tb_o{t} = ({}u{offset}) & ~3u;", ins.aux.imm as u32));
                for k in 0..n {
                    self.line(&format!("uint t{t}_{k} = {buf}.Load(tb_o{t} + {}u);", 4 * k));
                }
                for k in 0..n {
                    let dst = sreg_opnd(base + k as u8);
                    self.touch(&dst);
                    self.ws(&dst, &format!("t{t}_{k}"))?;
                }
                return Ok(());
            }
            Some(Res::HeapBuffer(src)) => {
                let t = self.tmp;
                self.tmp += 1;
                let offset = if soff != 0x7d { format!(" + {}", ssrc_name(soff)?) } else { String::new() };
                let buf = self.bind_heap(HeapView::Constants, src);
                self.line(&format!("uint tb_o{t} = ({}u{offset}) >> 2;", ins.aux.imm as u32));
                for k in 0..n {
                    self.line(&format!("uint t{t}_{k} = {buf}.data[(tb_o{t}+{k}u) >> 2][(tb_o{t}+{k}u)&3u];"));
                }
                for k in 0..n {
                    let dst = sreg_opnd(base + k as u8);
                    self.touch(&dst);
                    self.ws(&dst, &format!("t{t}_{k}"))?;
                }
                return Ok(());
            }
            Some(Res::Cb(s)) => s,
            Some(res @ (Res::Buf(slot, is_uav) | Res::Raw(slot, is_uav))) => {
                // Snapshot all words before replacing any overlapping destination SGPR.
                let raw = matches!(res, Res::Raw(..));
                let nm = if is_uav { "ubuf" } else { "sbuf" };
                let dynamic = if soff != 0x7d { format!(" + {}", ssrc_name(soff)?) } else { String::new() };
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t} = {:#x}u{dynamic};", ins.aux.imm as u32));
                for k in 0..n {
                    let load = if raw {
                        format!("raw_{nm}{slot}.Load(t{t} + {}u)", 4 * k)
                    } else {
                        format!("tb_sbw_{nm}{slot}(t{t} + {}u)", 4 * k)
                    };
                    self.line(&format!("uint t{t}_{k} = {load};"));
                }
                for k in 0..n {
                    let dst = sreg_opnd(base + k as u8);
                    self.touch(&dst);
                    self.ws(&dst, &format!("t{t}_{k}"))?;
                }
                return Ok(());
            }
            _ => return Err("unresolved constant buffer".into()),
        };
        if soff != 0x7d {
            // Dynamic byte offset = imm + SGPR[soffset].
            let so = ssrc_name(soff)?;
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t} = ({:#x}u + {so}) >> 2;", ins.aux.imm as u32));
            // Read all words before writing (the offset register may be a destination).
            for k in 0..n {
                self.line(&format!("uint t{t}_{k} = cb{slot}[((t{t} + {k}u) >> 2) & 4095u][(t{t} + {k}u) & 3u];"));
            }
            for k in 0..n {
                let dst = sreg_opnd(base + k as u8);
                self.touch(&dst);
                self.ws(&dst, &format!("t{t}_{k}"))?;
            }
            return Ok(());
        }
        let w0 = ins.aux.imm as u32 / 4;
        for k in 0..n {
            let w = w0 + k;
            let dst = sreg_opnd(base + k as u8);
            self.touch(&dst);
            self.ws(&dst, &format!("cb{slot}[{}].{}", w / 4, XYZW[(w % 4) as usize]))?;
        }
        Ok(())
    }

    /// Loads through a flat address.
    pub(in crate::hlsl) fn emit_flat(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        match ins.enc {
            Enc::Flat if m == "flat_load_ushort" => {
                code_table::halfword(&self.insns, idx, self.ex.isa.len(), self.ex.isa_data.len())
                    .ok_or("halfword load does not address a proven PC-relative table")?;
                self.uses_code = true;
                let t = self.tmp;
                self.tmp += 1;
                let addr = r(&s[0])?;
                self.line(&format!("uint t{t} = {addr} - {}u;", self.ex.isa.len()));
                self.wv(&d.ok_or("bad halfword destination")?, &format!("(tb_code[t{t} >> 2] >> ((t{t} & 2u) * 8u)) & 0xffffu"))
            }
            Enc::Flat if m.starts_with("flat_load_dword") => {
                // Per-lane load through a 64-bit address. Shaders use this for lookups in
                // constant tables embedded in their own code section (address derived from
                // `s_getpc_b64`), so it is only accepted when the program takes its PC.
                if ins.aux.mask == 1 {
                    return Err("scratch-memory load unsupported".into());
                }
                if !self.insns.iter().any(|i| i.mnemonic == "s_getpc_b64") {
                    return Err("global/flat load in a shader that never takes its PC".into());
                }
                self.uses_code = true;
                let n: u8 = match m {
                    "flat_load_dword" => 1,
                    "flat_load_dwordx2" => 2,
                    "flat_load_dwordx3" => 3,
                    _ => 4,
                };
                let extra = match &s[2] {
                    Opnd::Null => String::new(),
                    o => format!(" + {}", r(o)?),
                };
                let ndw = ((self.ex.isa_data.len().saturating_sub(self.ex.isa.len())) / 4).max(1);
                let vd = match d {
                    Some(Opnd::Vgpr(v)) => v,
                    _ => return Err("bad vdst".into()),
                };
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t} = ({} + {:#x}u{extra}) >> 2;", r(&s[0])?, ins.aux.imm as i32 as u32));
                for k in 0..n {
                    self.line(&format!("uint t{t}_{k} = tb_code[(t{t} + {k}u - {}u) % {ndw}u];", self.ex.isa.len() / 4));
                }
                for k in 0..n {
                    self.wv(&Opnd::Vgpr(vd + k), &format!("t{t}_{k}"))?;
                }
                Ok(())
            }
            _ => Err("unsupported encoding".into()),
        }
    }
}
