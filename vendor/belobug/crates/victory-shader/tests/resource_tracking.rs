use std::hint::black_box;
use std::time::Instant;

use victory_shader::ir::{self, Program, Reg};
use victory_shader::resource::{Address, track};
use victory_shader::{HwStage, RootParam, RootType, ShaderError};

const TABLE: RootParam = RootParam {
    root: RootType::Table,
    table_offset_dwords: 0,
    sgprs: 1,
    base_register: 0,
    user_data: 8,
    slot: 0,
};
const ZERO_HIGH: u32 = 0xBE83_0380; // s_mov_b32 s3, 0
const COPY_BASE: u32 = 0xBE82_0308; // s_mov_b32 s2, s8
const ADD_16: u32 = 0x8002_9002; // s_add_u32 s2, s2, 16
const LOAD: u32 = 0xF408_0401; // s_load_dwordx4 s[16:19], s[2:3], imm
const END: u32 = 0xBF81_0000; // s_endpgm

fn build(words: &[u32]) -> Program {
    let code: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    ir::build(&code, false, 256).unwrap()
}

// Each unrolled step advances the same base and reads several fields.
fn unrolled_loads(steps: u32, loads_per_step: u32) -> Program {
    let mut step = vec![ADD_16];
    for field in 0..loads_per_step {
        step.extend([LOAD, 0xFA00_0000 | (4 * field)]);
    }
    let mut words = vec![ZERO_HIGH, COPY_BASE];
    for _ in 0..steps {
        words.extend_from_slice(&step);
    }
    words.push(END);
    build(&words)
}

#[test]
fn tracks_every_offset_in_a_shared_address_chain() {
    let program = unrolled_loads(32, 4);
    let tracked = track(&program, &[TABLE], HwStage::Ps).unwrap();
    let expected: Vec<_> = (1..=32)
        .flat_map(|step| (0..4).map(move |field| (16 * step + 4 * field, None)))
        .collect();
    assert_eq!(
        tracked
            .scalar
            .iter()
            .map(|(_, a)| (a.offset, a.dynamic))
            .collect::<Vec<_>>(),
        expected
    );
    assert!(tracked.scalar.iter().all(|(_, a)| a.param == 0));
}

#[test]
fn shared_address_keeps_its_dynamic_term_and_wraps_offsets() {
    let program = build(&[
        ZERO_HIGH,
        0x8002_0908, // s_add_u32 s2, s8, s9
        0x8002_FF02, // s_add_u32 s2, s2, 0xfffffff0
        0xFFFF_FFF0,
        LOAD,
        0xFA00_0004,
        LOAD,
        0xFA00_0014,
        END,
    ]);
    let dynamic = program
        .inputs
        .iter()
        .find(|(r, _)| *r == Reg::S(9))
        .unwrap()
        .1;
    let tracked = track(&program, &[TABLE], HwStage::Ps).unwrap();
    assert_eq!(
        tracked.scalar,
        [
            (
                (0, 3),
                Address {
                    param: 0,
                    offset: 0xFFFF_FFF4,
                    dynamic: Some(dynamic),
                    includes_base: false,
                }
            ),
            (
                (0, 4),
                Address {
                    param: 0,
                    offset: 4,
                    dynamic: Some(dynamic),
                    includes_base: false,
                }
            ),
        ]
    );
}

#[test]
fn shared_sum_with_two_dynamic_terms_stays_opaque() {
    let program = build(&[
        ZERO_HIGH,
        0x8004_0A09, // s_add_u32 s4, s9, s10
        0x8002_0408, // s_add_u32 s2, s8, s4
        LOAD,
        0xFA00_0000,
        LOAD,
        0xFA00_0004,
        END,
    ]);
    let dynamic = program.blocks[0].insts[1].write(Reg::S(4)).unwrap();
    let tracked = track(&program, &[TABLE], HwStage::Ps).unwrap();
    assert_eq!(
        tracked.scalar,
        [
            (
                (0, 3),
                Address {
                    param: 0,
                    offset: 0,
                    dynamic: Some(dynamic),
                    includes_base: false,
                }
            ),
            (
                (0, 4),
                Address {
                    param: 0,
                    offset: 4,
                    dynamic: Some(dynamic),
                    includes_base: false,
                }
            ),
        ]
    );
    assert_eq!(
        track(&program, &[], HwStage::Ps).unwrap_err(),
        ShaderError::Unsupported {
            offset: 12,
            what: "an address not from a root parameter",
        }
    );
}

// cargo test -p victory-shader --release --test resource_tracking \
//     bench_unrolled_loads -- --ignored --nocapture
#[test]
#[ignore = "benchmark"]
fn bench_unrolled_loads() {
    for (steps, loads, iterations) in [(16, 4, 1_000), (128, 4, 50), (384, 4, 20)] {
        let program = unrolled_loads(steps, loads);
        let layout = [TABLE];
        assert_eq!(
            track(&program, &layout, HwStage::Ps).unwrap().scalar.len(),
            (steps * loads) as usize
        );
        let mut samples = [0.0; 7];
        for sample in &mut samples {
            let start = Instant::now();
            for _ in 0..iterations {
                let tracked = track(black_box(&program), black_box(&layout), HwStage::Ps).unwrap();
                black_box(tracked);
            }
            *sample = start.elapsed().as_secs_f64() * 1e6 / f64::from(iterations);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{steps:>3} steps x {loads} loads: {:>9.2} us/track (median)",
            samples[3]
        );
    }
}
