use super::*;

#[test]
fn decodes_fetch_call_registers_and_gfx1013_multiply_add() {
    // Assembled with llvm-mc -mcpu=gfx1013. Material shaders in Frosty
    // use both kinds of instructions. gfx1030 only accepts the first one.
    let words = [
        0xbe8c_2104u32, // s_swappc_b64 s[12:13], s[4:5]
        0xd541_0004,
        0x0415_e907, // v_mad_f32 v4, v7, 2.0, v5
    ];
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let stream = disassemble(&bytes);
    let [call, mad] = stream.as_slice() else {
        panic!("expected a fetch call and a multiply-add");
    };
    assert_eq!(call.mnemonic, "s_swappc_b64");
    assert_eq!(call.dst, [Opnd::Sgpr(12)]);
    assert_eq!(call.src, [Opnd::Sgpr(4)]);
    assert_eq!(mad.mnemonic, "v_mad_f32");
    assert_eq!(mad.dst, [Opnd::Vgpr(4)]);
    assert_eq!(mad.src, [Opnd::Vgpr(7), Opnd::ConstF32(2.0), Opnd::Vgpr(5)]);
}

#[test]
fn walks_mixed_length_llvm_gfx1030_instructions_in_order() {
    // llvm-mc: s_mov_b32 s2, 0xabcd0123; v_interp_p2_f32 v1, v2, attr3.y;
    // exp mrt0, v1, v2, v3, v4 done; s_endpgm.
    let words = [
        0xbe82_03ffu32,
        0xabcd_0123,
        0xc805_0d02,
        0xf800_080f,
        0x0403_0201,
        0xbf81_0000,
    ];
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let stream = disassemble(&bytes);
    assert!(stream.iter().all(Insn::is_complete));
    let operations: Vec<_> = stream
        .iter()
        .map(|i| (i.offset, i.dwords, i.mnemonic.as_str()))
        .collect();
    assert_eq!(
        operations,
        [
            (0, 2, "s_mov_b32"),
            (8, 1, "v_interp_p2_f32"),
            (12, 2, "exp"),
            (20, 1, "s_endpgm")
        ]
    );
    assert_eq!(stream[0].dst, [Opnd::Sgpr(2)]);
    assert_eq!(stream[0].src, [Opnd::Literal(0xabcd_0123)]);
    assert_eq!(stream[1].aux.imm, 3);
    assert_eq!(stream[2].src[3], Opnd::Vgpr(4));
}

#[test]
fn final_short_instruction_only_consumes_available_whole_words() {
    let bytes = [0x00, 0, 0, 0xf4, 0xaa, 0xbb];
    let ins = disassemble(&bytes);
    assert_eq!(ins.len(), 1);
    assert_eq!(ins[0].enc, Enc::Smem);
    assert_eq!(ins[0].dwords, 1);
    assert_eq!(ins[0].raw, [0xf400_0000]);
    assert!(!ins[0].is_complete());
    assert_eq!(instructions(&bytes[4..]).count(), 0);
}

#[test]
fn checks_literal_length_encoded_in_second_dword() {
    let words = [0xd54b_0000u32, 255 << 9, 0x1234_5678];
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    for n in 4..bytes.len() {
        assert!(!instructions(&bytes[..n]).next().unwrap().is_complete());
    }
    assert!(instructions(&bytes).next().unwrap().is_complete());
}
