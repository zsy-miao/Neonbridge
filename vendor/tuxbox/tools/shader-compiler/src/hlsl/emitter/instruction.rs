//! One instruction: dispatch by encoding, and the scalar instructions.
use super::*;

impl<'a> Emitter<'a> {
    pub(in crate::hlsl) fn emit_insn(&mut self, idx: usize, ins: &Insn, is_ps: bool, br: Option<usize>) -> Result<(), String> {
        self.emit_insn_body(idx, ins, is_ps, br)?;
        if let Some(vote) = self.lockstep.as_ref().and_then(|votes| votes.get(&idx)).cloned() {
            let mask = r(&vote.mask)?;
            let not = if vote.none { "!" } else { "" };
            self.line(&format!("scc = uint({not}WaveActiveAnyTrue(({mask} & 1u) != 0u));"));
        }
        if self.vote_defs.contains(&idx) {
            let mask = r(&ins.dst[0])?;
            self.line(&format!("tb_votes{idx} = WaveActiveCountBits(({mask} & 1u) != 0u);"));
        }
        Ok(())
    }

    pub(in crate::hlsl) fn emit_insn_body(&mut self, idx: usize, ins: &Insn, is_ps: bool, br: Option<usize>) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        self.line(&format!("// {:#06x} {m}", ins.offset));
        self.cur_idx = idx;
        if let Some(&site) = self.exec_snapshots.sites.get(&idx) {
            if site.emit(self, idx, ins)? { return Ok(()); }
        }
        if m == "tb_reflection_far_query" {
            reflection_far::emit(self, ins);
            return Ok(());
        }
        if m == "tb_radiance_query" {
            radiance_probe::emit(self, ins);
            return Ok(());
        }
        if let Some(reg) = self.packed.as_ref().and_then(|p| p.ballot_nonempty.get(&idx)) {
            self.line(&format!("s{reg} &= exec_lo; scc = uint(WaveActiveAnyTrue((s{reg} & 1u) != 0u));"));
            return Ok(());
        }
        if ins.enc == Enc::Ds && m != "ds_swizzle_b32" && !self.hoist_wave_exchanges && wave_cull::lds_words(self).is_some() {
            // Publish writes and finish reads before the next guest LDS access.
            self.line("GroupMemoryBarrierWithGroupSync();");
        }
        if let Some(width) = self.fspx_wave {
            if fspx_wave::emit(self, idx, ins, width)? { return Ok(()); }
        }
        if self.wave_cull {
            wave_cull::prepare_insn(self, idx, ins)?;
            if wave_cull::emit(self, idx, ins, br)? { return Ok(()); }
        }
        if self.page_waves && page_waves::emit(self, idx, ins, br)? { return Ok(()); }
        match m {
            "tb_light_grid_reserve" => {
                self.line(light_grid::RESERVE);
                return Ok(());
            }
            "tb_first_exec_lane" => return self.ws(&d.ok_or("missing election result")?, &format!("election{idx}")),
            "tb_indexed_readlane" => {
                if self.hoisted_swizzle != Some(idx) { self.emit_indexed_read(idx, ins)?; }
                self.ws(&d.ok_or("missing lane-read destination")?, &format!("readlane{idx}"))?;
                return Ok(());
            }
            "tb_view_occluded" => {
                self.line("hidden = (exec_lo & 1u) != 0u; hidden_id = v0; hidden_view = cull_view;");
                return Ok(());
            }
            "tb_view_candidate" => {
                self.line("cull_candidate = (exec_lo & 1u) != 0u;");
                self.line(&format!("cull_record = uint3({}, {}, {});", r(&ins.src[0])?, r(&ins.src[1])?, r(&ins.src[2])?));
                return Ok(());
            }
            "s_ff1_i32_b64" | "s_ff1_i32_b32" | "tb_first_active_lane" if lone_lane::on() && ins.src == [Opnd::ExecLo] => {
                return self.ws(&d.ok_or("missing elected lane register")?, "((exec_lo & 1u) ? 0u : 0xffffffffu)");
            }
            "s_ff1_i32_b64" if ins.src == [Opnd::ExecLo] => {
                // WaveActiveMin returns the same lane ID to all participating invocations.
                let dst = d.ok_or("missing elected lane register")?;
                return self.ws(&dst, "WaveActiveMin((exec_lo & 1u) ? WaveGetLaneIndex() : 0xffffffffu)");
            }
            "tb_subsurface_group_sync" => {
                self.line("GroupMemoryBarrierWithGroupSync();");
                return Ok(());
            }
            "tb_first_active_lane" => {
                let dst = d.ok_or("missing elected lane register")?;
                return self.ws(&dst, "WaveActiveMin((exec_lo & 1u) ? WaveGetLaneIndex() : 0xffffffffu)");
            }
            "tb_capture_allocation_mask32" => {
                let dst = d.ok_or("missing mask register")?;
                return self.ws(&dst, "WaveActiveBallot((exec_lo & 1u) != 0u).x");
            }
            "tb_capture_allocation_mask" => {
                let dst = d.ok_or("missing mask register")?;
                let upper = hi(&dst);
                self.touch(&upper);
                self.line(&format!("uint4 alloc_mask{idx} = WaveActiveBallot((exec_lo & 1u) != 0u);"));
                self.ws(&dst, &format!("alloc_mask{idx}.x"))?;
                return self.ws(&upper, &format!("alloc_mask{idx}.y"));
            }
            "tb_allocation_prefix_lo" | "tb_allocation_prefix_hi" => {
                let high = m.ends_with("_hi");
                let lane = if high { "max(32u, WaveGetLaneIndex()) - 32u" } else { "WaveGetLaneIndex()" };
                let word = if high { "y" } else { "x" };
                let prefix = format!("countbits(WaveActiveBallot((exec_lo & 1u) != 0u).{word} & {})", low_mask(lane));
                return self.wv(&d.ok_or("missing prefix destination")?, &format!("({} + {prefix})", r(&s[1])?));
            }
            _ => {}
        }
        if wave_alloc::emit(self, ins)? { return Ok(()); }
        if hair_cluster::emit(self, ins) { return Ok(()); }
        if hair_cluster_export::emit(self, ins) { return Ok(()); }
        if m == "tb_hidden_instance" {
            self.line("hidden = (exec_lo & 1u) != 0u; hidden_id = v0; if (hidden) v42 = v28;");
            return Ok(());
        }
        if instance_cull::emit(self, ins)? { return Ok(()); }
        if ps09_classify::emit(self, ins) { return Ok(()); }
        if let Some(tile) = self.lds_tile {
            if tile.emit(self, idx)? { return Ok(()); }
        }
        if let Some(&id) = self.epilog_pack.get(&idx) {
            // Export epilogue: keep the unpacked operands (see `find_export_epilogs`).
            for (k, sfx) in ["a", "b"].iter().enumerate() {
                let o = *s.get(k).ok_or("pack without two operands")?;
                self.touch(&o);
                self.regs.insert(format!("tb_pk{id}{sfx}"));
                self.line(&format!("tb_pk{id}{sfx} = {};", r(&o)?));
            }
        }
        // Run-time descriptor selectors (only for shaders with a "one of N" image op).
        if let Some((reg, cands)) = self.bsel.remove(&idx) {
            // One guarded copy of the op per candidate buffer.
            self.regs.insert(format!("tb_dsel{reg}"));
            let mut first = true;
            for (id, res) in &cands {
                self.line(&format!("{}if (tb_dsel{reg} == {id:#x}u) {{", if first { "" } else { "else " }));
                first = false;
                self.res.insert(idx, vec![*res]);
                self.emit_insn(idx, ins, is_ps, br)?;
                self.line("}");
                self.drop_heap_handles();
            }
            self.bsel.insert(idx, (reg, cands));
            return Ok(());
        }
        if !self.msel.is_empty() || !self.bsel.is_empty() {
            if let Some(&(d, table, reg, imm)) = self.dsel_runtime.get(&idx) {
                let offset = Opnd::Sgpr(reg);
                self.touch(&offset);
                self.regs.insert(format!("tb_dsel{d}"));
                self.line(&format!("tb_dsel{d} = {:#x}u | ({} + {imm:#x}u);",
                    u32::from(table) << 24, r(&offset)?));
            }
            if let Some(&(d, id)) = self.dsel_loads.get(&idx) {
                self.regs.insert(format!("tb_dsel{d}"));
                self.line(&format!("tb_dsel{d} = {id:#x}u;"));
            }
            if let Some(&(d, s)) = self.dsel_moves.get(&idx) {
                self.regs.insert(format!("tb_dsel{d}"));
                self.regs.insert(format!("tb_dsel{s}"));
                self.line(&format!("tb_dsel{d} = tb_dsel{s};"));
            }
        }
        if ins.enc == Enc::Mimg {
            if let Some(sel) = self.msel.get(&idx).cloned() {
                // One guarded copy of the op per (texture, sampler) candidate pair.
                self.regs.insert(format!("tb_dsel{}", sel.tex_reg));
                self.regs.insert(format!("tb_dsel{}", sel.samp_reg));
                let samps: Vec<Option<(u32, Res)>> =
                    if sel.samp.is_empty() { vec![None] } else { sel.samp.iter().copied().map(Some).collect() };
                let mut first = true;
                for (tid, tres) in &sel.tex {
                    for s in &samps {
                        let mut cond = vec![];
                        if sel.tex.len() > 1 {
                            cond.push(format!("tb_dsel{} == {tid:#x}u", sel.tex_reg));
                        }
                        let mut v = vec![*tres];
                        if let Some((sid, sres)) = s {
                            if sel.samp.len() > 1 {
                                cond.push(format!("tb_dsel{} == {sid:#x}u", sel.samp_reg));
                            }
                            v.push(*sres);
                        }
                        self.line(&format!("{}if ({}) {{", if first { "" } else { "else " }, cond.join(" && ")));
                        first = false;
                        self.res.insert(idx, v);
                        self.emit_mimg(idx, ins)?;
                        self.line("}");
                        self.drop_heap_handles();
                    }
                }
                return Ok(());
            }
        }
        if scalar_bits::emit(self, idx, ins)? { return Ok(()); }
        match ins.enc {
            Enc::Sopp => self.emit_sopp(idx, ins, br),
            Enc::Vop2 | Enc::Vop1 | Enc::Vop3 | Enc::Vopc => self.emit_vec(ins),
            Enc::Sop2 => self.emit_sop2(idx, ins, is_ps),
            Enc::Sop1 => self.emit_sop1(idx, ins, is_ps),
            Enc::Sopk => self.emit_sopk(ins),
            Enc::Sopc => self.emit_sopc(ins),
            Enc::Smem if self.packed.is_some() => self.emit_packed_load(idx, ins),
            Enc::Smem => self.emit_smem(idx, ins),
            Enc::Mimg => self.emit_mimg(idx, ins),
            Enc::Mtbuf => self.emit_mtbuf(idx, ins),
            Enc::Mubuf if ins.op == 0x71 || ins.op == 0x72 => {
                // D3D12 exposes UAV memory visibility, not individual cache levels.
                // A device fence has no thread rendezvous; guest S_BARRIER is separate.
                if self.is_cs {
                    self.line("DeviceMemoryBarrier();");
                }
                Ok(())
            }
            Enc::Mubuf if self.packed.is_some() => self.emit_packed_buffer(idx, ins),
            Enc::Mubuf => self.emit_mubuf(idx, ins),
            Enc::Flat => self.emit_flat(idx, ins),
            Enc::Ds => self.emit_ds(idx, ins, is_ps),
            Enc::Vinterp => self.emit_vinterp(ins, is_ps),
            Enc::Exp => self.emit_exp(ins, is_ps),
            _ => Err("unsupported encoding".into()),
        }
    }
}
