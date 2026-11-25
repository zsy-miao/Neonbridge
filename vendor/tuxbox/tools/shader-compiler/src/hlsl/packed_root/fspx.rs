//! Register-root ABI used by FidelityFX postpasses. The argument registers hold
//! descriptor table addresses (one word) and root CBV addresses (two words).
use super::*;

mod convolution;
pub(in crate::hlsl) use convolution::layout as convolution_layout;
mod exposure;
mod prepass;

fn raw(insns: &[Insn], words: &[&[u32]]) -> bool {
    insns.len() >= words.len() && insns.iter().zip(words).all(|(i, w)| i.raw == *w)
}

fn fixed_load(i: &Insn, encoding: u32) -> Option<u32> {
    match i.raw[..] {
        [op, offset] if op == encoding && offset & 0xffe0_0003 == 0xfa00_0000 => {
            Some(offset & 0x1f_ffff)
        }
        _ => None,
    }
}

/// Prove the address registers are never observed as numbers. Initially s8:s9
/// contain the loaded constants, s10:s11 the descriptor literals, and s14 group X.
/// At a forward control-flow join a scalar must have a value on every incoming path.
fn check_alu(body: &[Insn], descriptor: usize, end: usize) -> Result<(), String> {
    let mut defined = (0xfu128 << 8) | (1 << 14);
    let mut incoming = BTreeMap::new();
    for i in body {
        if let Some(other) = incoming.remove(&i.offset) {
            defined &= other;
        }
        let m = i.mnemonic.as_str();
        if m == "s_cbranch_execz" {
            let target = i.offset as i64 + 4 + i.aux.imm * 4;
            if target <= i.offset as i64 || (target > descriptor as i64 && target != end as i64) {
                return Err("branch enters FSPX descriptor use".into());
            }
            incoming
                .entry(target as usize)
                .and_modify(|bits| *bits &= defined)
                .or_insert(defined);
            continue;
        }
        let inputs = match (i.enc, m) {
            (Enc::Sopp, "s_waitcnt") => continue,
            // Unused trailing VOP3 fields encode s0. With 32-bit operands the
            // source registers below are all the scalar inputs of the instruction.
            (Enc::Vop3, _) if i.op < 0x100 => &i.src[..2],
            (Enc::Vop3, "v_mul_lo_u32" | "v_mul_hi_u32") => &i.src[..2],
            (Enc::Vop3, "v_lshrrev_b64")
                if i.src == [Opnd::ConstInt(0), Opnd::ConstInt(0), Opnd::Sgpr(0)] =>
            {
                continue
            }
            (Enc::Vop3, "v_add_co_ci_u32") if (i.raw[0] >> 8) & 0x7f == 106 => &i.src[..],
            (Enc::Vop3, "v_cndmask_b32" | "v_add_lshl_u32" | "v_add3_u32") => &i.src[..],
            (Enc::Vopc | Enc::Vop1 | Enc::Vop2, _) if !m.ends_with("64") => &i.src[..],
            (Enc::Sop1, "s_mov_b32") => &i.src[..],
            (Enc::Sop2, "s_add_u32" | "s_sub_i32" | "s_min_u32" | "s_mul_i32" | "s_andn2_b32") => {
                &i.src[..]
            }
            _ => return Err(format!("unsupported FSPX operation {m}")),
        };
        if inputs
            .iter()
            .any(|o| matches!(o, Opnd::Sgpr(n) if defined & (1u128 << n) == 0))
        {
            return Err(format!("unresolved FSPX scalar input at {:#x}", i.offset));
        }
        if let [Opnd::Sgpr(n)] = i.dst[..] {
            if n == 4 {
                return Err("FSPX UAV table register overwritten".into());
            }
            defined |= 1u128 << n;
        }
    }
    Ok(())
}

impl PackedRoot {
    pub(super) fn fspx(ex: &ShaderExtract, insns: &[Insn]) -> Result<Self, String> {
        let root = ex.root_sig.as_ref().ok_or("FSPX needs a root signature")?;
        let [uavs, srvs, cbvs @ ..] = &root.params[..] else {
            return Err("unsupported FSPX root layout".into());
        };
        let RootParamKind::Table(u) = &uavs.kind else {
            return Err("expected FSPX UAV table".into());
        };
        let [u] = &u[..] else {
            return Err("unsupported FSPX UAV ranges".into());
        };
        let RootParamKind::Table(srvs) = &srvs.kind else {
            return Err("expected FSPX SRV table".into());
        };
        if ex.psv_threads == Some([256, 1, 1])
            && ex.fspx_config == Some([0x4005cf00, 0x6c090102, 0, 0x10100, 1, 0x402c0349, 0x18190])
        {
            return exposure::bindings(ex, insns, root, u, srvs, cbvs);
        }
        let sharpen = ex.psv_threads == Some([64, 1, 1])
            && ex.fspx_config == Some([0x4005cf00, 0x6c090102, 0, 0x10040, 1, 0x402c0349, 0x190]);
        if srvs.is_empty() || (!sharpen && srvs.len() != 1) {
            return Err("unsupported FSPX SRV ranges".into());
        }
        let expected_cbvs = if sharpen { 1 } else { 4 };
        if srvs.iter().any(|s| s.range_type != 0 || s.space != 0)
            || cbvs.len() != expected_cbvs
            || !cbvs.iter().enumerate().all(|(i, p)| {
                matches!(p.kind,
                RootParamKind::Descriptor { kind: 2, register, space: 0 } if register == i as u32)
            })
            || u.range_type != 1
            || u.space != 0
            || !matches!(u.offset_in_table, 0 | u32::MAX)
        {
            return Err("unsupported FSPX root signature".into());
        }
        if let Some(abi) = convolution::layout(ex) {
            return convolution::bindings(ex, insns, u, &srvs[0], abi);
        }
        if sharpen
            || (ex.psv_threads == Some([256, 1, 1])
                && ex.fspx_config
                    == Some([
                        0x4005cf00, 0x6c090102, 0, 0x10100, 0x100001, 0x402c0345, 0x1819c,
                    ]))
        {
            return prepass::bindings(ex, insns, u, srvs, cbvs.len());
        }
        if ex.psv_threads != Some([32, 1, 1])
            || ex.fspx_config
                != Some([
                    0x4005cf00, 0x6c090102, 0, 0x10020, 0x100001, 0x402c0341, 0x9c,
                ])
        {
            return Err("unsupported FSPX compute register layout".into());
        }
        let [cb, uav] = &ex.resources[..] else {
            return Err("unsupported FSPX resource layout".into());
        };
        if cb.res_type != 2
            || cb.space != 0
            || cb.lower != 0
            || cb.upper != 0
            || uav.res_type != 7
            || uav.space != 0
            || uav.lower != uav.upper
        {
            return Err("expected FSPX root CBV and raw UAV".into());
        }
        let first = insns
            .iter()
            .position(|i| !matches!(i.mnemonic.as_str(), "v_nop" | "s_inst_prefetch"))
            .ok_or("missing FSPX prologue")?;
        // Build the b0 descriptor from s[6:7], load two words, and form the
        // linear dispatch index from s14 (group X) and v0 (local X).
        let start = &insns[first..];
        if !raw(
            start,
            &[&[0xbe880406], &[0xbe8a03c1], &[0xbe8b03ff, 0x31016fac]],
        ) || !start
            .get(4)
            .is_some_and(|i| i.raw == [0xd7460000, 0x04010a0e])
        {
            return Err("unsupported FSPX root CBV prologue".into());
        }
        let cb_off = fixed_load(&start[3], 0xf4240204)
            .filter(|n| *n <= 65536 - 8)
            .ok_or("invalid FSPX root CBV load")?
            / 4;
        let mut plan = Self {
            abi: RootAbi::RegisterWave32,
            ..Self::default()
        };
        plan.loads
            .insert(first + 3, vec![Some((0, cb_off)), Some((0, cb_off + 1))]);

        let desc = insns
            .iter()
            .position(|i| i.mnemonic == "s_load_dwordx4")
            .ok_or("missing FSPX UAV descriptor")?;
        if desc < first + 7 || !raw(&insns[desc - 2..], &[&[0xbe880304], &[0xbe890380]]) {
            return Err("expected FSPX UAV table in s4".into());
        }
        let off = fixed_load(&insns[desc], 0xf4080204).ok_or("invalid FSPX UAV descriptor load")?;
        if off % 32 != 0
            || off / 32 >= u.num
            || u.base_register.checked_add(off / 32) != Some(uav.lower)
        {
            return Err("FSPX table offset disagrees with PSV UAV".into());
        }

        check_alu(
            &insns[first + 5..desc - 2],
            insns[desc - 2].offset,
            insns.last().unwrap().offset,
        )?;

        // Compiler converts the loaded descriptor into a byte-addressed view:
        // multiply record count by max(stride,1), clear stride, select raw OOB.
        // Only the resulting descriptor feeds the stores. Placeholder words in
        // the emitted scalar arithmetic are unobservable under the checks below.
        plan.loads.insert(desc, vec![None; 4]);
        let mut normalized = false;
        let mut tail = desc + 1;
        while let Some(i) = insns.get(tail) {
            if !normalized
                && raw(
                    &insns[tail..],
                    &[
                        &[0x938cff09, 0xe0010],
                        &[0x848c810c],
                        &[0x8709ff09, 0xc000ffff],
                        &[0x870bff0b, 0xcfffffff],
                        &[0x930a0c0a],
                        &[0x880bff0b, 0x30000000],
                    ],
                )
            {
                normalized = true;
                tail += 6;
                continue;
            }
            match i.mnemonic.as_str() {
                "s_waitcnt" | "s_endpgm" => {}
                "v_lshrrev_b64"
                    if i.src == [Opnd::ConstInt(0), Opnd::ConstInt(0), Opnd::Sgpr(0)] => {}
                "v_and_b32" | "v_add_nc_u32"
                    if i.src.iter().all(|o| {
                        matches!(o, Opnd::Vgpr(_) | Opnd::ConstInt(_) | Opnd::Literal(_))
                    }) => {}
                "buffer_store_dwordx4"
                    if normalized
                        && i.raw[0] == 0xe0781000
                        && i.raw[1] & 0xffff0000 == 0x80020000 =>
                {
                    plan.buffers.insert(
                        tail,
                        (
                            Binding {
                                slot: uav.lower,
                                uav: true,
                            },
                            0,
                        ),
                    );
                }
                _ => return Err(format!("unsupported FSPX UAV use at {:#x}", i.offset)),
            }
            tail += 1;
        }
        if plan.buffers.is_empty() {
            return Err("missing FSPX raw UAV store".into());
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skipped_assignment_cannot_resolve_a_root_address() {
        // s_cbranch_execz +1; s_mov_b32 s12, 0; s_mov_b32 s13, s12
        let words = [0xbf880001u32, 0xbe8c0380, 0xbe8d030c];
        let body = disassemble(
            &words
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        assert!(check_alu(&body, 12, 32)
            .unwrap_err()
            .contains("scalar input"));
        assert!(check_alu(&body[1..], 12, 32).is_ok());
    }
}
