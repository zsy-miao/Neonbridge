use super::*;
use crate::rdna2::disassemble;

const ZERO: u32 = 0xbec20380; // s_mov_b32 s66, 0
const ONE: u32 = 0xbec20381; // s_mov_b32 s66, 1
const USE: [u32; 2] = [0xe0342000, 0x42070202]; // buffer_load_dwordx2 with soffset s66
const END: u32 = 0xbf810000;

fn tracked(words: &[u32]) -> BTreeSet<(usize, u8)> {
    let bytes = words
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect::<Vec<_>>();
    proven_zero(&disassemble(&bytes))
}

#[test]
fn every_incoming_path_must_preserve_zero() {
    let branch = 0xbf840001; // s_cbranch_scc0 +1
    let nop = 0xbf800000;
    assert_eq!(
        tracked(&[ZERO, branch, nop, USE[0], USE[1], END]),
        BTreeSet::from([(12, 66)])
    );
    assert!(tracked(&[ZERO, branch, ONE, USE[0], USE[1], END]).is_empty());
    assert!(tracked(&[branch, ZERO, USE[0], USE[1], END]).is_empty());
    // s_cbranch_scc0 -4 returns to the buffer instruction with s66 now equal to 1.
    assert!(tracked(&[ZERO, USE[0], USE[1], ONE, 0xbf84fffc, END]).is_empty());
}

#[test]
fn nonzero_constant_proof_handles_alu_merges_and_clobbers() {
    let load = [0xf4081000, 66u32 << 25];
    let thirty_two = 0xbec203a0;
    let add = 0x80428142; // s_add_u32 s66, s66, 1
    let check = |words: &[u32]| {
        let bytes: Vec<_> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
        proven_smem_constants(&disassemble(&bytes))
    };
    assert_eq!(check(&[thirty_two, add, load[0], load[1], END]).get(&(8,66)), Some(&33));
    let branch = 0xbf840001;
    assert_eq!(check(&[thirty_two, branch, thirty_two, load[0], load[1], END]).get(&(12,66)), Some(&32));
    assert!(check(&[thirty_two, branch, ONE, load[0], load[1], END]).is_empty());
    assert!(!check(&[thirty_two, load[0], load[1], load[0], load[1], END]).contains_key(&(12,66)));
    assert!(check(&[thirty_two, 0xd70f4200, 0x02020300, load[0], load[1], END]).is_empty());
}

#[test]
fn an_upper_bound_is_insufficient_for_a_static_table_offset() {
    // CSELECT of 20 and 10, or an unknown register masked to four bits.
    // Both values have an upper bound but neither is an exact offset.
    for producer in [0x8518_8a94, 0x8718_8f05] {
        let words = [producer, 0xf408_1000, 24 << 25, END];
        let bytes: Vec<_> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert!(proven_smem_constants(&disassemble(&bytes)).is_empty());
    }
}

#[test]
fn copied_offsets_require_proven_sources_on_every_path() {
    let zero65 = 0xbec10380; // s_mov_b32 s65, 0
    let copy = 0xbec20341;   // s_mov_b32 s66, s65
    let self_copy = 0xbec20342;
    let load = [0xf4081000, 66u32 << 25];
    assert_eq!(tracked(&[zero65, copy, self_copy, load[0], load[1], END]),
               BTreeSet::from([(12, 66)]));
    assert!(tracked(&[copy, load[0], load[1], END]).is_empty());
    let branch = 0xbf840001;
    assert!(tracked(&[branch, zero65, copy, load[0], load[1], END]).is_empty());
}

#[test]
fn scalar_load_offset_must_be_zero_on_all_paths() {
    let load = [0xf4081000, 66u32 << 25];
    let branch = 0xbf840001;
    assert_eq!(tracked(&[ZERO, load[0], load[1], END]), BTreeSet::from([(4, 66)]));
    assert!(tracked(&[ONE, load[0], load[1], END]).is_empty());
    assert!(tracked(&[ZERO, branch, ONE, load[0], load[1], END]).is_empty());
    assert!(tracked(&[branch, ZERO, load[0], load[1], END]).is_empty());
    assert!(tracked(&[ZERO, load[0], load[1], load[0], load[1], END]).contains(&(4, 66)));
    assert!(!tracked(&[ZERO, load[0], load[1], load[0], load[1], END]).contains(&(12, 66)));
}

#[test]
fn overlapping_and_secondary_scalar_destinations_clobber_zero() {
    for writer in [
        [0xf4081000, 0xfa000000], // s_load_dwordx4 s[64:67], s[0:1], 0
        [0xd70f4200, 0x02020300], // v_add_co_u32 v0, s66, v0, v1
    ] {
        assert!(tracked(&[ZERO, writer[0], writer[1], USE[0], USE[1], END]).is_empty());
    }
}
