//! Reaching vector byte-offset proofs for directly indexed descriptor heaps.
//! Offsets are alignment facts, not proof of an in-bounds descriptor index.
use super::{Enc, Insn, Opnd, Sym, Emitter};
use std::collections::HashMap;

fn offset(sym: &HashMap<u16, Sym>, o: &Opnd) -> Option<Sym> {
    let Opnd::Vgpr(v) = o else { return None };
    sym.get(&(2000 + u16::from(*v))).copied()
        .filter(|s| matches!(s, Sym::HeapOffset | Sym::HeapSamplerOffset))
}

fn result_words(i: &Insn) -> u16 {
    let m = i.mnemonic.as_str();
    match i.enc {
        Enc::Mimg => {
            if m.starts_with("image_store") && i.raw.first().is_some_and(|w| w & (3 << 16) == 0) { return 0; }
            // Gathers return four components regardless of DMASK. Use an
            // upper bound for D16, atomics and other special image results.
            let count = if m.starts_with("image_gather") { 4 }
                else if m.contains("atomic") { 2 }
                else { i.aux.mask.count_ones().max(1) as u16 };
            count + u16::from(i.raw.first().is_some_and(|w| w & (3 << 16) != 0))
        }
        Enc::Mubuf | Enc::Mtbuf => {
            if m.starts_with("buffer_store") || m.starts_with("tbuffer_store") { return 0; }
            if matches!(m, "buffer_gl0_inv" | "buffer_gl1_inv") { return 0; }
            Emitter::mubuf_dwords(m).map(|n| n as u16).unwrap_or(4)
                + u16::from(i.raw.get(1).is_some_and(|w| w & (1 << 23) != 0))
        }
        // A conservative bound for the supported LDS/flat data results.
        Enc::Ds if m.starts_with("ds_write") => 0,
        Enc::Flat if m.contains("_store") => 0,
        Enc::Ds | Enc::Flat => 4,
        Enc::Vop1 | Enc::Vop2 | Enc::Vop3 => {
            if super::float64::pair_result(m) || m.ends_with("_f64")
                || m.ends_with("_b64") || m.ends_with("_u64") || m.ends_with("_i64")
                || matches!(m,"v_mad_u64_u32" | "v_mad_i64_i32" | "v_div_scale_f64")
            { 2 } else { 1 }
        }
        _ => 1,
    }
}

pub(super) fn transfer(i: &Insn, sym: &mut HashMap<u16, Sym>, resource: bool, pixel: bool) -> Option<Sym> {
    if !(resource || pixel) || i.mnemonic == "v_nop" { return None; }
    // Only a full ordinary vector write can establish a new alignment fact.
    // DPP/SDWA, truncated instructions and partial writes are not copies.
    let value = if i.is_complete() && i.aux.sdwa.is_none()
        && i.src.iter().all(|o| !matches!(o, Opnd::Raw(_)))
        && matches!(i.dst.first(), Some(Opnd::Vgpr(_)))
        && matches!(i.enc, Enc::Vop1 | Enc::Vop2 | Enc::Vop3)
        && (i.enc != Enc::Vop3 || i.aux.mask == 0 && i.aux.imm == 0 && i.aux.imm2 == 0)
    {
        match i.mnemonic.as_str() {
            "v_lshlrev_b32" if resource && i.src.first() == Some(&Opnd::ConstInt(5)) => Some(Sym::HeapOffset),
            "v_lshlrev_b32" if pixel && i.src.first() == Some(&Opnd::ConstInt(4)) => Some(Sym::HeapSamplerOffset),
            "v_mov_b32" => i.src.first().and_then(|o| offset(sym, o)),
            "v_cndmask_b32" => {
                let a = i.src.first().and_then(|o| offset(sym, o));
                let b = i.src.get(1).and_then(|o| offset(sym, o));
                if a == b { a } else { None }
            }
            _ => None,
        }
    } else { None };
    // A decoded READLANE destination is scalar, not a vector write.
    if i.mnemonic != "v_readlane_b32" {
        let words = result_words(i);
        for dst in &i.dst {
            if let Opnd::Vgpr(v) = dst {
                let start = u16::from(*v);
                for n in start..(start + words).min(256) {
                    let key = 2000 + n;
                    if matches!(sym.get(&key), Some(Sym::HeapOffset | Sym::HeapSamplerOffset)) {
                        sym.remove(&key);
                    }
                }
            }
        }
    }
    value
}
