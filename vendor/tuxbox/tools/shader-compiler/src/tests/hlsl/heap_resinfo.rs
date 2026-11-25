//! Compute-heap `image_get_resinfo` on a 2D texture.
//!
//! The captured form is `s_lshl_b32` by 5, `s_load_dwordx8` from the class-5
//! heap base, then `image_get_resinfo` dim 1, dmask 3, no A16/D16. That query
//! is `Texture2D.GetDimensions`. Other dimensions, a depth component, and
//! 16-bit address or data stay refused.

use super::Emitter;
use crate::rdna2::disassemble;
use crate::{ShaderExtract, Stage};

fn fssm_rows(rows: &[[u32; 5]]) -> Vec<u8> {
    let mut program = vec![0; 0x100];
    program[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    let offset = (program.len() - 16) as u64;
    program[0xe8..0xe8 + 8].copy_from_slice(&offset.to_le_bytes());
    program[0xf8..0xf8 + 4].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    program.extend(rows.iter().flatten().copied().flat_map(u32::to_le_bytes));
    let size = (program.len() - 16) as u32;
    program[8..12].copy_from_slice(&size.to_le_bytes());
    program
}

fn heap_extract(isa: &[u8]) -> ShaderExtract {
    ShaderExtract { flat_inputs: Vec::new(), flat_outputs: Vec::new(),
        stage: Stage::Compute,
        shader_model: (6, 6),
        required_features: Some(1 << 25),
        entry: Some("main".into()),
        isa: isa.to_vec(),
        isa_data: isa.to_vec(),
        isa_offset_in_fssm: 0,
        isa_preamble: [0, 64, 1, 1],
        // Class 5, one dword, hardware register s10.
        fssm: fssm_rows(&[[5, 0, 1, 0, 10]]),
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        psv_threads: None,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        primitive_sig: Vec::new(),
        has_input_sig: false,
        has_output_sig: false,
        has_root_sig: true,
        has_psv0: false,
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        root_sig: Some(tuxbox_dxbc::rootsig::RootSig {
            version: 1,
            flags: 0x400,
            params: Vec::new(),
            num_static_samplers: 0,
            static_samplers: Vec::new(),
        }),
    }
}

/// s4 = s4 << 5; s[8:15] = heap[s4]; v[1:2] = image size of that 2D texture at v19.
fn resinfo_words(image: u32, image_hi: u32) -> [u32; 5] {
    [
        0x8f04_8504, // s_lshl_b32 s4, s4, 5
        0xf40c_0205, // s_load_dwordx8 s[8:15], s[10:11], s4
        0x0800_0000,
        image,
        image_hi,
    ]
}

fn lower(words: &[u32]) -> Result<String, String> {
    let ex = heap_extract(&words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>());
    let insns = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, insns.clone());
    em.resolve_resources()?;
    for (idx, ins) in insns.iter().enumerate() {
        em.emit_insn(idx, ins, false, None)?;
    }
    let mut hlsl = em.resource_decls()?;
    hlsl.push_str(&em.heap_handle_decls());
    hlsl.push_str(&em.body);
    Ok(hlsl)
}

#[test]
fn compute_heap_2d_resinfo_is_get_dimensions() {
    // image_get_resinfo v[1:2], v19, s[8:15], s[0:3] dmask:3 dim:2D unorm
    let hlsl = lower(&resinfo_words(0xf038_1308, 0x0002_0113)).unwrap();
    assert!(hlsl.contains("Texture2D<float4> tb_heap_t2_8;"), "{hlsl}");
    assert!(hlsl.contains("s8 = tb_heap_index(s4 >> 5);"), "{hlsl}");
    assert!(hlsl.contains("tb_heap_t2_8 = ResourceDescriptorHeap[s8];"), "{hlsl}");
    assert!(hlsl.contains("tb_heap_t2_8.GetDimensions(v19, "), "{hlsl}");
    assert!(hlsl.contains("if (exec_lo & 1u) v1 = w"), "{hlsl}");
    assert!(hlsl.contains("if (exec_lo & 1u) v2 = h"), "{hlsl}");
    assert!(!hlsl.contains("= levels"), "{hlsl}");
    assert!(!hlsl.contains("NonUniformResourceIndex"), "{hlsl}");
}

#[test]
fn other_resinfo_shapes_stay_refused() {
    // dim:3D
    let dim3 = lower(&resinfo_words(0xf038_1310, 0x0002_0113)).unwrap_err();
    assert!(
        dim3.contains("unsupported direct heap texture operation image_get_resinfo dim 2"),
        "{dim3}"
    );
    // dmask bit 2 asks for depth.
    let depth = lower(&resinfo_words(0xf038_1708, 0x0002_0113)).unwrap_err();
    assert!(
        depth.contains("image_get_resinfo supports 2D width, height and mip count"),
        "{depth}"
    );
    // A16.
    let a16 = lower(&resinfo_words(0xf038_1308, 0x4002_0113)).unwrap_err();
    assert!(
        a16.contains("image_get_resinfo supports 2D width, height and mip count"),
        "{a16}"
    );
}

#[test]
fn aligned_immediate_is_added_to_verified_dynamic_heap_offset() {
    let mut words = resinfo_words(0xf038_1308, 0x0002_0113);
    words[2] |= 32;
    let hlsl = lower(&words).unwrap();
    assert!(hlsl.contains("tb_heap_index((s4 + 0x20u) >> 5)"), "{hlsl}");
    words[2] = 0x0800_0004;
    assert!(lower(&words).is_err());
}
