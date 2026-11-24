//! Multi-view Nanite instance culling appends (instance flags, view ID)
//! after every packed view. A 64-thread group can span two host subgroups.
use super::{
    compute_group, fssm_bindings, packed_root::matches_program, Emitter, Enc, Insn, Opnd, Res,
    ShaderExtract, Stage,
};

#[derive(Clone, Copy)]
pub(super) struct ViewProgram {
    pub join: usize,
    pub back: usize,
    capture: usize,
    node: u8,
    capacity: u8,
    address: u8,
    hzb: bool,
    hidden_start: usize,
    checks: [usize; 3],
}

const SHADOW: ViewProgram = ViewProgram {
    join: 0x13c8,
    back: 0x13d4,
    capture: 0x10a0,
    node: 36,
    capacity: 13,
    address: 15,
    hzb: false,
    hidden_start: 0,
    checks: [0x4c4, 0x594, 0x1050],
};
const SHADOW_HZB: ViewProgram = ViewProgram {
    join: 0x1a80,
    back: 0x1a8c,
    capture: 0x1728,
    node: 37,
    capacity: 21,
    address: 79,
    hzb: true,
    hidden_start: 0xb98,
    checks: [0x4d8, 0x590, 0x16d0],
};

fn capture(op: &mut Insn, mnemonic: &str, sources: Vec<Opnd>) {
    op.mnemonic = mnemonic.into();
    op.enc = Enc::Vop1;
    op.dst.clear();
    op.src = sources;
}

pub(super) fn prepare(ex: &ShaderExtract, insns: &mut Vec<Insn>) -> Option<ViewProgram> {
    if ex.stage != Stage::Compute
        || ex.entry.as_deref() != Some("InstanceCull")
        || compute_group(ex) != [64, 1, 1]
    {
        return None;
    }
    let program = if matches_program(&ex.isa, 5084, 0xc892c2898fc4dfd6) {
        SHADOW
    } else if matches_program(&ex.isa, 6804, 0x974e2b1648c67f62) {
        SHADOW_HZB
    } else if matches_program(&ex.isa, 5164, 0xdacaa03f58169cce) {
        ViewProgram {
            join: 0x1418,
            back: 0x1424,
            capture: 0x10f0,
            node: 37,
            checks: [0x4d4, 0x5ac, 0x109c],
            ..SHADOW
        }
    } else if matches_program(&ex.isa, 6860, 0x8465f9ec4130847e) {
        ViewProgram {
            join: 0x1ab8,
            back: 0x1ac4,
            capture: 0x1760,
            node: 38,
            hidden_start: 0xba0,
            checks: [0x4d8, 0x598, 0x1704],
            ..SHADOW_HZB
        }
    } else {
        return None;
    };
    // Both queues are write-only during the view. Finish their atomics and
    // ordered writes at the one workgroup rendezvous before the next view.
    insns.retain(|op| {
        (op.offset <= program.capture || op.offset >= program.join)
            && (!program.hzb
                || !(program.hidden_start + 1..program.hidden_start + 0x1c8).contains(&op.offset))
    });
    for op in insns {
        if op.offset == program.capture {
            capture(
                op,
                "tb_view_candidate",
                vec![
                    Opnd::Vgpr(program.node),
                    Opnd::Sgpr(program.capacity),
                    Opnd::Sgpr(program.address),
                ],
            );
        } else if program.hzb && op.offset == program.hidden_start {
            capture(op, "tb_view_occluded", vec![Opnd::Vgpr(0)]);
        }
    }
    Some(program)
}

impl ViewProgram {
    pub fn resources(self, em: &mut Emitter<'_>) -> Result<(), String> {
        let bindings: &[(u32, u32)] = if self.hzb {
            &[
                (1, 0),
                (2, 0),
                (5, 0),
                (5, 1),
                (5, 2),
                (3, 3),
                (5, 4),
                (4, 5),
                (6, 0),
                (8, 1),
                (6, 2),
                (7, 3),
                (8, 4),
                (8, 5),
            ]
        } else {
            &[
                (2, 0),
                (5, 0),
                (5, 1),
                (5, 2),
                (5, 3),
                (4, 4),
                (6, 0),
                (7, 1),
                (8, 2),
                (8, 3),
            ]
        };
        if em.ex.resources.len() != bindings.len()
            || !bindings.iter().all(|&(kind, slot)| {
                em.ex
                    .resources
                    .iter()
                    .any(|r| (r.res_type, r.space, r.lower, r.upper) == (kind, 0, slot, slot))
            })
        {
            return Err(format!(
                "unsupported multi-view culling bindings {:?}",
                em.ex.resources
            ));
        }
        for (&(uav, slot), words) in &mut em.sbuf {
            *words = fssm_bindings::structured_words(em.ex, slot, uav)?;
        }
        let view = if self.hzb { 4 } else { 3 };
        for (offset, expected) in
            self.checks
                .into_iter()
                .zip([Res::Cb(0), Res::Buf(view, false), Res::Uav(0, 1)])
        {
            let index = em.insns.iter().position(|op| op.offset == offset);
            if index.and_then(|i| em.res.get(&i)).map(Vec::as_slice) != Some(&[expected][..]) {
                return Err(format!(
                    "unexpected multi-view culling descriptor at {offset:#x}"
                ));
            }
        }
        let atomic = em
            .insns
            .iter()
            .find(|op| op.offset == self.checks[2])
            .ok_or("missing instance visibility atomic")?;
        match atomic.raw.as_slice() {
            [0xf05c1108, _] => {}
            [0xf05c1308, _] => {
                em.wide_visibility_uavs.insert(0);
            }
            _ => return Err("unknown instance visibility atomic flags".into()),
        }
        Ok(())
    }

    pub fn declarations(self) -> String {
        if !self.hzb {
            return super::instance_cull::queue_declarations(1);
        }
        let decl = super::instance_cull::queue_declarations(3);
        decl + super::instance_cull_hzb::HIDDEN_DECLARATIONS
    }

    pub fn append(self, em: &mut Emitter<'_>) {
        if self.hzb {
            em.line(super::instance_cull_hzb::HIDDEN_APPEND);
            em.line("hidden = false;");
        }
        em.line(APPEND);
    }
}
pub(super) const APPEND: &str = r#"
    {
        uint lane = gtid.x;
        if (lane < 2u) cull_mask[lane] = 0u;
        GroupMemoryBarrierWithGroupSync();
        if (cull_candidate) InterlockedOr(cull_mask[lane >> 5], 1u << (lane & 31u));
        GroupMemoryBarrierWithGroupSync();
        uint lo = cull_mask[0], hi = cull_mask[1];
        if (lane == 0u && (lo | hi) != 0u) {
            uint count = countbits(lo) + countbits(hi);
            InterlockedAdd(cull_queue[0].nodes, count, cull_base);
            InterlockedAdd(cull_queue[0].candidates, count);
            if ((cb0[2].w & 1u) != 0u) InterlockedAdd(cull_stats[0].instances, count);
        }
        GroupMemoryBarrierWithGroupSync();
        if (cull_candidate) {
            uint rank = countbits(((lane < 32u) ? lo : hi) & ((1u << (lane & 31u)) - 1u));
            uint index = cull_base + rank + ((lane < 32u) ? 0u : countbits(lo));
            if (index < cull_record.y) cull_nodes.Store2(cull_record.z + index * 8u, uint2(cull_record.x, cull_view << 16));
        }
        GroupMemoryBarrierWithGroupSync();
        cull_candidate = false;
    }
"#;
