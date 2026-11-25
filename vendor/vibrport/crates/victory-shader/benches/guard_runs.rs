//! Time structurisation only; the synthetic programs need no title files or GPU.

use std::hint::black_box;
use std::time::Instant;

use victory_shader::{ir, structure::structure};

fn program(masks: u32, run: usize) -> ir::Program {
    let mut words = Vec::new();
    for i in 0..masks.max(1) {
        if masks != 0 {
            // v_cmpx_eq_u32 exec, 0, v[i]: each comparison narrows EXEC.
            words.push(0x7DA4_0080 | (i << 9));
        }
        words.extend(std::iter::repeat_n(0x4A00_0081, run)); // v_add_nc_u32 v0, 1, v0
    }
    words.push(0xBF81_0000); // s_endpgm
    let code = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    ir::build(&code, true, 256).unwrap()
}

fn main() {
    const BATCH: u32 = 64;
    for (label, masks, run) in [
        ("unmasked", 0, 512),
        ("short runs", 8, 1),
        ("long runs", 8, 64),
    ] {
        let p = program(masks, run);
        black_box(structure(&p).unwrap());
        let mut samples = [std::time::Duration::ZERO; 9];
        for sample in &mut samples {
            let start = Instant::now();
            for _ in 0..BATCH {
                black_box(structure(black_box(&p)).unwrap());
            }
            *sample = start.elapsed() / BATCH;
        }
        samples.sort_unstable();
        println!(
            "{label}: {:?} per program (median)",
            samples[samples.len() / 2]
        );
    }
}
