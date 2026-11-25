//! The 8x8 compute filter fills a nine-element LDS stencil before all 64 lanes
//! consume it. Host subgroups may be smaller than the guest workgroup.
use super::{compute_group, Emitter, Enc, Insn, Opnd, ShaderExtract, Stage};

#[derive(Clone, Copy)]
pub(super) struct LdsTile {
    count_lo: usize,
    count_hi: usize,
    mask: usize,
    restore: usize,
    pair_move: Option<usize>,
}

impl LdsTile {
    pub(super) fn recognize(ex: &ShaderExtract, insns: &[Insn]) -> Option<Self> {
        if ex.stage != Stage::Compute || compute_group(ex) != [8, 8, 1] {
            return None;
        }
        let pos = |raw: &[u32]| insns.iter().position(|i| i.raw == raw);
        let count_lo = pos(&[0xd7650003, 0x000100c1])?;
        let count_hi = pos(&[0xd7660003, 0x000206c1])?;
        let mask = pos(&[0x92fe8089])?; // s_bfm_b64 exec, 9, 0
        let restore = pos(&[0xbefe04c1])?; // s_mov_b64 exec, -1
        if !(count_lo < count_hi && count_hi < mask && mask < restore)
            || insns.get(restore.checked_sub(1)?)?.raw != [0xd8380605, 0x00090403]
            || !insns[mask..restore]
                .iter()
                .any(|i| i.raw == [0xd87c0000, 0x00000701])
            || insns[..restore]
                .iter()
                .any(|i| i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some())
        {
            return None;
        }
        // The packed-output variant exchanges adjacent results immediately
        // before selecting the even lanes. All preceding skips must rejoin.
        let pair_move = pos(&[0x7e1402fa, 0xff0cb109]);
        if let Some(i) = pair_move {
            if insns.get(i + 1)?.raw != [0xbefe03ff, 0x55555555]
                || insns.get(i + 2)?.raw != [0xbeff03ff, 0x55555555]
                || insns[..i].iter().any(|b| {
                    b.enc == Enc::Sopp
                        && Emitter::branch_cond(b.op).is_some()
                        && (b.aux.imm < 0
                            || b.offset as i64 + 4 + b.aux.imm * 4 > insns[i].offset as i64)
                })
            {
                return None;
            }
        }
        Some(Self {
            count_lo,
            count_hi,
            mask,
            restore,
            pair_move,
        })
    }

    pub(super) fn emit(self, em: &mut Emitter<'_>, i: usize) -> Result<bool, String> {
        const LANE: &str = "(gtid.x + gtid.y * 8u)";
        if i == self.count_lo {
            em.wv(&Opnd::Vgpr(3), &format!("min({LANE}, 32u)"))?;
        } else if i == self.count_hi {
            em.wv(&Opnd::Vgpr(3), LANE)?;
        } else if i == self.mask {
            em.line(&format!("exec_lo = ({LANE} < 9u); exec_hi = 0u;"));
        } else if i == self.restore {
            em.line("GroupMemoryBarrierWithGroupSync();");
            return Ok(false); // emit the original EXEC restore
        } else if self.pair_move.is_some()
            && em.insns[i].mnemonic == "s_mov_b32"
            && matches!(
                em.insns[i].raw.as_slice(),
                [0xbeea03ff, 0x55555555 | 0xaaaaaaaa] | [0xbe8c03ff, 0xaaaaaaaa]
            )
            && em.insns.get(i + 1).is_some_and(|hi| {
                hi.raw[0] == em.insns[i].raw[0] + (1 << 16)
                    && hi.raw.get(1) == em.insns[i].raw.get(1)
            })
        {
            // These adjacent uniform mask words select low/high nibbles via
            // V_CNDMASK. The translator stores a lane-local bit in SDST.
            let word = em.insns[i].raw[1];
            let mask = em.insns[i].dst[0];
            em.ws(&mask, &format!("(({word:#x}u >> ({LANE} & 31u)) & 1u)"))?;
        } else if Some(i) == self.pair_move {
            // All LDS values have already been consumed at this point. The
            // same scratch space can hold the packed-output exchange.
            em.line("GroupMemoryBarrierWithGroupSync();");
            em.line(&format!("tb_lds[{LANE}] = v9;"));
            em.line("GroupMemoryBarrierWithGroupSync();");
            em.wv(&Opnd::Vgpr(10), &format!("tb_lds[{LANE} ^ 1u]"))?;
        } else if self.pair_move.is_some_and(|p| p + 1 == i) {
            em.line(&format!("exec_lo = (({LANE} & 1u) == 0u); exec_hi = 0u;"));
        } else if !self.pair_move.is_some_and(|p| p + 2 == i) {
            return Ok(false);
        }
        Ok(true)
    }
}
