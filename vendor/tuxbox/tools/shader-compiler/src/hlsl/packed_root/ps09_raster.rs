//! The PS09 segment rasterizers share two descriptor tables, five inline words
//! and a two-word typed visibility UAV. Only the verified programs use this ABI.
use super::*;
use crate::meta::RootSig;

pub(super) fn bindings(
    ex: &ShaderExtract,
    root: &RootSig,
    insns: &[Insn],
) -> Result<Option<PackedRoot>, String> {
    if !(matches_program(&ex.isa, 3004, 0xf592a180ae458db7)
        || matches_program(&ex.isa, 2956, 0x28eadf1adfb088ed))
    {
        return Ok(None);
    }
    let [first, second, inline] = root.params.as_slice() else {
        return Err("unexpected PS09 raster root".into());
    };
    let table = |p: &crate::meta::RootParam, layout: &[(u32, u32, u32)]| {
        let RootParamKind::Table(ranges) = &p.kind else {
            return false;
        };
        ranges.len() == layout.len()
            && ranges.iter().zip(layout).all(|(r, &(ty, reg, offset))| {
                (
                    r.range_type,
                    r.base_register,
                    r.offset_in_table,
                    r.space,
                    r.num,
                ) == (ty, reg, offset, 0, 1)
            })
    };
    if compute_group(ex) != [64, 1, 1]
        || root.num_static_samplers != 0
        || root.params.iter().any(|p| p.visibility != 0)
        || !table(
            first,
            &[
                (0, 13, 13),
                (0, 73, 15),
                (0, 72, 18),
                (0, 71, 21),
                (0, 74, 29),
            ],
        )
        || !table(second, &[(1, 10, 0), (2, 15, 1), (2, 0, 2)])
        || !matches!(
            inline.kind,
            RootParamKind::Constants {
                register: 500,
                space: 0,
                num32: 5
            }
        )
    {
        return Err("unsupported PS09 raster descriptor layout".into());
    }
    let mut plan = PackedRoot {
        abi: RootAbi::Ps09Raster,
        ..Default::default()
    };
    plan.entry_constants.extend([(6, 500, 0), (9, 500, 3)]);
    // The s4 and s5 pointers are copied to s12:13 and s20:21 respectively.
    // Remember the logical binding of each subsequent descriptor fetch. The
    // exact-program guard is important because these registers are reused as
    // data elsewhere; this is not a general control-flow provenance analysis.
    let mut resources = BTreeMap::new();
    for (idx, i) in insns.iter().enumerate() {
        let n = |base: &str| -> Result<u8, String> {
            match i.mnemonic.strip_prefix(base) {
                Some("") => Ok(1),
                Some("x2") => Ok(2),
                Some("x4") => Ok(4),
                Some("x8") => Ok(8),
                Some("x16") => Ok(16),
                _ => Err("unexpected PS09 scalar load width".into()),
            }
        };
        match i.enc {
            Enc::Smem => {
                let [Opnd::Sgpr(base)] = i.src[..] else {
                    return Err("unexpected PS09 scalar address".into());
                };
                let [Opnd::Sgpr(dst)] = i.dst[..] else {
                    return Err("unexpected PS09 scalar destination".into());
                };
                let offset = i.aux.imm as u32;
                if i.mnemonic.starts_with("s_load_dword") {
                    let slot = match (base, offset) {
                        (12, 0x1a0) => 13,
                        (12, 0x1e0) => 73,
                        (12, 0x240) => 72,
                        (12, 0x2a0) => 71,
                        (12, 0x3a0) => 74,
                        (20, 0) => 10,
                        (20, 0x20) => 15,
                        (20, 0x40) => 0,
                        _ => return Err("unexpected PS09 raster table load".into()),
                    };
                    // The shader converts t73's descriptor to byte addressing.
                    // Its only stride computation is max(stride, 1). Metadata
                    // fixes the original stride at one; the backend supplies
                    // the raw SRV and enforces buffer bounds at the access.
                    resources.insert(dst, slot);
                    plan.loads
                        .insert(idx, vec![None; n("s_load_dword")? as usize]);
                } else {
                    let slot = *resources
                        .get(&base)
                        .ok_or("unresolved PS09 scalar buffer")?;
                    let count = n("s_buffer_load_dword")?;
                    let soffset = (i.raw[1] >> 25) as u8;
                    if matches!(slot, 0 | 15) && soffset == 125 {
                        plan.loads.insert(
                            idx,
                            (0..u32::from(count))
                                .map(|k| Some((slot, offset / 4 + k)))
                                .collect(),
                        );
                    } else if matches!(slot, 13 | 71 | 72) {
                        plan.raw_scalar
                            .insert(idx, (Binding { slot, uav: false }, offset, count));
                        if soffset != 125 {
                            plan.raw_offsets.insert(idx, soffset);
                        }
                    } else {
                        return Err("unexpected PS09 raster scalar buffer".into());
                    }
                }
            }
            Enc::Mubuf => {
                let [_, Opnd::Sgpr(base), _] = i.src[..] else {
                    return Err("unexpected PS09 vertex load".into());
                };
                if resources.get(&base) != Some(&73) {
                    return Err("unresolved PS09 vertex buffer".into());
                }
                plan.buffers.insert(
                    idx,
                    (
                        Binding {
                            slot: 73,
                            uav: false,
                        },
                        0,
                    ),
                );
            }
            Enc::Mimg => match resources.get(&40) {
                Some(74) => {
                    plan.images.insert(
                        idx,
                        (
                            Binding {
                                slot: 74,
                                uav: false,
                            },
                            1,
                            None,
                        ),
                    );
                }
                Some(10) => {
                    plan.wide_images.insert(idx);
                }
                _ => return Err("unresolved PS09 raster image".into()),
            },
            _ => {}
        }
    }
    Ok(Some(plan))
}

pub(in crate::hlsl) fn emit_wide_image(em: &mut Emitter<'_>, ins: &Insn, image: &str) -> Result<(), String> {
    // With DMASK=3, IMAGE_ATOMIC_UMAX consumes two VGPRs as one unsigned
    // 64-bit operand. The paired IMAGE_LOAD reads the same RG32_UINT texel.
    let [Opnd::Vgpr(d)] = ins.dst[..] else {
        return Err("invalid image data".into());
    };
    let Opnd::Vgpr(a) = ins.src[0] else {
        return Err("invalid image coordinates".into());
    };
    if !matches!(ins.raw[..], [0xf0001308 | 0xf05c1308, _]) {
        return Err("unsupported 64-bit image access flags".into());
    }
    for n in [a, a + 1, d, d + 1] {
        em.touch(&Opnd::Vgpr(n));
    }
    let uv = format!("uint2(v{a}, v{})", a + 1);
    let temp = em.tmp;
    em.tmp += 1;
    em.line("if (exec_lo & 1u) {");
    if ins.mnemonic == "image_load" {
        em.line(&format!("uint64_t val{temp} = {image}[{uv}];"));
        em.line(&format!(
            "v{d} = (uint)val{temp}; v{} = (uint)(val{temp} >> 32);",
            d + 1
        ));
    } else {
        em.line(&format!(
            "InterlockedMax({image}[{uv}], (uint64_t(v{}) << 32) | v{d});",
            d + 1
        ));
    }
    em.line("}");
    Ok(())
}
