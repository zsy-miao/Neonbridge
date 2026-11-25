use super::*;
use crate::{ShaderExtract, Stage};
mod fixture {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/wave_guard_fixture.rs"));
}

fn insn(mnemonic: &str, enc: Enc, op: u32, dst: Vec<Opnd>, src: Vec<Opnd>) -> Insn {
    Insn {
        offset: 0,
        dwords: 1,
        enc,
        op,
        raw: vec![0],
        mnemonic: mnemonic.into(),
        dst,
        src,
        aux: Default::default(),
    }
}

fn nop() -> Insn {
    insn("s_nop", Enc::Sopp, 0, vec![], vec![])
}

fn branch(op: u32) -> Insn {
    insn("branch", Enc::Sopp, op, vec![], vec![])
}

fn compare(src: Vec<Opnd>) -> Insn {
    insn("s_cmp_eq_u32", Enc::Sopc, 6, vec![], src)
}

// Synthetic self-contained masked reduction: save EXEC, force the full
// mask, CNDMASK-neutralize inactive lanes, DPP shape, readlane, restore.
// Mirrors the shape lower() accepts but is only control-flow shaped
// enough for check_control_flow.
fn masked_reduction(width: u32, result: u8) -> Vec<Insn> {
    let save_mnemonic = if width == 32 {
        "s_or_saveexec_b32"
    } else {
        "s_or_saveexec_b64"
    };
    let and_mnemonic = if width == 32 { "s_and_b32" } else { "s_and_b64" };
    let mut ops = vec![
        insn(save_mnemonic, Enc::Sop1, 0, vec![Opnd::Sgpr(8)], vec![Opnd::ConstInt(-1)]),
        insn(and_mnemonic, Enc::Sop1, 0, vec![Opnd::Sgpr(9)], vec![Opnd::Sgpr(7), Opnd::Sgpr(8)]),
        insn("v_cndmask_b32", Enc::Vop2, 0, vec![Opnd::Vgpr(1)], vec![Opnd::ConstInt(0), Opnd::Vgpr(1), Opnd::Sgpr(9)]),
        insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
        insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
        insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
        insn("v_or_b32", Enc::Vop2, DPP_REDUCE, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]),
        insn("v_permlanex16_b32", Enc::Vop1, 0, vec![Opnd::Vgpr(2)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0), Opnd::ConstInt(0)]),
        insn("v_or_b32", Enc::Vop2, 0, vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(2), Opnd::Vgpr(1)]),
    ];
    for (index, step) in [(4, 1), (5, 2), (6, 3)] {
        ops[index].aux.imm = step;
    }
    if width == 32 {
        ops.push(insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                      vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0)]));
        ops.push(insn("s_mov_b32", Enc::Sop1, 0, vec![Opnd::ExecLo], vec![Opnd::Sgpr(8)]));
    } else {
        ops.push(insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                      vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(32)]));
        ops.push(insn("v_or_b32", Enc::Vop2, 0, vec![Opnd::Vgpr(1)], vec![Opnd::Sgpr(result), Opnd::Vgpr(1)]));
        ops.push(insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                      vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0)]));
        ops.push(insn("s_mov_b64", Enc::Sop1, 0, vec![Opnd::ExecLo], vec![Opnd::Sgpr(8)]));
    }
    ops
}

// Synthetic input to the control-flow pass, after the exact DPP
// sequence has been recognized by lower(). Include both wave widths.
fn reduction(width: u32, result: u8) -> Vec<Insn> {
    let mut ops = vec![nop(); if width == 32 { 10 } else { 12 }];
    ops[0] = insn("s_or_saveexec_b64", Enc::Sop1, 0,
                 vec![Opnd::Sgpr(0)], vec![Opnd::ConstInt(-1)]);
    ops[2] = insn("v_add_nc_u32", Enc::Vop2, DPP_REDUCE,
                 vec![Opnd::Vgpr(1)], vec![Opnd::Vgpr(1)]);
    let read = ops.len() - 2;
    ops[read] = insn("v_readlane_b32", Enc::Vop3, READ_LANE,
                     vec![Opnd::Sgpr(result)], vec![Opnd::Vgpr(1), Opnd::ConstInt(0)]);
    ops[read + 1] = insn("s_mov_b64", Enc::Sop1, 0,
                         vec![Opnd::ExecLo], vec![Opnd::Sgpr(0)]);
    ops
}

fn offsets(ops: &mut [Insn]) {
    for (i, op) in ops.iter_mut().enumerate() {
        op.offset = i * 4;
    }
}

fn edge(ops: &mut [Insn], from: usize, to: usize, op: u32) {
    ops[from] = branch(op);
    ops[from].aux.imm = to as i64 - from as i64 - 1;
    offsets(ops);
}

#[test]
fn straight_dpp8_exchange_is_not_a_lane_local_copy() {
    let (mut ex, _, _) = fixture::shader(32);
    let words = [0x2800_00e9u32, 0xde54_c100, 0xd760_0007, 0x0001_0100, 0xbf81_0000];
    ex.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ex.isa_data = ex.isa.clone();
    let translated = super::super::translate(&ex).unwrap();
    assert!(translated.hlsl.contains("tb_reduction_read(gtid.x, v0, (gtid.x & ~7u)"));
    assert!(translated.hlsl.contains("tb_reduction_read(gtid.x, v0, 0x0u)"));
    assert_eq!(translated.required_wave_size, None);
    let ops = crate::rdna2::disassemble(&ex.isa);
    for group in [[16,1,1],[64,1,1],[32,2,1]] {
        assert!(!straight_exchange(&mut ops.clone(), group));
    }
    let mut branch = ops.clone();
    branch.insert(0, self::branch(2));
    assert!(!straight_exchange(&mut branch,[32,1,1]));
    let mut mask = ops.clone();
    mask.insert(0, insn("s_mov_b32", Enc::Sop1, 3,
        vec![Opnd::ExecLo],vec![Opnd::ConstInt(0)]));
    assert!(!straight_exchange(&mut mask,[32,1,1]));
    let mut dynamic = ops.clone();
    dynamic[1].src[1] = Opnd::Sgpr(8);
    assert!(!straight_exchange(&mut dynamic,[32,1,1]));
    let mut after_store = ops.clone();
    after_store.insert(1, insn("buffer_store_dword", Enc::Mubuf, 0,
        vec![Opnd::Vgpr(4)],vec![Opnd::Vgpr(5),Opnd::Sgpr(16)]));
    assert!(!straight_exchange(&mut after_store,[32,1,1]));
    let mut exchange = ops.clone();
    exchange.remove(1);
    assert!(straight_exchange(&mut exchange,[64,1,1]));
    assert_eq!(exchange[0].op,DPP8_EXCHANGE);
}

#[test]
fn row_mirror_reduction_accepts_either_bound_ctrl_with_full_masks() {
    for width in [32, 64] {
        let (mut shader, mut words, _) = fixture::shader(width);
        let reference = super::super::translate(&shader).unwrap();
        for word in &mut words {
            if *word == 0xff09_4002 { *word = 0xff01_4002; }
        }
        shader.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        shader.isa_data = shader.isa.clone();
        let translated = super::super::translate(&shader).unwrap();
        assert_eq!(translated.hlsl, reference.hlsl);
        assert_eq!(translated.required_wave_size, reference.required_wave_size);

        for bit in [1 << 24, 1 << 28] {
            let mut ops = crate::rdna2::disassemble(&shader.isa);
            let mirror = ops.iter().position(|i| i.raw.get(1) == Some(&0xff01_4002)).unwrap();
            ops[mirror].raw[1] ^= bit;
            assert!(lower(&mut ops, mirror - 3).is_err());
        }
    }
}

#[test]
fn full_exec_reduction_ignores_fetch_inactive_at_each_shuffle() {
    for width in [32, 64] {
        let (mut shader, words, _) = fixture::shader(width);
        let expected = super::super::translate(&shader).unwrap();
        // Three DPP8 selectors followed by the DPP16 ROW_MIRROR control word.
        for word_index in [3, 5, 7, 10] {
            let mut changed = words.clone();
            if word_index == 10 {
                changed[word_index] |= 1 << 18;
            } else {
                changed[word_index] += 1; // DPP8 FI=1 uses selector 234 instead of 233.
            }
            shader.isa = changed.iter().flat_map(|w| w.to_le_bytes()).collect();
            shader.isa_data = shader.isa.clone();
            let translated = super::super::translate(&shader).unwrap();
            assert_eq!(translated.hlsl, expected.hlsl);
            assert_eq!(translated.required_wave_size, expected.required_wave_size);

            let mut ops = crate::rdna2::disassemble(&shader.isa);
            ops[0].src = vec![Opnd::ConstInt(0)];
            assert!(lower(&mut ops, 2).is_err(), "reduction requires full EXEC");
        }
    }
}

#[test]
fn dpp8_reductions_read_from_the_complementary_subgroup() {
    for width in [32, 64] {
        let (mut shader, mut words, _) = fixture::shader(width);
        // Each lane picks its XOR-2 partner, then its XOR-4 partner.
        words[6] = 0xb3e2_1a02;
        words[8] = 0x688f_ac02;
        shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        shader.isa_data = shader.isa.clone();
        let translated = super::super::translate(&shader).unwrap();
        let lane = if width == 32 { "WaveGetLaneIndex()" } else { "gtid.x" };
        for distance in [2, 4] {
            assert!(translated.hlsl.contains(&format!("{lane} ^ {distance}u")));
        }

        // An asymmetric table must retain its individual source lanes.
        words[8] = 0x4c1d_e502; // [5, 4, 7, 6, 1, 0, 3, 2]
        shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        shader.isa_data = shader.isa.clone();
        let mixed = super::super::translate(&shader).unwrap();
        assert!(mixed.hlsl.contains(&format!("0x4c1de5u >> (3u * ({lane} & 7u))")));

        // One destination reading its own half would double-count a partial sum.
        for (word_index, distance) in [(4, 1), (6, 2), (8, 4)] {
            let mut invalid = words.clone();
            invalid[word_index] ^= distance << 8;
            let isa = invalid.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
            assert!(lower(&mut crate::rdna2::disassemble(&isa), 2).is_err());
        }
    }
}

#[test]
fn translates_two_native_reductions_with_a_uniform_scc_guard() {
    for width in [32, 64] {
        // GFX1030 instructions assembled with llvm-mc, with the four DPP
        // control words from the exact reduction recognized by lower().
        let (mut ex, mut words, cmp) = fixture::shader(width);
        let translated = super::super::translate(&ex).unwrap();
        assert_eq!(translated.required_wave_size, (width == 32).then_some(32));
        if width == 64 {
            assert!(translated.hlsl.contains("tb_reduction_read("));
            assert!(!translated.hlsl.contains("WaveReadLaneAt("));
            assert!(!translated.hlsl.contains("[WaveSize("));
        } else {
            assert!(translated.hlsl.contains("WaveReadLaneAt("));
        }
        // Replace the proven scalar with an unproven entry register; the SCC
        // branch is uniform regardless of the source, so it is still accepted.
        words[cmp] = 0xbf06_8008;
        ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        ex.isa_data = ex.isa.clone();
        assert!(super::super::translate(&ex).is_ok());
    }
}

#[test]
fn native_scc_guard_translates_with_any_source() {
    for width in [32, 64] {
        let (mut ex, mut words, cmp) = fixture::shader(width);
        // s_mov_b32 s7, 1; the following comparison and branch are unchanged.
        words.insert(cmp, 0xbe87_0381);
        ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        ex.isa_data = ex.isa.clone();
        let translated = super::super::translate(&ex).unwrap();
        assert!(translated.hlsl.contains("s7 = 1u;") || translated.hlsl.contains("s7 = 0x1u;") || translated.hlsl.contains("s7 = 0x00000001u;"), "{}", translated.hlsl);
        // s_mov_b32 s7, s8 (unknown input) is still accepted: the SCC branch
        // is uniform regardless of the comparison source.
        words[cmp] = 0xbe87_0308;
        ex.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        ex.isa_data = ex.isa.clone();
        assert!(super::super::translate(&ex).is_ok());
    }
}

#[test]
fn bitwise_and_float_sum_reductions_use_shared_memory_only_for_a_single_wave64() {
    for (opcode, neutral, combine) in [
        (0x36, 0xc1, ") & v2;"), (0x38, 0x80, ") | v2;"),
        (0x3a, 0x80, ") ^ v2;"), (0x06, 0x80, ")) + asfloat(v2))"),
    ] {
        let (mut shader, mut words, end) = fixture::shader(64);
        words.truncate(end);
        words[2] = (words[2] & !0xff) | neutral;
        for word in &mut words {
            if *word >> 24 == 0x4a { *word = (opcode << 24) | (*word & 0xffffff); }
        }
        words.push(0xbf81_0000);
        shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        shader.isa_data = shader.isa.clone();
        let single = super::super::translate(&shader).unwrap();
        assert_eq!(single.required_wave_size, None);
        assert!(single.hlsl.lines().any(|line| line.contains("tb_reduction_read(gtid.x")
            && line.contains(combine)));

        shader.isa_preamble = [0, 128, 1, 1];
        let multi = super::super::translate(&shader).unwrap();
        assert_eq!(multi.required_wave_size, Some(64));
    }
}

#[test]
fn portable_wave_can_scale_its_result_from_an_embedded_table() {
    let (mut shader, mut words, end) = fixture::shader(64);
    words.truncate(end);
    words.extend([
        0xbe9a1f00, 0xbf0d8005, 0x851c8084, // GETPC; bit-test group ID; choose offset 4 or 0
        0xf400074d, 0x38000028, 0xbf8cc07f, // load exponent into s29 and wait
        0x7e0c0207, 0xd7620006, 0x00003b06, // move reduction result s7; LDEXP v6,v6,s29
        0x8f1e8805, 0xbf810000,             // scale group ID for output addressing; endpgm
    ]);
    shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    shader.isa_data = shader.isa.clone();
    shader.isa_data.extend([u32::MAX, 2].into_iter().flat_map(u32::to_le_bytes));
    assert_eq!(super::super::translate(&shader).unwrap().required_wave_size, None);
    shader.isa_preamble = [0, 128, 1, 1];
    assert_eq!(super::super::translate(&shader).unwrap().required_wave_size, Some(64));

    let mut ops = crate::rdna2::disassemble(&shader.isa);
    super::super::lower_wave_reductions(&mut ops).unwrap();
    let load = ops.iter_mut().find(|i| i.mnemonic == "s_load_dword").unwrap();
    load.raw[1] &= 0x01ffffff; // SOFFSET s0 would expose the saved EXEC mask.
    assert!(!portable_single_wave(&ops));
}

#[test]
fn portable_extrema_requires_direct_exec_and_no_mask_escape() {
    for operation in ["umin", "umax", "smin", "smax", "fmin", "fmax"] {
        let ex = fixture::extrema(64, operation);
        let translated = super::super::translate(&ex).unwrap();
        assert_eq!(translated.required_wave_size, None);
        assert!(translated.hlsl.contains("tb_reduction_read("));
        assert!(!translated.hlsl.contains("WaveReadLaneAt("));
        let mut ops = crate::rdna2::disassemble(&ex.isa);
        assert_eq!(super::super::lower_wave_reductions(&mut ops).unwrap(), Some(64));
        assert!(portable_single_wave(&ops));
        let select = ops.iter().position(|i| i.mnemonic == "v_cndmask_b32").unwrap();
        let mut altered = ops.clone();
        altered[select].src[2] = Opnd::Sgpr(8);
        assert!(!portable_single_wave(&altered));
        if matches!(operation, "smin" | "smax" | "fmin" | "fmax") {
            let mut altered = ops.clone();
            altered[select-1].src[0] = Opnd::Literal(0);
            assert!(!portable_single_wave(&altered));
        }
        let mut altered = ops.clone();
        altered.insert(altered.len()-1, insn("s_mov_b32", Enc::Sop1, 0,
            vec![Opnd::Sgpr(8)], vec![Opnd::Sgpr(0)]));
        assert!(!portable_single_wave(&altered));
        let native = fixture::extrema(32, operation);
        assert_eq!(super::super::translate(&native).unwrap().required_wave_size, Some(32));
    }
}

#[test]
fn extrema_accept_the_same_identity_as_a_direct_literal() {
    for op in ["smin", "smax", "fmin", "fmax"] {
        let mut shader = fixture::extrema(64, op);
        let mut words: Vec<u32> = shader.isa.chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
        // Replace the literal move and register CNDMASK with a literal CNDMASK.
        let select = [words[3], (words[4] & !0x1ff) | 0xff, words[2]];
        words.splice(1..5, select);
        shader.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        shader.isa_data = shader.isa.clone();
        let translated = super::super::translate(&shader).unwrap();
        assert_eq!(translated.required_wave_size, None);
        assert!(translated.hlsl.contains("tb_reduction_read(gtid.x"));

        words[3] ^= 1;
        let isa = words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
        assert!(lower(&mut crate::rdna2::disassemble(&isa), 2).is_err());
    }
}

#[test]
fn signed_extrema_with_an_unproven_shuffle_cannot_become_lane_local() {
    for operation in ["smin", "smax"] {
        let mut shader = fixture::extrema(64, operation);
        let mut words: Vec<u32> = shader.isa.chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
        let shuffle = words.iter_mut().find(|w| **w == 0xde54_c102).unwrap();
        *shuffle ^= 1 << 8; // Lane zero now reads itself instead of its partner.
        shader.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        shader.isa_data = shader.isa.clone();
        assert!(super::super::translate(&shader).is_err());
    }
}

#[test]
fn portable_wave64_requires_a_single_group_and_no_other_wave_operations() {
    let (mut ex, _, _) = fixture::shader(64);
    let mut insns = crate::rdna2::disassemble(&ex.isa);
    assert_eq!(super::super::lower_wave_reductions(&mut insns).unwrap(), Some(64));
    assert!(portable_single_wave(&insns));
    let compare = insns.iter().position(|i| i.mnemonic == "s_cmp_eq_u32").unwrap();
    let outside = compare + 2;
    let mut changed = insns.clone();
    changed[outside].mnemonic = "v_readfirstlane_b32".into();
    assert!(!portable_single_wave(&changed));
    changed = insns.clone();
    changed[outside].src[0] = Opnd::Sgpr(0); // saved EXEC must not escape
    assert!(!portable_single_wave(&changed));
    changed = insns.clone();
    changed[compare].src[0] = Opnd::Sgpr(8);
    assert!(!portable_single_wave(&changed));
    changed = insns.clone();
    changed[8].dst[0] = Opnd::Sgpr(0); // overwrites saved EXEC
    assert!(!portable_single_wave(&changed));

    ex.isa_preamble = [0, 128, 1, 1];
    let native = super::super::translate(&ex).unwrap();
    assert_eq!(native.required_wave_size, Some(64));
    assert!(native.hlsl.contains("[WaveSize(64)]"));
    assert!(!native.hlsl.contains("tb_reduction_read("));
}

#[test]
fn scc_guard_must_skip_the_entire_reduction() {
    // An SCC branch is uniform (the SCC is a scalar register), so it may skip
    // the whole reduction regardless of the comparison's sources. It must
    // however skip the entire region, not land inside it.
    for width in [32, 64] {
        let mut ops = vec![compare(vec![Opnd::ConstInt(1), Opnd::Literal(0)]), branch(4)];
        ops.extend(reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 4);
        assert!(check_control_flow(&ops, width).is_ok());
        // The compare may instead use a scalar register; the SCC is uniform,
        // so the branch is still safe.
        ops[0].src[0] = Opnd::Sgpr(9);
        assert!(check_control_flow(&ops, width).is_ok());
        ops[0].src[0] = Opnd::ConstInt(1);
        edge(&mut ops, 1, after - 1, 4);
        assert!(check_control_flow(&ops, width).is_err());
        edge(&mut ops, 1, after, 8); // EXEC is a per-invocation mask
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn scc_guard_accepts_unproven_source_without_bypass() {
    // An SCC branch skipping the region is uniform regardless of the
    // comparison source (the SCC is a scalar register). A branch that bypasses
    // the region by landing inside it is still rejected.
    for width in [32, 64] {
        let mut ops = vec![nop(), insn("s_mov_b32", Enc::Sop1, 0,
            vec![Opnd::Sgpr(9)], vec![Opnd::ConstInt(1)]),
            compare(vec![Opnd::Sgpr(9), Opnd::ConstInt(0)]), branch(4)];
        ops.extend(reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 3, after, 4);
        assert!(check_control_flow(&ops, width).is_ok());
        // A branch landing inside the region bypasses its prologue.
        edge(&mut ops, 3, 5, 4);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn scc_guard_accepts_scalar_chain_source() {
    // An SCC branch skipping the region is uniform even when the comparison
    // source is computed by a scalar chain; no uniformity proof is required.
    for width in [32, 64] {
        let mut ops = vec![nop(),
            insn("s_mov_b32", Enc::Sop1, 0, vec![Opnd::Sgpr(9)], vec![Opnd::ConstInt(2)]),
            insn("s_add_u32", Enc::Sop2, 0, vec![Opnd::Sgpr(10)], vec![Opnd::Sgpr(9), Opnd::ConstInt(1)]),
            insn("s_mov_b32", Enc::Sop1, 0, vec![Opnd::Sgpr(11)], vec![Opnd::Sgpr(10)]),
            compare(vec![Opnd::Sgpr(11), Opnd::Sgpr(9)]), branch(4)];
        ops.extend(reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 5, after, 4);
        assert!(check_control_flow(&ops, width).is_ok());
        // A divergent (VCC) branch skipping the region is still rejected.
        edge(&mut ops, 5, after, 6);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn unconditional_bypass_must_skip_whole_collective_and_have_no_other_entry() {
    for width in [32, 64] {
        let mut ops = vec![nop(), branch(2)];
        ops.extend(reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 2);
        assert!(check_control_flow(&ops, width).is_ok());
        edge(&mut ops, 0, 3, 8);
        assert!(check_control_flow(&ops, width).is_err());
        ops[0] = nop();
        offsets(&mut ops);
        edge(&mut ops, 1, after - 1, 2);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn scc_guard_accepts_prior_reduction_source() {
    // An SCC branch skipping the region is uniform even when the comparison
    // source is a prior reduction's result; no uniformity proof is required.
    for width in [32, 64] {
        let mut ops = reduction(width, 7);
        ops.push(nop());
        ops.push(compare(vec![Opnd::Sgpr(7), Opnd::ConstInt(0)]));
        let guard = ops.len();
        ops.push(branch(5));
        ops.extend(reduction(width, 8));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, guard, after, 5);
        assert!(check_control_flow(&ops, width).is_ok());
        // The comparison source may be any scalar register.
        ops[guard - 1].src[0] = Opnd::Sgpr(8);
        offsets(&mut ops);
        assert!(check_control_flow(&ops, width).is_ok());
    }
}

#[test]
fn execz_forward_skip_accepts_self_contained_region() {
    // EXEC-empty skip: every lane takes the skip and the region never
    // runs; otherwise the region forces the full mask via its saved-EXEC
    // prologue, so the reduction runs cooperatively for the whole wave.
    for width in [32, 64] {
        let mut ops = vec![nop(), branch(8)];
        ops.extend(masked_reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 8);
        assert_eq!(check_control_flow(&ops, width).unwrap(), BTreeSet::from([1]));
    }
}

#[test]
fn execz_forward_skip_accepts_earlier_skips_of_the_whole_region() {
    // The shape compilers emit for nested conditions: an outer skip, then
    // the guard, both leaving past the reduction.
    for width in [32, 64] {
        for (outer, extra) in [(8, 0), (8, 1), (5, 0)] {
            let mut ops = vec![nop(), branch(outer), nop(), branch(8)];
            ops.extend(masked_reduction(width, 7));
            let after = ops.len();
            ops.extend([nop(), nop()]);
            edge(&mut ops, 1, after + extra, outer);
            edge(&mut ops, 3, after, 8);
            // Each EXECZ is voted on by the whole wave.
            let voted = if outer == 8 { BTreeSet::from([1, 3]) } else { BTreeSet::from([3]) };
            assert_eq!(check_control_flow(&ops, width).unwrap(), voted, "outer {outer} width {width}");
        }
    }
}

#[test]
fn execz_forward_skip_rejects_earlier_branches_it_cannot_vote_on() {
    for width in [32, 64] {
        // A divergent skip, and an outer skip that lands inside the region.
        for (outer, inside) in [(6, false), (7, false), (9, false), (8, true), (5, true)] {
            let mut ops = vec![nop(), branch(outer), nop(), branch(8)];
            ops.extend(masked_reduction(width, 7));
            let after = ops.len();
            ops.push(nop());
            edge(&mut ops, 1, if inside { 7 } else { after }, outer);
            edge(&mut ops, 3, after, 8);
            assert!(check_control_flow(&ops, width).is_err(), "outer {outer} width {width}");
        }
    }
}

#[test]
fn execz_forward_skip_rejects_enclosing_loop_before_region() {
    for width in [32, 64] {
        let mut ops = vec![nop(), branch(8), nop(), branch(4)];
        ops.extend(masked_reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 8);
        // This loop encloses the vote but ends before the reduction.
        edge(&mut ops, 3, 0, 4);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn execz_forward_skip_rejects_bypass_and_reentry() {
    for width in [32, 64] {
        let mut ops = vec![nop(), branch(8)];
        ops.extend(masked_reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 8);
        assert!(check_control_flow(&ops, width).is_ok());
        // Another branch enters the skipped interval after the guard.
        edge(&mut ops, 0, 2, 8);
        assert!(check_control_flow(&ops, width).is_err());
        // A backward edge from after the region re-enters it.
        let mut ops = vec![nop(), branch(8)];
        ops.extend(masked_reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 8);
        edge(&mut ops, after, 2, 8);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn execz_forward_skip_rejects_indirect_edges() {
    let mut ops = vec![nop(), branch(8)];
    ops.extend(masked_reduction(32, 7));
    let after = ops.len();
    ops.push(nop());
    edge(&mut ops, 1, after, 8);
    assert!(check_control_flow(&ops, 32).is_ok());
    // An indirect edge is never statically bounded.
    ops[2] = insn("s_setpc_b64", Enc::Sop1, 0, vec![], vec![]);
    assert!(check_control_flow(&ops, 32).is_err());
}

#[test]
fn execz_forward_skip_rejects_unmasked_region() {
    // Only the masked full-mask form is convergent under partial EXEC;
    // a region without the saved-EXEC CNDMASK prologue must be refused
    // even with an execz skip guard.
    for width in [32, 64] {
        let mut ops = vec![nop(), branch(8)];
        ops.extend(reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        edge(&mut ops, 1, after, 8);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn execz_forward_skip_accepts_tail_branch_after_region() {
    // A local forward branch inside the skip window that starts and
    // resolves after the region cannot bypass or enter the region; the
    // region itself is convergent because it forces the full mask.
    for width in [32, 64] {
        let mut ops = vec![nop(), branch(8)];
        ops.extend(masked_reduction(width, 7));
        let tail = ops.len();
        ops.push(insn("s_cbranch_scc0", Enc::Sopp, 4, vec![], vec![]));
        ops.push(nop());
        ops.push(nop());
        let end = ops.len();
        edge(&mut ops, 1, end, 8);
        edge(&mut ops, tail, tail + 2, 4);
        assert!(check_control_flow(&ops, width).is_ok());
    }
}

#[test]
fn execz_forward_skip_rejects_branch_skipping_the_region() {
    // A divergent branch inside the skip interval that reaches the region
    // would bypass or partially execute the whole-wave reduction; it is
    // rejected. (A uniform branch landing exactly on the region start is a
    // safe merge, covered by execz_skip_allows_uniform_merge_branch_in_window.)
    for width in [32, 64] {
        let mut ops = vec![nop(), branch(8), nop()];
        ops.extend(masked_reduction(width, 7));
        let end = ops.len();
        edge(&mut ops, 1, end, 8);
        // A divergent branch landing on the region start.
        edge(&mut ops, 2, 3, 6);
        assert!(check_control_flow(&ops, width).is_err());
        // A uniform branch landing strictly inside the region.
        edge(&mut ops, 2, 6, 4);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn scc_guard_rejects_divergent_entry_and_early_exit() {
    // An SCC branch skipping the region is uniform, but a divergent (VCC)
    // branch entering the region, or an early exit before the region, is
    // rejected.
    for width in [32, 64] {
        let mut ops = vec![nop(), compare(vec![Opnd::ConstInt(0), Opnd::ConstInt(0)]), branch(5)];
        ops.extend(reduction(width, 7));
        let after = ops.len();
        ops.push(nop());
        ops.push(nop());
        edge(&mut ops, 2, after, 5);
        assert!(check_control_flow(&ops, width).is_ok());
        // A divergent branch entering the region is rejected.
        edge(&mut ops, 0, 4, 6);
        assert!(check_control_flow(&ops, width).is_err());
        // An early exit before the region is rejected.
        ops[0] = insn("s_endpgm", Enc::Sopp, 1, vec![], vec![]);
        offsets(&mut ops);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn uniform_branch_landing_on_region_start_is_a_safe_merge() {
    // A uniform branch whose target is exactly the region start is a merge:
    // every active lane either takes it (to the region) or falls through to
    // the region, so the reduction is reached by all active lanes together.
    for width in [32, 64] {
        let mut ops = vec![compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]), branch(5)];
        ops.extend(reduction(width, 7));
        // The reduction (and its saved-EXEC prologue) starts at index 2.
        edge(&mut ops, 1, 2, 5);
        assert!(check_control_flow(&ops, width).is_ok());
        // A divergent (VCC) branch landing on the region start is not safe.
        edge(&mut ops, 1, 2, 6);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn branch_landing_inside_the_reduction_is_rejected() {
    // A branch that lands strictly inside the region would make the
    // reduction execute partially; it is rejected for both uniform and
    // divergent branches.
    for width in [32, 64] {
        let mut ops = vec![compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]), branch(5)];
        ops.extend(reduction(width, 7));
        // Index 5 is strictly inside the region [2, 2 + len).
        edge(&mut ops, 1, 5, 5);
        assert!(check_control_flow(&ops, width).is_err());
        edge(&mut ops, 1, 5, 6);
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn loop_back_edge_with_uniform_body_branches_is_safe() {
    // The back-edge of a loop whose body up to the region contains only
    // uniform branches is safe: the region is reached by every active lane
    // on each iteration.
    for width in [32, 64] {
        let mut ops = vec![
            nop(), // loop header
            compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
            branch(5), // uniform scc1 in the body
        ];
        ops.extend(reduction(width, 7)); // region starts at index 3
        let after = ops.len();
        ops.push(nop());
        ops.push(nop());
        edge(&mut ops, 2, 3, 5); // body branch lands on the region start
        edge(&mut ops, after, 0, 4); // back-edge (scc0) to the loop header
        assert!(check_control_flow(&ops, width).is_ok());
    }
}

#[test]
fn loop_back_edge_with_divergent_body_branch_is_rejected() {
    // A divergent (VCC) branch in the loop body before the region can make
    // the reduction run on only some lanes, even though the back-edge is
    // uniform.
    for width in [32, 64] {
        let mut ops = vec![
            nop(), // loop header
            compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
            branch(6), // divergent vcc in the body
        ];
        ops.extend(reduction(width, 7)); // region starts at index 3
        let after = ops.len();
        ops.push(nop());
        ops.push(nop());
        edge(&mut ops, 2, 0, 6); // divergent body branch to the header
        edge(&mut ops, after, 0, 4); // back-edge (scc0) to the loop header
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn loop_back_edge_divergent_condition_is_rejected() {
    // A divergent (VCC) back-edge makes different lanes iterate different
    // numbers of times, so the reduction in the body would run on a partial
    // lane set; it is rejected even when the loop body is otherwise uniform.
    for width in [32, 64] {
        let mut ops = vec![
            nop(), // loop header
            compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
            branch(5), // uniform scc1 in the body
        ];
        ops.extend(reduction(width, 7)); // region starts at index 3
        let after = ops.len();
        ops.push(nop());
        ops.push(nop());
        edge(&mut ops, 2, 3, 5); // uniform body branch to the region start
        edge(&mut ops, after, 0, 7); // divergent (vccnz) back-edge to the header
        assert!(check_control_flow(&ops, width).is_err());
    }
}

#[test]
fn execz_skip_allows_uniform_merge_branch_in_window() {
    // An EXEC-empty skip of a masked region is safe even when the skip
    // window contains a uniform branch that lands on the region start
    // (a merge): all active lanes reach the region together.
    for width in [32, 64] {
        let mut ops = vec![
            nop(),
            branch(8), // execz skip
            compare(vec![Opnd::ConstInt(1), Opnd::ConstInt(0)]),
            branch(5), // uniform scc1
        ];
        ops.extend(masked_reduction(width, 7)); // region starts at index 4
        let after = ops.len();
        ops.push(nop());
        ops.push(nop());
        edge(&mut ops, 1, after, 8); // execz skip lands past the region
        edge(&mut ops, 3, 4, 5); // uniform branch lands on the region start
        assert!(check_control_flow(&ops, width).is_ok());
    }
}
