//! Register bindings of the PS09 tagged-depth pass. The probe verifies the
//! original PSO/binding records and supplies a compact desktop root signature.
use super::*;
use crate::meta::RootSig;

pub(super) fn bindings(
    ex: &ShaderExtract,
    root: &RootSig,
    insns: &[Insn],
) -> Result<Option<PackedRoot>, String> {
    if !matches_program(&ex.isa, 244, 0x2245d7f914038cd8) {
        return Ok(None);
    }
    let [raw, tex, tag] = root.params.as_slice() else {
        return Err("unsupported PS09 depth root parameters".into());
    };
    let table = |param: &crate::meta::RootParam, reg, offset| {
        let RootParamKind::Table(ranges) = &param.kind else {
            return false;
        };
        let [range] = ranges.as_slice() else {
            return false;
        };
        range.range_type == 0
            && range.num == 1
            && range.space == 0
            && range.base_register == reg
            && range.offset_in_table == offset
    };
    if ex.ps09_pixel_inputs != Some(0x8080)
        || !table(raw, 0, 21)
        || !table(tex, 1, 0)
        || root.params.iter().any(|p| !matches!(p.visibility, 0 | 5))
        || root.num_static_samplers != 0
        || !matches!(
            tag.kind,
            RootParamKind::Constants {
                register: 0,
                space: 0,
                num32: 1
            }
        )
        || !matches!(ex.input_sig.as_slice(), [p] if p.sysval == 1)
        || !matches!(ex.output_sig.as_slice(), [d] if d.sysval == 65)
    {
        return Err("unsupported PS09 depth binding contract".into());
    }
    // Table s8 holds the buffer descriptor at 0x2a0. Table s9 holds the
    // image descriptor at zero. SGPR s10 carries the inline material tag.
    let mut plan = PackedRoot::default();
    plan.entry_constants.push((10, 0, 0));
    plan.uint_srvs.insert(1);
    for (idx, ins) in insns.iter().enumerate() {
        match (ins.enc, ins.offset) {
            (Enc::Smem, 0x14) => {
                plan.loads.insert(idx, vec![None; 8]);
            }
            (Enc::Smem, 0x54) => {
                plan.loads.insert(idx, vec![None; 4]);
            }
            (Enc::Mimg, 0x20) => {
                plan.images.insert(
                    idx,
                    (
                        Binding {
                            slot: 1,
                            uav: false,
                        },
                        1,
                        None,
                    ),
                );
            }
            (Enc::Mubuf, 0xa0) => {
                plan.buffers.insert(
                    idx,
                    (
                        Binding {
                            slot: 0,
                            uav: false,
                        },
                        0,
                    ),
                );
            }
            _ => {}
        }
    }
    Ok(Some(plan))
}
