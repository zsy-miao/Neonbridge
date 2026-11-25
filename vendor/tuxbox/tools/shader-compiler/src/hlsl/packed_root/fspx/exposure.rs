//! Descriptor uses in the audited single-pass exposure reduction. Arithmetic,
//! branches, quad reads and LDS instructions still go through the ISA emitter.
use super::*;
use crate::meta::{RootParam, RootRange, RootSig};

pub(super) fn bindings(
    ex: &ShaderExtract,
    insns: &[Insn],
    root: &RootSig,
    uav: &RootRange,
    srvs: &[RootRange],
    cbvs: &[RootParam],
) -> Result<PackedRoot, String> {
    let table = |r: &RootRange, ty, count| {
        r.range_type == ty
            && r.num == count
            && r.space == 0
            && r.base_register == 0
            && r.offset_in_table == u32::MAX
    };
    let resources = [(2, 0), (3, 0), (6, 0), (6, 1), (7, 2)];
    if root.version != 2
        || root.flags != 0
        || root.num_static_samplers != 0
        || root.params.iter().any(|p| p.visibility != 0)
        || !table(uav, 1, 3)
        || !matches!(srvs, [s] if table(s, 0, 1))
        || !matches!(
            cbvs,
            [RootParam {
                kind: RootParamKind::Descriptor {
                    kind: 2,
                    register: 0,
                    space: 0
                },
                ..
            }]
        )
        || ex.resources.len() != resources.len()
        || !ex.resources.iter().zip(resources).all(|(r, (ty, reg))| {
            r.res_type == ty && r.space == 0 && r.lower == reg && r.upper == reg
        })
    {
        return Err("unsupported FSPX exposure root layout".into());
    }
    // SGPRs are reused across branches. Restrict the site-specific descriptor
    // mapping and the global ticket fences to the original executable stream.
    if !matches_program(&ex.isa, 5076, 0xbe928344bb17e2ef) {
        return Err("unsupported FSPX exposure instruction layout".into());
    }
    let mut plan = PackedRoot {
        abi: RootAbi::RegisterWave64,
        ..PackedRoot::default()
    };
    let binding = |slot, uav| Binding { slot, uav };
    for (idx, ins) in insns.iter().enumerate() {
        match ins.offset {
            // All lanes still participate before the single ticket-taking thread is selected.
            0xcb4 => {
                plan.device_sync_before.insert(idx);
            }
            // Ticket and coarse texels must be visible before the last group reads them.
            0xd08 => {
                plan.device_sync_barriers.insert(idx);
            }
            _ => {}
        }
        match ins.enc {
            Enc::Smem => {
                let words = match ins.mnemonic.as_str() {
                    "s_buffer_load_dwordx4" if ins.offset == 0x38 => {
                        (0..4).map(|n| Some((0, n))).collect()
                    }
                    "s_buffer_load_dword" if ins.offset == 0x11c => vec![Some((0, 6))],
                    "s_load_dwordx4" => vec![None; 4],
                    "s_load_dwordx8" => vec![None; 8],
                    "s_load_dwordx16" => vec![None; 16],
                    _ => return Err("unexpected exposure scalar load".into()),
                };
                plan.loads.insert(idx, words);
            }
            Enc::Mimg => {
                let resource = match (ins.mnemonic.as_str(), ins.offset) {
                    ("image_load", ..0xd84) => binding(0, false),
                    ("image_load", _) | ("image_store", 0xca0) => binding(1, true),
                    ("image_atomic_add", 0xce8) | ("image_store", 0xd58) => binding(0, true),
                    _ => return Err("unexpected exposure image access".into()),
                };
                plan.images.insert(idx, (resource, 1, None));
            }
            Enc::Mubuf => {
                plan.buffers.insert(idx, (binding(2, true), 0));
            }
            _ => {}
        }
    }
    Ok(plan)
}
