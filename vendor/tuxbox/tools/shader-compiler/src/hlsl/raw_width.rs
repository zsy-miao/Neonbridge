//! Raw buffer transfers stay one dword per HLSL `Load`/`Store`.
//!
//! The MUBUF address is 32-bit (`vaddr + soffset + offset`), so a title can produce an offset
//! that wraps, such as 0xFFFFFFFC. Under vkd3d-proton on NVIDIA, a multi-dword raw load
//! (`Load2`..`Load4`) is lowered through `VK_NV_raw_access_chains`; one 16-byte load at
//! 0xFFFFFFFC lost the device (Xid 109) on an RTX 5080 with Proton Experimental 11.0-20261112.
//! Four 4-byte loads at the same offsets completed and returned zero for the out-of-range word.
//! Merging the per-dword transfers would reintroduce that fault, so this test pins the form.
use super::Emitter;
use crate::rdna2::disassemble;
use crate::{ShaderExtract, Stage};

fn compute_extract(words: &[u32]) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract {
        stage: Stage::Compute,
        shader_model: (6, 0),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: false,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: Vec::new(),
        root_sig: None,
    }
}

#[test]
fn raw_buffer_transfers_are_emitted_one_dword_at_a_time() {
    // vaddr v2 (OFFEN), vdata v[4:7], srsrc s[8:11], soffset 0.
    let cases = [
        ([0xe038_1000, 0x8002_0402], "buffer_load_dwordx4", "Load"),
        ([0xe078_1000, 0x8002_0402], "buffer_store_dwordx4", "Store"),
    ];
    for (words, mnemonic, op) in cases {
        let ex = compute_extract(&words);
        let insns = disassemble(&ex.isa);
        assert_eq!(insns[0].mnemonic, mnemonic);
        let mut em = Emitter::new(&ex, insns.clone());
        em.raw_transfer_on(&insns[0], "raw_ubuf0".into(), true).unwrap();
        assert_eq!(em.body.matches(&format!("raw_ubuf0.{op}(")).count(), 4, "{mnemonic}:\n{}", em.body);
        for wide in ["2(", "3(", "4("] {
            assert!(!em.body.contains(&format!("{op}{wide}")), "{mnemonic} merged:\n{}", em.body);
        }
    }
}

/// Locally constructed linear SRV, followed by both indexed address forms.
/// Encodings produced by llvm-mc -mcpu=gfx1030.
fn indexed_fixture() -> ShaderExtract {
    let mut ex = compute_extract(&[
        0xbe8b_03ff, 0x1101_4fac, // format
        0xbe8a_03c1, // count
        0xbe88_030c, // low address
        0x8809_ff0d, 0x0001_0000, // high address | byte stride
        0xe038_2000, 0x8002_0402, // IDXEN
        0xe038_300c, 0x8002_0402, // IDXEN + OFFEN, immediate 12
        0xbf81_0000,
    ]);
    ex.resources.push(crate::psv::PsvResource {
        res_type: 4, space: 0, lower: 0, upper: 0,
    });
    ex
}

#[test]
fn byte_index_proof_rejects_unknown_stride_and_clobbers() {
    let ex = indexed_fixture();
    let ops = disassemble(&ex.isa);
    assert!(super::raw_index::local_byte_descriptor(&ops[..4], 8));
    assert!(super::raw_index::local_byte_descriptor(&ops[..5], 8));
    assert!(!super::raw_index::local_byte_descriptor(&ops[1..4], 8));
    assert!(!super::raw_index::local_byte_descriptor(&ops[..4], 12));
    let mut wrong = ops[..4].to_vec();
    wrong[3].src[1] = crate::rdna2::Opnd::Literal(0x40000);
    assert!(!super::raw_index::local_byte_descriptor(&wrong, 8));
    wrong = ops[..4].to_vec();
    wrong.push(ops[1].clone()); // clobbers count
    assert!(!super::raw_index::local_byte_descriptor(&wrong, 8));
}

#[test]
fn proven_byte_index_emits_addresses_without_widening_loads() {
    let ex = indexed_fixture();
    let translated = super::translate(&ex).unwrap();
    assert_eq!(translated.hlsl.matches("raw_sbuf0.Load(").count(), 8);
    // Captured by the ordinary test harness; available to the GPU control
    // with --nocapture, without exposing a test-only API in the library.
    println!("BEGIN_HLSL\n{}\nEND_HLSL", translated.hlsl);
    let ops = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, ops.clone());
    em.resolve_resources().unwrap();
    assert_eq!(em.raw_byte_index.len(), 2);
    assert!(em.raw_byte_index.contains(&ops[4].offset));
    assert!(em.raw_byte_index.contains(&ops[5].offset));
    em.emit_mubuf(4, &ops[4]).unwrap();
    em.emit_mubuf(5, &ops[5]).unwrap();
    assert!(em.body.contains("= v2 + 0x0u + 0u;"), "{}", em.body);
    assert!(em.body.contains("= (v2 + v3) + 0x0u + 12u;"), "{}", em.body);
    assert_eq!(em.body.matches("raw_sbuf0.Load(").count(), 8);
    assert!(!em.body.contains("raw_sbuf0.Load4("));
    assert!(em.regs.contains("v3"));
    em.raw_byte_index.clear();
    assert!(em.emit_mubuf(4, &ops[4]).unwrap_err().contains("stride"));
}

#[test]
fn indexed_descriptor_proof_does_not_cross_a_basic_block() {
    let mut ex = indexed_fixture();
    // Insert an unconditional branch between construction and the load.
    let mut words: Vec<u32> = ex.isa.chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
    words.insert(6, 0xbf82_0000);
    ex.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let ops = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, ops.clone());
    em.resolve_resources().unwrap();
    assert!(em.raw_byte_index.is_empty());
}

#[test]
fn static_zero_scalar_offset_matches_immediate_descriptor_load() {
    // s_mov_b32 s66, 0; s_load_dwordx4 s[64:67], s[0:1], 0.
    // The offset register overlaps the destination, so the proof must use
    // the value BEFORE the load, not the outgoing dataflow state.
    let variants = [
        [0xbec2_0380, 0xf408_1000, 0xfa00_0000, 0xbf81_0000],
        [0xbec2_0380, 0xf408_1000, 66u32 << 25, 0xbf81_0000],
    ];
    let mut loads = Vec::new();
    for words in variants {
        let ex = compute_extract(&words);
        let insns = disassemble(&ex.isa);
        let mut em = Emitter::new(&ex, insns);
        em.resolve_resources().unwrap();
        loads.push(em.dsel_loads.clone());
    }
    assert_eq!(loads[0], loads[1]);
    assert!(!loads[0].is_empty());
    let ex = compute_extract(&[0xbec2_0381, 0xf408_1000, 66u32 << 25, 0xbf81_0000]);
    let mut em = Emitter::new(&ex, disassemble(&ex.isa));
    let reason = em.resolve_resources().unwrap_err();
    assert!(reason.contains("dynamic soffset unsupported"), "{reason}");
}

#[test]
fn exact_nonzero_scalar_offset_matches_static_descriptor_and_unknown_refuses() {
    // s66 = 32; compare a dynamic encoding with the equivalent immediate.
    let variants = [
        [0xbec2_03a0, 0xf408_1000, 0xfa00_0020, 0xbf81_0000],
        [0xbec2_03a0, 0xf408_1000, 66u32 << 25, 0xbf81_0000],
    ];
    let mut loads = Vec::new();
    for words in variants {
        let ex = compute_extract(&words);
        let mut em = Emitter::new(&ex, disassemble(&ex.isa));
        em.resolve_resources().unwrap();
        loads.push(em.dsel_loads.clone());
    }
    assert_eq!(loads[0], loads[1]);
    assert_eq!(loads[0].get(&1), Some(&(64, 32)));
    // A path can skip the definition; no constant can be inferred.
    let ex = compute_extract(&[0xbf84_0001, 0xbec2_03a0, 0xf408_1000,
                               66u32 << 25, 0xbf81_0000]);
    let mut em = Emitter::new(&ex, disassemble(&ex.isa));
    assert!(em.resolve_resources().unwrap_err().contains("dynamic soffset"));
}

#[test]
fn bounded_runtime_code_offset_uses_real_table_index_without_modulo() {
    // llvm-mc gfx1030: getpc s[8:9]; and s10,s12,3; lshl s10,s10,2;
    // s_load_dword s16,s[8:9],s10 offset:20; endpgm.
    let words = [0xbe881f00, 0x870a830c, 0x8f0a820a, 0xf4000404, 0x14000014, 0xbf810000];
    let mut ex = compute_extract(&words);
    ex.isa_data.extend([11u32,22,33,44].iter().flat_map(|v| v.to_le_bytes()));
    let ops = disassemble(&ex.isa);
    assert_eq!(super::code_offset::prove(&ops,3,24,40), Some(10));
    let translated = super::translate(&ex).unwrap();
    assert!(translated.hlsl.contains("tb_code[t"), "{}", translated.hlsl);
    assert!(!translated.hlsl.contains("% 4u]"), "{}", translated.hlsl);
    assert!(translated.hlsl.contains("s8 + s10 + 0x14u - 24u"), "{}", translated.hlsl);
    println!("BEGIN_HLSL\n{}\nEND_HLSL",translated.hlsl);
    assert_eq!(super::code_offset::prove(&ops,3,24,36),None); // final dword would exceed table
    assert_eq!(super::code_offset::prove(&ops,3,24,24),None); // empty table
    let mut bad = ops.clone();
    bad[2].src[1] = crate::rdna2::Opnd::ConstInt(1);
    assert_eq!(super::code_offset::prove(&bad,3,24,40),None); // unaligned
    let mut bad = ops.clone();
    bad[1].src[1] = crate::rdna2::Opnd::ConstInt(7);
    assert_eq!(super::code_offset::prove(&bad,3,24,40),None); // mask too large
    let mut bad = ops.clone();
    bad[1].dst[0] = crate::rdna2::Opnd::Sgpr(9);
    assert_eq!(super::code_offset::prove(&bad,3,24,40),None); // PC high clobber
    let mut bad = ops.clone();
    bad[3].op = 1; // two dwords: the last possible load crosses the table end
    assert_eq!(super::code_offset::prove(&bad,3,24,40),None);
    let mut bad = ops.clone();
    // A branch entering directly at the load invalidates both proofs.
    let branch = disassemble(&0xbf82_fffcu32.to_le_bytes()).remove(0);
    let mut branch = branch;
    branch.offset = 24;
    bad.push(branch);
    assert_eq!(super::code_offset::prove(&bad,3,24,40),None);
}

fn bounded_cbv_fixture() -> ShaderExtract {
    // gfx1030: and s10,s8,1; lshl s10,s10,5; load descriptor s[16:19]
    // from s[4:5]+s10; buffer_load s20; endpgm.
    let mut ex = compute_extract(&[
        0x870a8108, 0x8f0a850a, 0xf4080402, 0x14000000,
        0xf4200508, 0xfa000000, 0xbf810000]);
    ex.isa_preamble = [0,1,1,1];
    ex.resources.push(crate::psv::PsvResource {res_type:2,space:0,lower:0,upper:1});
    ex.root_sig = Some(crate::meta::RootSig {
        version:1, flags:0, num_static_samplers:0, static_samplers:Vec::new(),
        params:vec![crate::meta::RootParam {visibility:0, kind:crate::meta::RootParamKind::Table(vec![
            crate::meta::RootRange {range_type:2,num:2,base_register:0,space:0,offset_in_table:0}
        ])}]
    });
    ex
}

#[test]
fn bounded_dynamic_cbv_descriptors_select_only_reflected_slots() {
    let ex = bounded_cbv_fixture();
    let mut em = Emitter::new(&ex, disassemble(&ex.isa));
    em.resolve_resources().unwrap();
    assert_eq!(em.dsel_runtime.get(&2),Some(&(16,4,10,0)));
    assert_eq!(em.bsel.get(&3).map(|x| x.1.len()),Some(2));
    let translated = super::translate(&ex).unwrap();
    assert!(translated.hlsl.contains("tb_dsel16 = 0x4000000u | (s10 + 0x0u)"));
    assert!(translated.hlsl.contains("tb_dsel16 == 0x4000020u"));
    println!("BEGIN_HLSL\n{}\nEND_HLSL",translated.hlsl);
    let mut bad = bounded_cbv_fixture();
    bad.resources[0].upper = 0;
    assert!(Emitter::new(&bad,disassemble(&bad.isa)).resolve_resources().is_err());
    let mut bad = bounded_cbv_fixture();
    bad.isa.splice(0..0,0xbe850380u32.to_le_bytes()); // clobber base high
    assert!(Emitter::new(&bad,disassemble(&bad.isa)).resolve_resources().is_err());
    let mut bad = bounded_cbv_fixture();
    bad.isa.splice(24..24,0xbe950310u32.to_le_bytes()); // descriptor word escapes
    assert!(Emitter::new(&bad,disassemble(&bad.isa)).resolve_resources().is_err());
    let mut bad = bounded_cbv_fixture();
    bad.isa.splice(0..0,0xbf820000u32.to_le_bytes()); // control-flow boundary
    assert!(Emitter::new(&bad,disassemble(&bad.isa)).resolve_resources().is_err());
    let mut bad = bounded_cbv_fixture();
    bad.root_sig = None;
    assert!(Emitter::new(&bad,disassemble(&bad.isa)).resolve_resources().is_err());
    let mut bad = bounded_cbv_fixture();
    bad.isa[4..8].copy_from_slice(&0x8f0a840au32.to_le_bytes());
    assert!(Emitter::new(&bad,disassemble(&bad.isa)).resolve_resources().is_err());
}

#[test]
fn dynamic_cbv_rejects_incompatible_reflection_and_range_bounds() {
    use crate::meta::RootParamKind;
    for variant in 0..8 {
        let mut ex = bounded_cbv_fixture();
        let RootParamKind::Table(ranges) = &mut ex.root_sig.as_mut().unwrap().params[0].kind
            else { panic!("fixture root table") };
        match variant {
            0 => ranges[0].range_type = 0, // SRV rather than CBV
            1 => ranges[0].num = 1, // second candidate out of bounds
            2 => ranges[0].num = u32::MAX, // unbounded table
            3 => ranges[0].offset_in_table = 1, // ambiguous physical base
            4 => ranges[0].base_register = u32::MAX, // register overflow
            5 => ex.resources[0].space = 1, // reflection space mismatch
            6 => {
                ex.resources.push(crate::psv::PsvResource {res_type:2,space:0,lower:1,upper:1});
            }, // overlapping reflection
            7 => ex.resources[0].res_type = 4, // wrong resource class
            _ => unreachable!(),
        }
        let mut em = Emitter::new(&ex, disassemble(&ex.isa));
        assert!(em.resolve_resources().is_err(), "accepted incompatible variant {variant}");
    }
    // A mask with five independently possible bits would expand to 32
    // descriptors; even a large finite range does not bypass the cap.
    let mut ex = bounded_cbv_fixture();
    ex.isa[0..4].copy_from_slice(&0x870a9f08u32.to_le_bytes()); // AND 31
    let RootParamKind::Table(ranges) = &mut ex.root_sig.as_mut().unwrap().params[0].kind
        else { panic!("fixture root table") };
    ranges[0].num = 32;
    ex.resources[0].upper = 31;
    assert!(Emitter::new(&ex,disassemble(&ex.isa)).resolve_resources().is_err());
}
