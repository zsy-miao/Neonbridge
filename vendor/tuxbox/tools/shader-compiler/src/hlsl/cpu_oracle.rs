//! Test-only bridge: execute the production native decoder and emitter, then
//! make all ordinary scalar registers observable in a small HLSL harness.
//! The independent Python model never consumes our decoded opcodes or HLSL.
use super::*;
use std::{fs, path::PathBuf};

fn extract(words: &[u32]) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract {
        flat_inputs: Vec::new(), flat_outputs: Vec::new(),
        stage: Stage::Compute, shader_model: (6, 0), required_features: None,
        bitcode_stripped: true, fspx_config: None, ps09_pixel_inputs: None,
        fssm: Vec::new(), entry: Some("main".into()), isa_data: isa.clone(),
        isa, isa_offset_in_fssm: 0, psv_threads: None, isa_preamble: [0, 64, 1, 1],
        has_input_sig: false, has_output_sig: false, has_psv0: false, has_root_sig: false,
        input_sig: Vec::new(), output_sig: Vec::new(), resources: Vec::new(),
        gs_info: None, mesh_info: None, primitive_sig: Vec::new(), root_sig: None,
    }
}

fn statements(words: &[u32]) -> Result<(String, String), String> {
    let ex = extract(words);
    let ops = disassemble(&ex.isa);
    if ops.last().is_none_or(|i| i.mnemonic != "s_endpgm") ||
        ops.iter().any(|i| !i.is_complete()) {
        return Err("incomplete native program".into());
    }
    // Preserve the exact full translator's function body and helpers.
    // Only add observable stores, rather than reimplementing the lowering.
    let hlsl = translate(&ex)?.hlsl;
    let (header, entry) = hlsl.split_once("[numthreads(")
        .ok_or("missing compute entry")?;
    let (_, body) = entry.split_once("void main(").ok_or("missing main")?;
    let (_, body) = body.split_once('{').ok_or("missing body")?;
    let body = body.trim_end().strip_suffix('}').ok_or("missing closing brace")?;
    Ok((header.to_owned(), body.to_owned()))
}

#[test]
fn scalar_oracle_bridge() {
    let Some(input) = std::env::var_os("TUXBOX_ORACLE_PROGRAMS") else {
        // Normal unit test still checks one real decoded instruction.
        let words = [0xbe82_0381, 0xbf81_0000];
        assert!(statements(&words).unwrap().1.contains("s2 ="));
        return;
    };
    let output = PathBuf::from(std::env::var_os("TUXBOX_ORACLE_OUTPUT").expect("output dir"));
    let data = fs::read_to_string(input).expect("program manifest");
    let mut status = String::new();
    let mut batch = String::new();
    let mut helpers = String::new();
    let mut batch_count = 0usize;
    let mut batch_no = 0usize;
    for (line_no, line) in data.lines().enumerate() {
        let mut fields = line.split_whitespace();
        let id = fields.next().expect("case id");
        assert_eq!(id, format!("{line_no:06}"));
        let words: Vec<u32> = fields.map(|word| u32::from_str_radix(word,16).expect("word")).collect();
        assert!(words.len() <= 512);
        match statements(&words) {
            Err(reason) => {
                writeln!(status, "{id}\trefused\t{}", reason.replace(['\t','\n','\r'], " ")).unwrap();
            }
            Ok((header, body)) => {
                if batch_count != 0 && header != helpers {
                    batch.push_str("}\n");
                    fs::write(output.join(format!("batch-{batch_no:05}.hlsl")), format!("{helpers}{batch}")).unwrap();
                    batch_no += 1;
                    batch_count = 0;
                }
                if batch_count == 0 {
                    helpers = header;
                    batch = "RWStructuredBuffer<uint> output : register(u0);\n[numthreads(1,1,1)] void main(uint3 gid : SV_GroupID, uint3 gtid : SV_GroupThreadID) {\n".into();
                }
                batch.push_str("{\n");
                batch.push_str(&body);
                for (k, reg) in ["s2","s3","s4","s5","s6","s7","s8","s9","scc"].iter().enumerate() {
                    writeln!(batch, "output[{}u] = {reg};", batch_count*9+k).unwrap();
                }
                batch.push_str("}\n");
                writeln!(status, "{id}\tcompiled\t{batch_no}\t{batch_count}").unwrap();
                batch_count += 1;
                if batch_count == 64 {
                    batch.push_str("}\n");
                    fs::write(output.join(format!("batch-{batch_no:05}.hlsl")), format!("{helpers}{batch}")).unwrap();
                    batch_no += 1;
                    batch_count = 0;
                }
            }
        }
    }
    if batch_count != 0 {
        batch.push_str("}\n");
        fs::write(output.join(format!("batch-{batch_no:05}.hlsl")), format!("{helpers}{batch}")).unwrap();
    }
    fs::write(output.join("status.tsv"), status).unwrap();
}
