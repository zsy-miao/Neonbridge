use super::*;
use crate::{ShaderExtract, Stage, rdna2::disassemble};

// Assembled by llvm-mc -mcpu=gfx1030. Include aliasing destinations,
// especially scalar searches that overwrite the high source word.
const OPS: [(&str, u32); 12] = [
    ("s_brev_b64", 0xbe82_0c02),
    ("s_bcnt0_i32_b32", 0xbe82_0d02),
    ("s_bcnt0_i32_b64", 0xbe83_0e02),
    ("s_ff0_i32_b32", 0xbe82_1102),
    ("s_ff0_i32_b64", 0xbe82_1202),
    ("s_flbit_i32_b64", 0xbe83_1602),
    ("s_flbit_i32", 0xbe82_1702),
    ("s_flbit_i32_i64", 0xbe83_1802),
    ("s_bitset0_b64", 0xbe82_1c04),
    ("s_bitset1_b64", 0xbe82_1e02),
    ("s_bitcmp0_b64", 0xbf0e_0402),
    ("s_bitcmp1_b64", 0xbf0f_0402),
];

fn extract(words: &[u32]) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract { flat_inputs: Vec::new(), flat_outputs: Vec::new(),
        stage: Stage::Compute, shader_model: (6, 0), required_features: None,
        bitcode_stripped: true, fspx_config: None, ps09_pixel_inputs: None,
        fssm: Vec::new(), entry: Some("main".into()), isa_data: isa.clone(),
        isa, isa_offset_in_fssm: 0, psv_threads: None, isa_preamble: [0, 64, 1, 1],
        has_input_sig: false, has_output_sig: false, has_psv0: false, has_root_sig: false,
        input_sig: Vec::new(), output_sig: Vec::new(), resources: Vec::new(),
        gs_info: None, mesh_info: None, primitive_sig: Vec::new(), root_sig: None,
    }
}

fn program(op: u32, lo: u32, hi: u32, index: u32) -> Vec<u32> {
    vec![0xbe82_03ff, lo, 0xbe83_03ff, hi, 0xbe84_03ff, index,
         0xbf06_8004, op, 0xbf81_0000]
}

#[test]
fn accepts_all_native_scalar_bit_forms_and_preserves_half_width() {
    for (name, op) in OPS {
        let ex = extract(&program(op, 0x80000000, 0x00102030, 63));
        let ops = disassemble(&ex.isa);
        assert_eq!(ops[4].mnemonic, name);
        let translated = super::super::translate(&ex).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(translated.hlsl.contains("tb_bit"), "{name}");
        // For searches/counts the destination is just one SGPR.
        if shape(name).unwrap().dst == 1 {
            let Opnd::Sgpr(reg) = ops[4].dst[0] else { panic!() };
            assert!(clobbers_scalar(&ops[4], reg));
            assert!(!clobbers_scalar(&ops[4], reg + 1));
        }
    }
}

#[test]
fn refuses_unknown_halves_wave_masks_and_branch_entries() {
    for words in [
        // No reaching definition of high half.
        vec![0xbe82_03ff, 123, 0xbe82_0c02, 0xbf81_0000],
        // Explicit EXEC and VCC are not ordinary 64-bit values.
        vec![0xbe82_0c7e, 0xbf81_0000],
        vec![0xbe82_0c6a, 0xbf81_0000],
        // A copied EXEC pair cannot become ordinary just by renaming it.
        vec![0xbe82_047e, 0xbe82_0c02, 0xbf81_0000],
        // Unknown high half overwrites a previously known pair.
        vec![0xbe82_03ff, 1, 0xbe83_03ff, 2, 0xbe83_037e, 0xbe82_0c02, 0xbf81_0000],
        // Branch into the operation skips the definitions.
        vec![0xbf82_0004, 0xbe82_03ff, 1, 0xbe83_03ff, 2, 0xbe82_0c02, 0xbf81_0000],
        // Misaligned pair and special-register destination.
        vec![0xbe82_0481, 0xbe82_0c03, 0xbf81_0000],
        vec![0xbe82_0481, 0xbefe_0c02, 0xbf81_0000],
        // Literal pair extension is deliberately not assumed.
        vec![0xbe82_0cff, 123, 0xbf81_0000],
    ] {
        let ex = extract(&words);
        let ops = disassemble(&ex.isa);
        let at = ops.iter().position(|i| i.mnemonic == "s_brev_b64").unwrap();
        let mut emitter = Emitter::new(&ex, ops.clone());
        assert!(emit(&mut emitter, at, &ops[at]).is_err(), "{words:x?}");
    }
}

#[test]
fn minimized_dependent_scalar_reproducers_translate() {
    let cases: &[&[u32]] = &[
        // s_bitset0_b64
        &[0xbe8203c1, 0xbe8303ff, 0x80000000, 0xbe8403ff, 0xbc8b1cb8, 0xbe8503bf, 0xbe8603ff, 0x55555555, 0xbe8703ff, 0x9bf61b12, 0xbe8803ff, 0x75ce92b4, 0xbe8903a1, 0xbf068180, 0x84020202, 0xbe881c02, 0xbf810000],
        // s_bitcmp1_b64
        &[0xbe8203ff, 0xb9dd7fd1, 0xbe830388, 0xbe8403ff, 0x0db1cdd1, 0xbe8503ff, 0x80000001, 0xbe8603ff, 0x9c6888b6, 0xbe870387, 0xbe88038f, 0xbe8903ff, 0xd810ae0c, 0xbf068180, 0x84050808, 0xbf0f0904, 0xbf810000],
        // s_bitset1_b64
        &[0xbe8203ff, 0x3ba04cb5, 0xbe8303ff, 0xc8d3983d, 0xbe8403ff, 0x00010000, 0xbe8503ff, 0x25eeccdb, 0xbe8603ff, 0xc248236a, 0xbe8703ff, 0x80000001, 0xbe8803ff, 0x645c5573, 0xbe8903a0, 0xbf068080, 0x83840904, 0xbe841e09, 0xbf810000],
        // s_bitcmp0_b64
        &[0xbe8203ff, 0x00000100, 0xbe8303a0, 0xbe8403ff, 0x7fffffff, 0xbe8503ff, 0x000000ff, 0xbe8603c2, 0xbe8703ff, 0x7ffffffe, 0xbe8803ff, 0x000000ff, 0xbe8903c1, 0xbf068180, 0x91840402, 0xbf0e0406, 0xbf810000],
        // s_brev_b64
        &[0xbe8203ff, 0xbd2baec6, 0xbe8303ff, 0x29e91062, 0xbe8403ff, 0x7ad5cb5c, 0xbe8503c0, 0xbe8603ff, 0xccf05f96, 0xbe8703ff, 0x0460aaa4, 0xbe8803bf, 0xbe8903ff, 0x2ee85d25, 0xbf068080, 0x91880506, 0xbe880c08, 0xbf810000],
        // s_flbit_i32_i64
        &[0xbe8203ff, 0xe4149976, 0xbe8303ff, 0x05258a0f, 0xbe8403ff, 0xcbf61b70, 0xbe8503ff, 0x091be210, 0xbe8603ff, 0xdce41137, 0xbe8703ff, 0x647f0a11, 0xbe8803ff, 0xc10f13b4, 0xbe8903ff, 0x80000001, 0xbf068180, 0x84870407, 0xbe861806, 0xbf810000],
        // s_flbit_i32_b64
        &[0xbe8203ff, 0x7ffffffe, 0xbe8303ff, 0x99cba70d, 0xbe8403c2, 0xbe8503ff, 0x0000ffff, 0xbe8603ff, 0xa7614058, 0xbe8703ff, 0x9b2fa762, 0xbe8803ff, 0x3bd2391b, 0xbe890381, 0xbf068180, 0x82020206, 0xbe821602, 0xbf810000],
        // s_bcnt0_i32_b32
        &[0xbe820381, 0xbe8303bf, 0xbe840382, 0xbe8503ff, 0x00000100, 0xbe8603bf, 0xbe8703ff, 0x6cf23272, 0xbe8803ff, 0xbdd72c70, 0xbe8903ff, 0x0000007f, 0xbf068080, 0xbe821d02, 0xbe820d02, 0xbf810000],
        // s_bcnt0_i32_b64
        &[0xbe8203ff, 0xa8ae996e, 0xbe8303ff, 0xfa02d9b9, 0xbe8403ff, 0xef7870b5, 0xbe8503ff, 0x8b5e42a8, 0xbe86038f, 0xbe8703ff, 0xce2ded13, 0xbe8803a0, 0xbe8903c0, 0xbf068080, 0x82080708, 0xbe890e08, 0xbf810000],
        // s_ff0_i32_b64
        &[0xbe8203ff, 0x6266612d, 0xbe8303ff, 0x7b329ec3, 0xbe840382, 0xbe8503ff, 0x6a02fd24, 0xbe8603ff, 0x0000ffff, 0xbe8703c1, 0xbe8803ff, 0x000000ff, 0xbe8903ff, 0x80000001, 0xbf068180, 0x84020302, 0xbe831202, 0xbf810000],
        // s_flbit_i32
        &[0xbe8203ff, 0xb350e3e7, 0xbe8303ff, 0xcd470b2d, 0xbe8403ff, 0x73b11dbb, 0xbe8503ff, 0x2f47a3d1, 0xbe8603bf, 0xbe8703ff, 0x00010000, 0xbe8803ff, 0xffecd869, 0xbe8903ff, 0x2a22dc0d, 0xbf068080, 0x82030304, 0xbe831703, 0xbf810000],
        // s_ff0_i32_b32
        &[0xbe8203ff, 0x233e34b0, 0xbe8303ff, 0x7ffffffe, 0xbe840381, 0xbe850388, 0xbe8603ff, 0x49f406bb, 0xbe8703ff, 0x63a6caa0, 0xbe8803ff, 0x440e9dfe, 0xbe8903ff, 0x8b91e34f, 0xbf068080, 0x85080908, 0xbe841108, 0xbf810000],
    ];
    for words in cases {
        let ex = extract(words);
        assert!(super::super::translate(&ex).is_ok(), "{words:x?}");
    }
}

#[test]
fn dependent_scalar_chains_do_not_promote_unknown_flags_or_halves() {
    // Compare ordinary data, conditional-select and then first-zero.
    // Unknown SCC, including an EXEC-derived comparison, must not
    // establish provenance for the destination of the conditional select.
    for (prefix, accepted) in [
        (vec![0xbe82_0381, 0xbe83_0382, 0xbf06_8080], true),
        (vec![0xbe82_0381, 0xbe83_0382], false),
        (vec![0xbe82_0381, 0xbe83_0382, 0xbf06_807e], false),
    ] {
        let mut words = prefix;
        words.extend([0x8504_0302, 0xbe85_1104, 0xbf81_0000]);
        let ex = extract(&words);
        let ops = disassemble(&ex.isa);
        assert_eq!(ops[ops.len()-3].mnemonic, "s_cselect_b32");
        let em = Emitter::new(&ex, ops.clone());
        assert_eq!(ordinary_sites(&em).contains(&(ops.len()-2)), accepted, "{words:x?}");
    }
    // A known low half plus an unproven high half must not become
    // ordinary after a 64-bit shift.
    let words = [0xbe82_0381, 0x8f84_8102, 0xbe86_0c04, 0xbf81_0000];
    let ex = extract(&words);
    let ops = disassemble(&ex.isa);
    assert_eq!(ops[1].mnemonic, "s_lshl_b64");
    let em = Emitter::new(&ex, ops);
    assert!(!ordinary_sites(&em).contains(&2));
}

#[test]
fn local_move_and_arithmetic_proof_carries_ordinary_words() {
    let ex = extract(&[
        0xbe82_0481, // s_mov_b64 s[2:3], 1
        0xbe84_0402, // s_mov_b64 s[4:5], s[2:3]
        0xbe84_0c04, // s_brev_b64 s[4:5], s[4:5]
        0xbe85_1604, // s_flbit_i32_b64 s5, s[4:5]
        0xbf81_0000,
    ]);
    let out = super::super::translate(&ex).unwrap();
    assert!(out.hlsl.contains("reversebits(s5)"));
    assert!(out.hlsl.contains("firstbithigh(s5)"));
}

#[test]
fn ordinary_runtime_constant_buffer_values_are_not_restricted_to_literals() {
    let mut ex = extract(&[
        0xf408_0200, 0xfa00_0000, // s_load_dwordx4 descriptor from s[0:1]
        0xf424_0084, 0xfa00_0000, // s_buffer_load_dwordx2 s[2:3]
        0xbf8c_c07f,             // s_waitcnt lgkmcnt(0)
        0xbe82_0c02,             // reverse both loaded words
        0xbf81_0000,
    ]);
    ex.resources.push(crate::psv::PsvResource {
        res_type: 2, space: 0, lower: 0, upper: 0,
    });
    let out = super::super::translate(&ex).unwrap();
    assert!(out.hlsl.contains("cb0"));
    assert!(out.hlsl.contains("reversebits(s3)"));
    assert!(out.hlsl.contains("reversebits(s2)"));
    println!("BEGIN_DYNAMIC\n{}\nEND_DYNAMIC", out.hlsl);
}

#[test]
fn scalar_bit_cpu_oracle_fixture() {
    // Deterministic boundary and pseudo-random cases. The external DXC
    // oracle folds the emitted statements and checks every output against
    // Python's independent integer operations, with no GPU involved.
    let mut vectors = vec![
        (0, 0, 0), (u32::MAX, u32::MAX, 31),
        (u32::MAX, 0, 32), (0, u32::MAX, 63),
        (1, 0, 64), (0, 1, 65), (0x80000000, 0, 127),
        (0, 0x80000000, u32::MAX), (0xaaaaaaaa, 0x55555555, 1),
        (0x55555555, 0xaaaaaaaa, 33),
    ];
    for bit in 0..64 {
        let x = 1u64 << bit;
        vectors.push((x as u32, (x >> 32) as u32, bit));
        let x = !x;
        vectors.push((x as u32, (x >> 32) as u32, bit + 64));
    }
    let mut random = 0x5343_4152u32;
    for _ in 0..24 {
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            random
        };
        vectors.push((next(), next(), next()));
    }
    for (name, op) in OPS {
        let mut hlsl = String::from("RWStructuredBuffer<uint> output : register(u0);\n[numthreads(1,1,1)] void main() {\n");
        for (n, &(lo, hi, index)) in vectors.iter().enumerate() {
            let ex = extract(&program(op, lo, hi, index));
            let ops = disassemble(&ex.isa);
            let mut em = Emitter::new(&ex, ops.clone());
            for (i, ins) in ops.iter().enumerate().take(5) {
                em.emit_insn(i, ins, false, None).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            hlsl.push_str(&format!("// CASE {lo} {hi} {index}\n{{ uint s2=0u,s3=0u,s4=0u,scc=0u;\n"));
            hlsl.push_str(&em.body);
            hlsl.push('\n');
            for (k, reg) in ["s2", "s3", "s4", "scc"].iter().enumerate() {
                hlsl.push_str(&format!("output[{}u] = {reg};\n", n * 4 + k));
            }
            hlsl.push_str("}\n");
        }
        hlsl.push_str("}\n");
        println!("BEGIN_SCALAR {name}\n{hlsl}END_SCALAR {name}");
    }
}
