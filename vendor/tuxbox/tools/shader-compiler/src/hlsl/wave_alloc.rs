//! Wave-size-independent forms of the 64-lane integer sum and exclusive prefix
//! sum that compilers emit around an elected atomic append. The exact DPP
//! emulation in `wave_sum` needs host waves of 64 lanes, which only compute
//! shaders can request. When the row partials, cross-row values and half-wave
//! scalars die unread, the sequence is only a whole-wave total or an exclusive
//! prefix of values that are zero in inactive lanes. `WaveActiveSum` and
//! `WavePrefixSum` compute exactly that on every host wave width.
use super::{Emitter, Enc, Insn, Opnd};
use std::collections::BTreeSet;

pub(super) const TOTAL: &str = "tb_wave_total_u32";
pub(super) const PREFIX: &str = "tb_wave_exclusive_prefix_u32";
// Opcode for the rewritten instructions, so passes matching READLANE or VOP2
// opcodes ignore them.
const PSEUDO_OP: u32 = 0x7ff3;

#[derive(Clone, Copy, PartialEq)]
enum Reg { V(u8), S(u8) }

fn memory_width(m: &str) -> Option<u16> {
    let wide = |m: &str| if m.contains("x16") { 16 } else if m.contains("x8") { 8 } else if m.contains("x4") || m.contains("xyzw") { 4 }
        else if m.contains("x3") || m.contains("xyz") { 3 } else if m.contains("x2") || m.contains("xy") { 2 } else { 1 };
    ["buffer_", "tbuffer_", "image_", "global_", "ds_", "exp", "s_load", "s_buffer_load"].iter()
        .any(|p| m.starts_with(p)).then(|| wide(m).max(4))
}

fn covers(o: &Opnd, r: Reg, width: u16) -> bool {
    let (k, n) = match (*o, r) {
        (Opnd::Vgpr(k), Reg::V(n)) | (Opnd::Sgpr(k), Reg::S(n)) => (k, n),
        _ => return false,
    };
    u16::from(k) <= u16::from(n) && u16::from(n) < u16::from(k) + width
}

fn reads(ins: &Insn, r: Reg) -> bool {
    let m = ins.mnemonic.as_str();
    if let Some(width) = memory_width(m) {
        // Store and atomic data are decoded as destinations. MUBUF VADDR is
        // ignored when neither OFFEN nor IDXEN is set.
        let unused_vaddr = ins.enc == Enc::Mubuf && ins.aux.mask & 3 == 0;
        let data = !m.starts_with("s_") && !m.contains("_load");
        return ins.src.iter().enumerate().any(|(i, o)| !(unused_vaddr && i == 0) && covers(o, r, width))
            || (data || m.contains("atomic")) && ins.dst.iter().any(|o| covers(o, r, width));
    }
    match r {
        Reg::S(n) => super::reads_sgpr(ins, n),
        Reg::V(_) => {
            let width = if m.contains("64") && m.starts_with("v_") { 2 } else { 1 };
            ins.src.iter().any(|o| covers(o, r, width))
        }
    }
}

/// A write that defines `r` in every lane that can later read it.
fn kills(ins: &Insn, r: Reg, whole_wave: bool) -> bool {
    let m = ins.mnemonic.as_str();
    match r {
        Reg::S(n) => {
            if matches!(m, "v_readlane_b32" | "v_readfirstlane_b32") {
                return matches!(ins.dst.first(), Some(Opnd::Vgpr(k) | Opnd::Sgpr(k)) if *k == n);
            }
            let width = if m.starts_with("s_load") || m.starts_with("s_buffer_load") { memory_width(m).unwrap_or(1) }
                else if m.starts_with("s_") && m.ends_with("64") { 2 } else { 1 };
            (m.starts_with("s_") || m.starts_with("v_cmp")) && ins.dst.iter().any(|o| covers(o, r, width))
        }
        // Inactive lanes keep the old value; only whole-wave vector writes kill.
        Reg::V(_) => whole_wave && m.starts_with("v_") && !m.starts_with("v_cmp") && memory_width(m).is_none()
            && !matches!(m, "v_readlane_b32" | "v_readfirstlane_b32") && ins.dst == [match r { Reg::V(n) => Opnd::Vgpr(n), Reg::S(n) => Opnd::Sgpr(n) }],
    }
}

/// Every path from `start` overwrites `r` before reading it.
fn dead_from(insns: &[Insn], start: usize, r: Reg) -> bool {
    let index_of = |offset: i64| insns.iter().position(|i| i.offset as i64 == offset);
    let mut stack = vec![(start, false)];
    let mut seen = BTreeSet::new();
    while let Some((mut at, mut whole)) = stack.pop() {
        loop {
            if !seen.insert((at, whole)) { break; }
            let Some(ins) = insns.get(at) else { break };
            if reads(ins, r) {
                if std::env::var_os("TB_DFDEBUG").is_some() { eprintln!("wave_alloc: live at {:#x} {}", ins.offset, ins.mnemonic); }
                return false;
            }
            if kills(ins, r, whole) { break; }
            let m = ins.mnemonic.as_str();
            if ins.dst.contains(&Opnd::ExecLo) || ins.dst.contains(&Opnd::ExecHi) || m.contains("saveexec") || m.starts_with("v_cmpx") {
                whole = m == "s_or_saveexec_b64" && ins.src == [Opnd::ConstInt(-1)]
                    || m == "s_mov_b64" && ins.dst == [Opnd::ExecLo] && ins.src == [Opnd::ConstInt(-1)];
            }
            if matches!(m, "s_setpc_b64" | "s_swappc_b64") { return false; }
            if m == "s_endpgm" { break; }
            if ins.enc == Enc::Sopp {
                let target = ins.offset as i64 + 4 + 4 * ins.aux.imm;
                if ins.op == 2 {
                    let Some(t) = index_of(target) else { return false };
                    at = t;
                    continue;
                }
                if Emitter::branch_cond(ins.op).is_some() {
                    let Some(t) = index_of(target) else { return false };
                    stack.push((t, whole));
                }
            }
            at += 1;
        }
    }
    true
}

fn dpp(ins: &Insn, acc: u8, selector: u16, ctrl: u32) -> bool {
    ins.mnemonic == "v_add_nc_u32" && ins.dst == [Opnd::Vgpr(acc)]
        && ins.src == [Opnd::Raw(selector), Opnd::Vgpr(acc)] && ins.raw.get(1) == Some(&(ctrl << 8 | u32::from(acc)))
}

fn readlane(ins: &Insn, acc: u8, lane: i64) -> Option<u8> {
    match (ins.mnemonic.as_str(), ins.dst.first(), ins.src.get(..2)) {
        ("v_readlane_b32", Some(&Opnd::Vgpr(d)), Some([src, Opnd::ConstInt(l)])) if *src == Opnd::Vgpr(acc) && *l == lane => Some(d),
        _ => None,
    }
}

fn is(ins: &Insn, m: &str, dst: &[Opnd], src: &[Opnd]) -> bool {
    ins.mnemonic == m && ins.dst == dst && ins.src.get(..src.len()) == Some(src)
}

fn nop(ins: &mut Insn) {
    ins.mnemonic = "s_nop".into();
    ins.enc = Enc::Sopp;
    ins.op = 0;
    ins.dst.clear();
    ins.src.clear();
}

// DPP8 pairs, quads and octets, then DPP16 row_mirror (see `wave_sum`).
fn total(insns: &mut [Insn], a: usize) -> Option<()> {
    let w = insns.get(a..a + 12)?;
    let Opnd::Sgpr(saved) = *w[0].dst.first()? else { return None };
    let Opnd::Vgpr(acc) = *w[1].dst.first()? else { return None };
    let Opnd::Vgpr(tmp) = *w[6].dst.first()? else { return None };
    let (v, s) = (Opnd::Vgpr(acc), Opnd::Sgpr(saved));
    if !is(&w[0], "s_or_saveexec_b64", &[s], &[Opnd::ConstInt(-1)])
        || !matches!(w[1].src[..], [Opnd::ConstInt(0), Opnd::Vgpr(_), m] if m == s) || w[1].mnemonic != "v_cndmask_b32"
        || ![0xde54c1, 0x936012, 0x000924, 0xff0940].iter().enumerate().all(|(i, &c)| dpp(&w[2 + i], acc, if i == 3 { 250 } else { 233 }, c))
        || !is(&w[6], "v_permlanex16_b32", &[Opnd::Vgpr(tmp)], &[v, Opnd::ConstInt(0), Opnd::ConstInt(0)])
        || !is(&w[7], "v_add_nc_u32", &[v], &[Opnd::Vgpr(tmp), v]) || w[6].raw[0] >> 11 & 0xf != 0 {
        return None;
    }
    let high = readlane(&w[8], acc, 32)?;
    let sum = readlane(&w[10], acc, 0)?;
    if !is(&w[9], "v_add_nc_u32", &[v], &[Opnd::Sgpr(high), v]) || !is(&w[11], "s_mov_b64", &[Opnd::ExecLo], &[s]) {
        return None;
    }
    let end = a + 12;
    if !(dead_from(insns, end, Reg::V(acc)) && dead_from(insns, end, Reg::V(tmp))
        && (high == sum || dead_from(insns, end, Reg::S(high)))) {
        return None;
    }
    insns[a + 2..a + 10].iter_mut().for_each(nop);
    let r = &mut insns[a + 10];
    r.mnemonic = TOTAL.into();
    (r.enc, r.op) = (Enc::Sop1, PSEUDO_OP);
    r.dst = vec![Opnd::Sgpr(sum)];
    r.src = vec![v];
    Some(())
}

// Inclusive DPP16 row_shr scan, cross-row and cross-half carries, minus the input.
fn prefix(insns: &mut [Insn], a: usize) -> Option<()> {
    let w = insns.get(a..a + 19)?;
    let Opnd::Sgpr(saved) = *w[0].dst.first()? else { return None };
    let Opnd::Vgpr(x) = *w[1].dst.first()? else { return None };
    let Opnd::Vgpr(acc) = *w[2].dst.first()? else { return None };
    let Opnd::Vgpr(tmp) = *w[9].dst.first()? else { return None };
    let (v, s, ones) = (Opnd::Vgpr(acc), Opnd::Sgpr(saved), Opnd::ConstInt(-1));
    let upper_rows = Opnd::Literal(0xffff0000);
    if !is(&w[0], "s_or_saveexec_b64", &[s], &[ones])
        || !matches!(w[1].src[..], [Opnd::ConstInt(0), Opnd::Vgpr(_), m] if m == s) || w[1].mnemonic != "v_cndmask_b32"
        || !is(&w[2], "v_mov_b32", &[v], &[Opnd::Vgpr(x)]) || acc == x
        || ![0xff0911, 0xff0112, 0xff0114, 0xff0118].iter().enumerate().all(|(i, &c)| dpp(&w[3 + i], acc, 250, c))
        || !is(&w[7], "s_mov_b32", &[Opnd::ExecLo], &[upper_rows]) || !is(&w[8], "s_mov_b32", &[Opnd::ExecHi], &[upper_rows])
        || !is(&w[9], "v_permlanex16_b32", &[Opnd::Vgpr(tmp)], &[v, ones, ones])
        || !is(&w[10], "v_add_nc_u32", &[v], &[Opnd::Vgpr(tmp), v])
        || !is(&w[12], "s_mov_b32", &[Opnd::ExecLo], &[Opnd::ConstInt(0)]) || !is(&w[13], "s_mov_b32", &[Opnd::ExecHi], &[ones])
        || !is(&w[15], "s_mov_b32", &[Opnd::ExecLo], &[ones]) || !is(&w[16], "s_mov_b32", &[Opnd::ExecHi], &[ones])
        || !is(&w[17], "v_sub_nc_u32", &[v], &[v, Opnd::Vgpr(x)]) || !is(&w[18], "s_mov_b64", &[Opnd::ExecLo], &[s]) {
        return None;
    }
    let half = readlane(&w[11], acc, 31)?;
    if !is(&w[14], "v_add_nc_u32", &[v], &[Opnd::Sgpr(half), v]) {
        return None;
    }
    // Lanes 16-31 and 48-63 read lane 15 of the other row: FI and BC are set,
    // and both selectors are -1, so the source lane is always active.
    if !(dead_from(insns, a + 19, Reg::V(tmp)) && dead_from(insns, a + 19, Reg::S(half))) {
        return None;
    }
    insns[a + 2..a + 17].iter_mut().for_each(nop);
    let r = &mut insns[a + 17];
    r.mnemonic = PREFIX.into();
    (r.enc, r.op) = (Enc::Sop1, PSEUDO_OP);
    r.src = vec![Opnd::Vgpr(x)];
    Some(())
}

/// Rewrite every recognized sequence. Unrecognized ones stay for `wave_sum`.
pub(super) fn prepare(insns: &mut [Insn]) {
    for a in 0..insns.len() {
        if insns[a].mnemonic == "s_or_saveexec_b64" && total(insns, a).is_none() {
            let _ = prefix(insns, a);
        }
    }
}

pub(super) fn emit(em: &mut Emitter, ins: &Insn) -> Result<bool, String> {
    let m = ins.mnemonic.as_str();
    if m != TOTAL && m != PREFIX { return Ok(false); }
    let d = *ins.dst.first().ok_or("missing wave sum destination")?;
    let value = super::r(&ins.src[0])?;
    // Every host invocation in this region runs the guest's whole-wave mode;
    // inactive guest lanes contribute the zero selected before the sum.
    let t = em.tmp;
    em.tmp += 1;
    if m == TOTAL {
        em.line(&format!("uint t{t} = WaveActiveSum({value});"));
        em.ws(&d, &format!("t{t}"))?;
    } else {
        em.line(&format!("uint t{t} = WavePrefixSum({value});"));
        em.wv(&d, &format!("t{t}"))?;
    }
    Ok(true)
}
