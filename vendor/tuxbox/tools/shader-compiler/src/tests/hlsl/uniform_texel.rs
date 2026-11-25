use super::*;
use crate::rdna2::disassemble;

// Sample one uint into v7, copy the first active lane to s5, range-check
// against eleven and extract bits 8..12 from the same scalar.
const LOAD: [u32; 2] = [0xf0040108, 0x00000702];
const LANE: u32 = 0x7e0a0507;
const BOUNDS: u32 = 0xbf088b05;
const FIELD: [u32; 2] = [0x9381ff05, 0x00050008];

fn inferred(load: &[u32], read: &[u32], compare: u32, unpack: &[u32]) -> bool {
    let instructions = [load, read, &[compare], unpack].concat();
    let bytes = instructions
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let program = disassemble(&bytes);
    packed_uint(&program[0], &program[1..], &branch_targets(&program))
}

#[test]
fn range_check_and_scalar_field_identify_a_uniform_uint_texel() {
    assert!(inferred(&LOAD, &[LANE], BOUNDS, &FIELD));
    // The fall-through path to the extraction still has the original value.
    assert!(inferred(
        &LOAD,
        &[LANE],
        BOUNDS,
        &[&[0xbf880002], &FIELD[..]].concat()
    ));
    // Reading bits after comparing a value as an integer is insufficient for
    // a multi-component, half-width or signed load, or an exponent extraction.
    assert!(!inferred(
        &[LOAD[0] | 0x200, LOAD[1]],
        &[LANE],
        BOUNDS,
        &FIELD
    ));
    assert!(!inferred(
        &[LOAD[0], LOAD[1] | (1 << 31)],
        &[LANE],
        BOUNDS,
        &FIELD
    ));
    assert!(!inferred(&LOAD, &[LANE], 0xbf028b05, &FIELD));
    assert!(!inferred(&LOAD, &[LANE], 0xbf088005, &FIELD)); // Compare to zero.
    assert!(!inferred(&LOAD, &[LANE], BOUNDS, &[FIELD[0], 0x00080017]));
}

#[test]
fn clobbers_and_alternative_entries_break_the_inference() {
    assert!(!inferred(&LOAD, &[0x7e0e0280, LANE], BOUNDS, &FIELD)); // v_mov_b32 v7, 0
    assert!(!inferred(&LOAD, &[0xbf880003, LANE], BOUNDS, &FIELD)); // skip the read
    assert!(!inferred(
        &LOAD,
        &[LANE],
        BOUNDS,
        &[&[0xbe850380], &FIELD[..]].concat()
    )); // s_mov_b32 s5, 0
    assert!(!inferred(
        &LOAD,
        &[LANE],
        BOUNDS,
        &[&[0xbe840480], &FIELD[..]].concat()
    )); // s_mov_b64 s[4:5], 0
    assert!(!inferred(
        &LOAD,
        &[LANE],
        BOUNDS,
        &[&[0xbf820000], &FIELD[..]].concat()
    )); // branch to the extraction
    assert!(!inferred(&LOAD, &[LANE], BOUNDS, &[FIELD[0] + 1, FIELD[1]])); // different scalar
}
