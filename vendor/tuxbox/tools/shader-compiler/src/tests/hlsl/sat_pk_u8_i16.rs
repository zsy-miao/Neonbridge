//! V_SAT_PK_U8_I16 packs two signed 16-bit halves of the source into two
//! unsigned bytes in the low 16 bits of the destination. The upper 16 bits
//! are cleared. Each half is saturated with int-domain `clamp(v, 0, 255)`
//! after sign-extending the 16-bit field.
#[cfg(test)]
mod tests {
    use super::super::Emitter;
    use crate::rdna2::disassemble;
    use crate::{ShaderExtract, Stage};

    fn compute_extract(words: &[u32]) -> ShaderExtract {
        let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        ShaderExtract { flat_inputs: Vec::new(), flat_outputs: Vec::new(),
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
    fn packs_two_saturated_signed_bytes_into_the_low_word() {
        // VOP1 v_sat_pk_u8_i16 v0, v1 (opcode 98) followed by s_endpgm.
        let words: [u32; 2] = [0x7e00_c501, 0xbf81_0000];
        let ex = compute_extract(&words);
        let insns = disassemble(&ex.isa);
        assert_eq!(insns[0].mnemonic, "v_sat_pk_u8_i16");
        let mut em = Emitter::new(&ex, insns.clone());
        em.emit_vec(&insns[0]).unwrap();
        // Both halves saturate against [0, 255] after sign-extension, and the
        // high byte is shifted into bits 8..16 of the destination.
        assert!(em.body.contains("clamp(asint(v1 << 16) >> 16, 0, 255)"), "{}", em.body);
        assert!(em.body.contains("clamp(asint(v1) >> 16, 0, 255)"), "{}", em.body);
        assert!(em.body.contains("<< 8)"), "{}", em.body);
    }
}
