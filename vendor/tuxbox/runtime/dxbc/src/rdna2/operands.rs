//! Operand fields from AMD's machine-readable RDNA 1/2 ISA, MicrocodeFormat
//! bitmaps, and the scalar selector table in the RDNA 2 ISA manual, §6.2.
//! The HLSL emitter consumes the encoded base register of each register group.

use super::{Enc, InsnAux, Opnd};

// The hardware uses a shared 9-bit namespace for a vector ALU source. Scalar
// instructions use its low 256 entries. Keep unmodeled hardware selectors intact.
fn source(selector: u32, literal: u32) -> Opnd {
    match selector {
        0..=105 => Opnd::Sgpr(selector as u8),
        106 => Opnd::VccLo,
        107 => Opnd::VccHi,
        124 => Opnd::M0,
        125 => Opnd::Null,
        126 => Opnd::ExecLo,
        127 => Opnd::ExecHi,
        128..=192 => Opnd::ConstInt(i64::from(selector) - 128),
        193..=208 => Opnd::ConstInt(192 - i64::from(selector)),
        240..=247 => {
            let magnitude = [0.5, 1.0, 2.0, 4.0][((selector - 240) / 2) as usize];
            Opnd::ConstF32(if selector & 1 == 0 {
                magnitude
            } else {
                -magnitude
            })
        }
        248 => Opnd::ConstF32(0.5 * std::f32::consts::FRAC_1_PI),
        253 => Opnd::Scc,
        255 => Opnd::Literal(literal),
        256..=511 => Opnd::Vgpr(selector as u8),
        other => Opnd::Raw(other as u16),
    }
}

fn vgpr(word: u32, shift: u32) -> Opnd {
    Opnd::Vgpr((word >> shift) as u8)
}

fn compact_valu(enc: Enc, first: u32, extension: u32, aux: &mut InsnAux) -> (Vec<Opnd>, Vec<Opnd>) {
    let selector = first & 0x1ff;
    let mut src0 = source(selector, extension);
    let mut src1 = vgpr(first, 9);
    let mut compare_dest = Opnd::VccLo;

    // SDWA supplies the effective first source and can select scalar instead
    // of vector banks for both sources. Preserve the full modifier for lowering.
    if selector == 249 {
        aux.sdwa = Some(extension);
        src0 = if extension & (1 << 23) != 0 {
            source(extension & 0xff, 0)
        } else {
            vgpr(extension, 0)
        };
        if extension & (1 << 31) != 0 {
            src1 = source((first >> 9) & 0xff, 0);
        }
        if extension & (1 << 15) != 0 {
            compare_dest = source((extension >> 8) & 0x7f, 0);
        }
    }
    let dest = (first >> 17) & 0xff;
    match enc {
        Enc::Vopc => (vec![compare_dest], vec![src0, src1]),
        Enc::Vop1 => {
            let read_first_lane = (first >> 9) & 0xff == 2;
            let dest = if read_first_lane {
                source(dest, 0)
            } else {
                Opnd::Vgpr(dest as u8)
            };
            (vec![dest], vec![src0])
        }
        _ => (vec![Opnd::Vgpr(dest as u8)], vec![src0, src1]),
    }
}

/// Decode destination and source register groups and the addressing/modifier
/// fields used by the translator. The caller supplies words in instruction order.
pub(super) fn decode_operands(enc: Enc, raw: &[u32]) -> (Vec<Opnd>, Vec<Opnd>, InsnAux) {
    let mut aux = InsnAux::default();
    let Some(&first) = raw.first() else {
        return (Vec::new(), Vec::new(), aux);
    };
    let second = raw.get(1).copied().unwrap_or(0);
    let scalar_dest = || source((first >> 16) & 0x7f, 0);
    let scalar_pair = || {
        vec![
            source(first & 0xff, second),
            source((first >> 8) & 0xff, second),
        ]
    };
    let (dest, src) = match enc {
        Enc::Sop1 => (vec![scalar_dest()], vec![source(first & 0xff, second)]),
        Enc::Sop2 => (vec![scalar_dest()], scalar_pair()),
        Enc::Sopc => (vec![Opnd::Scc], scalar_pair()),
        Enc::Sopk => {
            let opcode = (first >> 23) & 31;
            aux.imm = if (9..=14).contains(&opcode) {
                i64::from(first as u16) // s_cmpk_*_u32 zero-extends SIMM16.
            } else {
                i64::from(first as i16)
            };
            (vec![scalar_dest()], Vec::new())
        }
        Enc::Sopp => {
            aux.imm = i64::from(first as i16);
            (Vec::new(), Vec::new())
        }
        Enc::Smem => {
            aux.imm = i64::from(second & 0x1f_ffff);
            (
                vec![source((first >> 6) & 127, 0)],
                vec![Opnd::Sgpr(((first & 63) * 2) as u8)],
            )
        }
        Enc::Vop1 | Enc::Vop2 | Enc::Vopc => compact_valu(enc, first, second, &mut aux),
        Enc::Vop3 => {
            // Compare opcodes in the ordinary VOP3 bank write a scalar mask.
            // Packed instructions have a separate prefix. Their lane selects
            // remain available in the raw words to the packed arithmetic emitter.
            let scalar_compare = first >> 26 == 0x35 && (first >> 16) & 0x3ff < 256;
            let dest = if scalar_compare {
                source(first & 255, 0)
            } else {
                vgpr(first, 0)
            };
            aux.mask = (((first >> 8) & 7) | ((second >> 29) << 3)) as u8;
            aux.imm = i64::from((first >> 15) & 1);
            aux.imm2 = i64::from((second >> 27) & 3);
            let literal = raw.get(2).copied().unwrap_or(0);
            let inputs = [0, 9, 18].map(|shift| source((second >> shift) & 511, literal));
            (vec![dest], inputs.to_vec())
        }
        Enc::Vinterp => {
            aux.imm = i64::from((first >> 10) & 63);
            aux.imm2 = i64::from((first >> 8) & 3);
            (vec![vgpr(first, 18)], vec![vgpr(first, 0)])
        }
        Enc::Ds => {
            aux.imm = i64::from(first as u16);
            (
                vec![vgpr(second, 24)],
                vec![vgpr(second, 0), vgpr(second, 8), vgpr(second, 16)],
            )
        }
        Enc::Mubuf => {
            aux.imm = i64::from(first & 0xfff);
            aux.mask = ((first >> 12) & 7) as u8; // OFFEN, IDXEN, GLC.
            let resource = Opnd::Sgpr((((second >> 16) & 31) * 4) as u8);
            (
                vec![vgpr(second, 8)],
                vec![vgpr(second, 0), resource, source(second >> 24, 0)],
            )
        }
        Enc::Mtbuf => {
            aux.imm = i64::from(first & 0xfff);
            aux.imm2 = i64::from((first >> 19) & 0x7f);
            aux.mask = ((first >> 12) & 3) as u8; // OFFEN, IDXEN.
            let resource = Opnd::Sgpr((((second >> 16) & 31) * 4) as u8);
            (
                vec![vgpr(second, 8)],
                vec![vgpr(second, 0), resource, source(second >> 24, 0)],
            )
        }
        Enc::Mimg => {
            let extension_words = ((first >> 1) & 3) as usize;
            aux.imm = i64::from((first >> 3) & 7); // DIM.
            aux.mask = ((first >> 8) & 15) as u8; // DMASK.
            aux.imm2 = extension_words as i64;

            let opcode = super::encoding::opcode_field(enc, first, Some(second));
            if matches!(opcode, 230 | 231) {
                // RDNA 2 table 48. BVH64 adds the high node-pointer word.
                // A16 packs the six direction and reciprocal components in three VGPRs.
                let count = 11 + usize::from(opcode == 231) - 3 * ((second >> 30) & 1) as usize;
                let mut inputs = Vec::with_capacity(count + 1);
                let first_address = second as u8;
                if extension_words == 0 {
                    inputs.extend(
                        (0..count as u8)
                            .map_while(|i| first_address.checked_add(i))
                            .map(Opnd::Vgpr),
                    );
                } else {
                    inputs.push(Opnd::Vgpr(first_address));
                    inputs.extend(
                        raw.iter()
                            .skip(2)
                            .flat_map(|word| word.to_le_bytes())
                            .take(count - 1)
                            .map(Opnd::Vgpr),
                    );
                }
                // SRSRC names four descriptor SGPRs. SSAMP and any unused
                // bytes in the last NSA word are padding, not live inputs.
                inputs.push(Opnd::Sgpr((((second >> 16) & 31) * 4) as u8));
                return (vec![vgpr(second, 8)], inputs, aux);
            }
            let mut inputs = Vec::with_capacity(3 + extension_words * 4);
            inputs.push(vgpr(second, 0));
            // NSA addresses are individually named VGPRs packed bytewise.
            for index in 0..extension_words {
                let addresses = raw.get(2 + index).copied().unwrap_or(0);
                inputs.extend(addresses.to_le_bytes().map(Opnd::Vgpr));
            }
            for shift in [16, 21] {
                inputs.push(Opnd::Sgpr((((second >> shift) & 31) * 4) as u8));
            }
            (vec![vgpr(second, 8)], inputs)
        }
        Enc::Flat => {
            // OFFSET is a 12-bit signed displacement. SEG selects flat,
            // scratch or global addressing.
            aux.imm = i64::from((first << 20) as i32 >> 20);
            aux.mask = ((first >> 14) & 3) as u8;
            let address = (second >> 16) & 127;
            // SADDR accepts both the NULL selector and 0x7f for vector-only addressing.
            let scalar = if address == 127 {
                Opnd::Null
            } else {
                source(address, 0)
            };
            (
                vec![vgpr(second, 24)],
                vec![vgpr(second, 0), vgpr(second, 8), scalar],
            )
        }
        Enc::Exp => {
            aux.mask = (first & 15) as u8;
            aux.imm = i64::from((first >> 4) & 63);
            aux.imm2 = i64::from((first >> 10) & 7); // COMPR, DONE, VM.
            (Vec::new(), second.to_le_bytes().map(Opnd::Vgpr).to_vec())
        }
        Enc::Unknown => (Vec::new(), Vec::new()),
    };
    (dest, src, aux)
}

#[cfg(test)]
mod tests {
    use super::*;

    // LLVM llvm-mc -triple=amdgcn -mcpu=gfx1030 -show-encoding.
    #[test]
    fn decodes_scalar_vector_and_literal_sources() {
        // s_add_u32 s3, 0xdeadbeef, s1
        let (dst, src, _) = decode_operands(Enc::Sop2, &[0x8003_01ff, 0xdead_beef]);
        assert_eq!(dst, [Opnd::Sgpr(3)]);
        assert_eq!(src, [Opnd::Literal(0xdead_beef), Opnd::Sgpr(1)]);
        // v_fma_f32 v0, 0x12345678, v1, v2
        let (_, src, _) = decode_operands(Enc::Vop3, &[0xd54b_0000, 0x040a_02ff, 0x1234_5678]);
        assert_eq!(
            src,
            [Opnd::Literal(0x1234_5678), Opnd::Vgpr(1), Opnd::Vgpr(2)]
        );
        // v_mov_b32_e32 v1, v255
        let (_, src, _) = decode_operands(Enc::Vop1, &[0x7e02_03ff]);
        assert_eq!(src, [Opnd::Vgpr(255)]);
    }

    #[test]
    fn preserves_sdwa_selector_and_address_order() {
        // v_mov_b32_sdwa v4, v5 dst_sel:DWORD dst_unused:UNUSED_PRESERVE src0_sel:BYTE_0
        let (dst, src, aux) = decode_operands(Enc::Vop1, &[0x7e08_02f9, 0x0000_1605]);
        assert_eq!(dst, [Opnd::Vgpr(4)]);
        assert_eq!(src, [Opnd::Vgpr(5)]);
        assert_eq!(aux.sdwa, Some(0x1605));
        // image_sample_d v0, [v31,v30,...,v23], s[0:7], s[8:11] dmask:1 dim:3D
        let (_, src, aux) = decode_operands(
            Enc::Mimg,
            &[0xf088_0114, 0x0040_001f, 0x1b1c_1d1e, 0x1718_191a],
        );
        let expected: Vec<_> = (23..=31)
            .rev()
            .map(Opnd::Vgpr)
            .chain([Opnd::Sgpr(0), Opnd::Sgpr(8)])
            .collect();
        assert_eq!(src, expected);
        assert_eq!((aux.imm, aux.mask, aux.imm2), (2, 1, 2));
    }

    #[test]
    fn bvh_sources_include_only_ray_inputs_and_descriptor() {
        // First non-sequential BVH64 in the retail direct-light shader. Its
        // destination overlaps the pointer and two inverse-direction inputs.
        let (dst, src, _) = decode_operands(
            Enc::Mimg,
            &[
                0xf19c_9f07,
                0x0011_3131,
                0x2322_1732,
                0x2826_2524,
                0xff35_3433,
            ],
        );
        let expected: Vec<_> = [49, 50, 23, 34, 35, 36, 37, 38, 40, 51, 52, 53]
            .map(Opnd::Vgpr)
            .into_iter()
            .chain([Opnd::Sgpr(68)])
            .collect();
        assert_eq!(dst, [Opnd::Vgpr(49)]);
        assert_eq!(src, expected); // the final NSA byte is deliberately nonzero

        for (opcode, a16, count) in [
            (230, false, 11),
            (231, false, 12),
            (230, true, 8),
            (231, true, 9),
        ] {
            // Contiguous form starting at v17. A16 only packs direction
            // and reciprocal components; origin and extent remain float32.
            let first = 0xf000_9f01 | ((opcode & 127) << 18);
            let second = (u32::from(a16) << 30) | 0x0003_0211;
            let (_, src, _) = decode_operands(Enc::Mimg, &[first, second]);
            let expected: Vec<_> = (17..17 + count)
                .map(Opnd::Vgpr)
                .chain([Opnd::Sgpr(12)])
                .collect();
            assert_eq!(src, expected);
        }
    }

    #[test]
    fn preserves_signed_global_offsets_and_vector_only_addressing() {
        // global_load_dword v3, v[15:16], off offset:-8; both encodings of off.
        for second in [0x037d_000f, 0x037f_000f] {
            let (dst, src, aux) = decode_operands(Enc::Flat, &[0xdc30_8ff8, second]);
            assert_eq!(dst, [Opnd::Vgpr(3)]);
            assert_eq!(src, [Opnd::Vgpr(15), Opnd::Vgpr(0), Opnd::Null]);
            assert_eq!((aux.imm, aux.mask), (-8, 2));
        }
    }

    #[test]
    fn decodes_typed_buffer_format_with_byte_offset() {
        // tbuffer_load_format_xyzw v[9:12], v2, s[0:3], 0
        // format:[BUF_FMT_8_8_8_8_UNORM] offen
        let (dst, src, aux) = decode_operands(Enc::Mtbuf, &[0xe9c3_1000, 0x8000_0902]);
        assert_eq!(dst, [Opnd::Vgpr(9)]);
        assert_eq!(src, [Opnd::Vgpr(2), Opnd::Sgpr(0), Opnd::ConstInt(0)]);
        assert_eq!((aux.imm, aux.mask, aux.imm2), (0, 1, 56));
    }

    #[test]
    fn decodes_export_and_interpolation_controls() {
        // exp mrt0, v1, v2, v3, v4 done
        let (dst, src, aux) = decode_operands(Enc::Exp, &[0xf800_080f, 0x0403_0201]);
        assert!(dst.is_empty());
        assert_eq!(
            src,
            [Opnd::Vgpr(1), Opnd::Vgpr(2), Opnd::Vgpr(3), Opnd::Vgpr(4)]
        );
        assert_eq!((aux.imm, aux.mask, aux.imm2), (0, 15, 2));
        // v_interp_p2_f32_e32 v1, v2, attr3.y
        let (dst, src, aux) = decode_operands(Enc::Vinterp, &[0xc805_0d02]);
        assert_eq!(
            (dst.as_slice(), src.as_slice()),
            (&[Opnd::Vgpr(1)][..], &[Opnd::Vgpr(2)][..])
        );
        assert_eq!((aux.imm, aux.imm2), (3, 1));
    }
}
