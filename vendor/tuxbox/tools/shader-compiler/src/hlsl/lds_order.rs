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
mod tests {
    use super::*;
    use crate::rdna2::disassemble;

    #[test]
    fn publish_masked_writes_before_and_after_a_read_loop() {
        let words = [
            0xd8380100, 0x00020100, // ds_write2_b32
            0xbefe040a,             // s_mov_b64 exec, s[10:11]
            0xbf8cc07f,             // s_waitcnt lgkmcnt(0)
            0xd8dc0100, 0,          // loop: ds_read2_b32
            0xbefe040a, 0xbf8cc07f, // a drain inside the loop cannot rendezvous
            0xbf84fffb,             // s_cbranch_scc0 loop
            0xd8380100, 0x00020100,
            0xbefe040a, 0xbf8cc07f,
            0xd8dc0100, 0,
        ];
        let bytes = words.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
        let insns = disassemble(&bytes);
        let sites = restored_write_waits(&insns, &[(3, 6)]);
        assert_eq!(sites.into_iter().collect::<Vec<_>>(), [2, 9]);
    }

    #[test]
    fn order_lds_phases_at_convergent_draining_waits() {
        let write = [0xd8380100, 0x00020100]; // ds_write2_b32
        let read = [0xd8dc0100, 0x00000000]; // ds_read2_b32
        let drain = [0xbf8cc07f]; // lgkmcnt(0)
        let busy = [0xbf8cc17f]; // lgkmcnt(1)
        let branch = [0xbf880001]; // s_cbranch_execz
        let barrier = [0xbf8a0000];
        let atomic_max = [0xd8200000, 0x00000100];
        let nop = [0xbf800000]; // s_nop
        let sites_in = |ops: &[&[u32]], loops: &[(usize, usize)]| {
            let isa = ops
                .iter()
                .flat_map(|op| op.iter().flat_map(|w| w.to_le_bytes()))
                .collect::<Vec<_>>();
            exchange_waits(&disassemble(&isa), loops)
                .into_iter()
                .collect::<Vec<_>>()
        };
        let sites = |ops: &[&[u32]]| sites_in(ops, &[]);
        assert_eq!(
            sites_in(
                &[&write, &drain, &read, &read, &drain, &write, &write, &drain, &read],
                &[(3, 5)]
            ),
            [1, 7]
        );
        assert_eq!(
            sites(&[&write, &write, &drain, &read, &drain, &write, &drain, &branch, &read]),
            [2, 4, 6]
        );
        assert_eq!(sites(&[&branch, &nop, &write, &drain, &read]), [3]);
        assert_eq!(
            sites(&[&write, &drain, &atomic_max, &atomic_max, &drain, &read]),
            [1, 4]
        );
        assert!(sites(&[&write, &busy, &read]).is_empty());
        assert!(sites(&[&write, &branch, &drain, &read]).is_empty());
        assert!(sites(&[&write, &drain, &barrier, &read]).is_empty());
    }
}
