//! Capture a physical wave32 ballot before the guest elects a single lane
//! to reserve space for every active lane in its feedback buffer.
use super::{operand, target};
use crate::rdna2::{Enc, Insn, Opnd};
use std::collections::BTreeMap;
use std::fmt::Write;

fn writes_sgpr(ins: &Insn, register: u8) -> bool {
    ins.dst.iter().any(|dst| {
        let Opnd::Sgpr(first) = *dst else { return false };
        // A descriptor load and a scalar pair both declare their first SGPR.
        let width = if ins.enc == Enc::Smem {
            ins.mnemonic.split_once("dwordx")
                .and_then(|(_, n)| n.parse::<u8>().ok()).unwrap_or(1)
        } else if ins.mnemonic.ends_with("64") { 2 } else { 1 };
        register.checked_sub(first).is_some_and(|delta| delta < width)
    })
}

pub(super) fn saved_exec_counts(code: &[Insn], declarations: &mut String) -> Result<BTreeMap<usize, String>, String> {
    let mut statements = BTreeMap::new();
    for (i, count) in code.iter().enumerate() {
        let [Opnd::Sgpr(src)] = count.src.as_slice() else { continue };
        if count.mnemonic != "s_bcnt1_i32_b32" { continue; }
        let Some(save) = code[..i].iter().rev().find(|ins| writes_sgpr(ins, *src)) else { continue };
        if save.mnemonic != "s_mov_b32" || save.src != [Opnd::ExecLo] { continue; }
        if code.iter().any(|edge| {
            target(edge).is_some_and(|dest| save.offset < dest && dest <= count.offset)
                && !(save.offset..count.offset).contains(&edge.offset)
        }) {
            return Err("ray EXEC save does not dominate wave count".into());
        }
        let mask = format!("ballotAt{}", save.offset);
        if let std::collections::btree_map::Entry::Vacant(entry) = statements.entry(save.offset) {
            writeln!(declarations, "uint {mask} = 0u;").unwrap();
            entry.insert(format!("s[{src}] = exec; {mask} = tb_ray_ballot(exec != 0u);"));
        }
        let dst = operand(&count.dst[0])?;
        statements.insert(count.offset, format!("{dst} = countbits({mask}); scc = ({dst} != 0u);"));
    }
    Ok(statements)
}
