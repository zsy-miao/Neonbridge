use super::*;

fn decode(words: &[u32]) -> Insn {
    let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let instruction = instructions(&code).next().unwrap();
    instruction
}

#[test]
fn mul24_folds_signed_overflow_and_preserves_adjacent_bytes() {
    let mut regs = Values::new();
    regs.words[129] = Some(0xab80_0000); // -2^23, or +2^23 unsigned
    regs.words[133] = Some(257);
    regs.words[132] = Some(0xaabb_ccdd);
    for (op, ext, expected) in [
        (0xd509_8004, 0x0002_0b01, 0x8000_0000), // signed saturation
        (0xd50b_8004, 0x0002_0b01, 0x8080_0000), // unsigned fits
        (0x1408_0af9, 0x0606_1101, 0xaabb_ffdd), // signed high word
        (0x1808_0af9, 0x0606_1101, 0xaabb_00dd), // unsigned high word
    ] {
        assert_eq!(regs.alu(&decode(&[op, ext]))[0], Some(expected));
    }
    // 65536 squared is the first value above the unsigned dword range.
    regs.words[129] = Some(65536);
    regs.words[133] = Some(65536);
    assert_eq!(regs.alu(&decode(&[0xd50b_8004, 0x0002_0b01]))[0], Some(u32::MAX));
    assert_eq!(regs.alu(&decode(&[0xd50b_0004, 0x0002_0b01]))[0], Some(0));
}

#[test]
fn sdwa_minmax_folds_using_the_instruction_signedness() {
    let mut regs = Values::new();
    regs.words[129] = Some(0x8000_00fe);
    regs.words[133] = Some(0x7fff_ff01);
    regs.words[132] = Some(0xaabb_ccdd);
    for (opcode, expected) in [
        (0x2208_0af9, 0xaabb_fedd), // signed minimum
        (0x2408_0af9, 0xaabb_01dd), // signed maximum
        (0x2608_0af9, 0xaabb_01dd), // unsigned minimum
        (0x2808_0af9, 0xaabb_fedd), // unsigned maximum
    ] {
        assert_eq!(regs.alu(&decode(&[opcode, 0x0606_1101]))[0], Some(expected));
    }
}

#[test]
fn sdwa_right_shift_preserves_the_material_descriptor_bank() {
    let mut regs = Values::new();
    let mut layout = Layout::default();
    regs.words[129] = Some(33);
    regs.words[133] = Some(0x8000_0040);
    regs.words[132] = Some(SRV);
    // v_lshrrev_b32 v4,v1,v5: replace only BYTE_0 of the SRV address.
    let shift = decode(&[0x2c08_0af9, 0x0606_1001]);
    layout.instruction(&shift, &mut regs).unwrap();
    assert_eq!(regs.words[132], Some(SRV + 32));
    let address = regs.words[132].unwrap();
    assert_eq!(layout.resource(address, SRV, View::Structured).as_deref(), Some("tbSrv1"));
    regs.words[132] = None;
    layout.instruction(&shift, &mut regs).unwrap();
    assert_eq!(regs.words[132], None);
}

#[test]
fn sdwa_add_does_not_carry_pointer_provenance_through_a_partial_write() {
    let mut layout = Layout::default();
    let mut regs = Values::new();
    regs.code_address[129] = true;
    let add = |ext| decode(&[0x4a08_0af9, ext]);
    layout.instruction(&add(0x0606_0601), &mut regs).unwrap();
    assert!(regs.code_address[132]);
    layout.instruction(&add(0x0606_1101), &mut regs).unwrap();
    assert!(!regs.code_address[132]);
}

#[test]
fn flat_three_word_read_preserves_adjacent_live_descriptor() {
    let mut regs = Values::new();
    let mut layout = Layout::default();
    // global_load_dwordx3 v[51:53], v[51:52], off (triangle vertices).
    let load = decode(&[3694952448, 863830067]);
    regs.words[128 + 51] = None;
    regs.words[128 + 52] = Some(2);
    regs.words[128 + 54] = Some(SRV + 32);
    layout.instruction(&load, &mut regs).unwrap();
    assert_eq!(regs.words[128 + 54], Some(SRV + 32));
    assert!(layout.emitted[&0].contains("tbVertices.Load(at + 8u)"));
}

#[test]
fn recovers_traversal_address_after_private_vgpr_spill() {
    let mut regs = Values::new();
    let mut layout = Layout::default();
    // Material shaders spill the traversal address in v16 while shading
    // and reload it for a second PrimitiveIndex lookup.
    let store = decode(&[3765436464, 83890176]);
    let load = decode(&[3761242160, 83890176]);
    layout.instruction(&store, &mut regs).unwrap();
    regs.words[128 + 16] = None;
    layout.instruction(&load, &mut regs).unwrap();
    assert_eq!(regs.words[128 + 16], Some(BVH_BASE >> 8));
}

#[test]
fn separate_slot_types_share_numbers_in_different_tables() {
    let mut layout = Layout::default();
    assert_eq!(
        layout.resource(SRV + 64, SRV, View::Structured).as_deref(),
        Some("tbSrv2")
    );
    assert_eq!(
        layout.resource(CBV + 64, CBV, View::Constant).as_deref(),
        Some("tbCb2")
    );
    assert!(layout.resource(SRV + 64, SRV, View::Texture).is_none());
    assert!(
        layout
            .resource(SAMPLER + 3, SAMPLER, View::Sampler)
            .is_none()
    );
}
