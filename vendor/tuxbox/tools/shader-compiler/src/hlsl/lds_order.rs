use super::{Emitter, Enc, Insn, Opnd};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
    Atomic,
}

/// A masked set of producers can publish LDS, restore EXEC, and let a
/// different set of lanes read the data. The host may split that guest
/// wave into two subgroups. Rendezvous outside the emulated branch guards,
/// even for host lanes that skip the guest store or its consumer.
pub(super) fn restored_write_waits(insns: &[Insn], loops: &[(usize, usize)]) -> BTreeSet<usize> {
    let mut sites = BTreeSet::new();
    let mut wrote = false;
    let mut restored = false;
    let mut wait = None;
    for (i, ins) in insns.iter().enumerate() {
        let name = ins.mnemonic.as_str();
        match name {
            "s_mov_b64" if ins.dst.first() == Some(&Opnd::ExecLo) => restored = wrote,
            "s_waitcnt" if restored && ins.aux.imm & 0x3f00 == 0 => {
                if !loops.iter().any(|&(head, back)| head <= i && i <= back) {
                    wait.get_or_insert(i);
                }
            }
            "s_barrier" | "s_endpgm" => {
                wrote = false;
                restored = false;
                wait = None;
            }
            _ if ins.enc == Enc::Ds => {
                let local = ins.raw[0] & (1 << 17) == 0;
                if local && name.starts_with("ds_read") {
                    sites.extend(wait);
                }
                wrote = local && name.starts_with("ds_write");
                restored = false;
                wait = None;
            }
            _ => {}
        }
    }
    sites
}

/// A guest wave can exchange LDS data using LGKM waits alone. Only insert host
/// workgroup barriers where every invocation reaches the wait. Forward branch
/// arms reconverge at their target. The number of loop iterations can vary.
pub(super) fn exchange_waits(insns: &[Insn], loops: &[(usize, usize)]) -> BTreeSet<usize> {
    let mut barriers = BTreeSet::new();
    let mut previous = None;
    let mut drained = None;
    let mut resume_at = 0;
    for (index, ins) in insns.iter().enumerate() {
        let name = ins.mnemonic.as_str();
        if name == "s_endpgm" {
            resume_at = i64::MAX;
        } else if ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some() {
            resume_at = resume_at.max(ins.offset as i64 + 4 + 4 * ins.aux.imm);
        }
        if loops
            .iter()
            .any(|&(head, back)| head <= index && index <= back)
        {
            previous = None;
            drained = None;
            continue;
        }
        if name == "s_waitcnt" && ins.aux.imm & 0x3f00 == 0 && ins.offset as i64 >= resume_at {
            drained.get_or_insert(index);
        }
        if name == "s_barrier" {
            previous = None;
            drained = None;
        }
        if ins.enc != Enc::Ds {
            continue;
        }
        let access = if ins.raw[0] & (1 << 17) != 0 {
            None // GDS has a different address space.
        } else if name.starts_with("ds_write") {
            Some(Access::Write)
        } else if name.starts_with("ds_read") {
            Some(Access::Read)
        } else if name == "ds_max_u32" {
            Some(Access::Atomic)
        } else {
            None
        };
        if access.is_some() && previous.is_some() && previous != access {
            barriers.extend(drained);
        }
        previous = access;
        drained = None;
    }
    barriers
}

#[cfg(test)]
#[path = "../tests/hlsl/lds_order.rs"]
mod tests;
