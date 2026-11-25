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
