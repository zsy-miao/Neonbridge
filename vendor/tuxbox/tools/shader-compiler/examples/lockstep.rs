//! Translate a synthetic compute program, supplied as native words, whose
//! thread group is one wave. `resolve` names it as a ray tracing resolve
//! program, which the translator runs with whole lane masks.
use tuxbox_xshader::{ShaderExtract, Stage, hlsl};

mod fixture {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/wave_guard_fixture.rs"));
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let resolve = args.next_if(|arg| arg == "resolve").is_some();
    let width: u32 = args.next().expect("wave width").parse().expect("integer");
    assert!(matches!(width, 32 | 64));
    let words: Vec<u32> = args.map(|s| u32::from_str_radix(&s, 16).expect("hex native word")).collect();
    assert!(!words.is_empty() && words.len() <= 512 && words.last() == Some(&0xbf810000));
    let (mut shader, _, _): (ShaderExtract, _, _) = fixture::shader(width);
    shader.isa = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    shader.isa_data = shader.isa.clone();
    assert!(matches!(shader.stage, Stage::Compute));
    if resolve {
        shader.entry = Some("CS_RaytracingResolve_Synthetic".into());
        // The compute table such programs carry: two table pointers, constants
        // and two heap pointers in the first twelve SGPRs.
        let rows = [[3, 0, 1, 0, 4u32], [3, 0, 1, 0, 0x105], [0, 0, 4, 1, 0x206], [5, 0, 1, 0, 10], [6, 0, 1, 0, 11]];
        shader.fssm = vec![0; 0x100];
        shader.fssm[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
        shader.fssm[0xe8..0xec].copy_from_slice(&0xf0u32.to_le_bytes());
        shader.fssm[0xf8..0xfc].copy_from_slice(&(rows.len() as u32).to_le_bytes());
        shader.fssm.extend(rows.into_iter().flatten().flat_map(u32::to_le_bytes));
        let payload = shader.fssm.len() as u32 - 16;
        shader.fssm[8..12].copy_from_slice(&payload.to_le_bytes());
    }
    match hlsl::translate(&shader) {
        Ok(result) => {
            assert_eq!(result.required_wave_size, Some(width));
            print!("{}", result.hlsl);
        }
        Err(reason) => {
            eprintln!("{reason}");
            std::process::exit(65);
        }
    }
}
