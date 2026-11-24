//! `v_perm_b32` with a selector known only at run time: both emitted helpers
//! (`tb_perm` here and `tb_ray_perm` in ray/alu.rs) work per byte in 32-bit
//! integers. Transcriptions of both against the ISA's byte table.

use super::translate_valu_probe;

/// RDNA2 ISA V_PERM_B32: bytes 0-3 of S1 (lo), 4-7 of S0 (hi), 8-11 replicate
/// the sign bits lo[15], lo[31], hi[15], hi[31], 12 is 0x00 and 13+ is 0xff.
fn isa_byte(hi: u32, lo: u32, sel: u32) -> u32 {
    let bytes = (u64::from(hi) << 32) | u64::from(lo);
    match sel {
        0..=7 => ((bytes >> (8 * sel)) & 0xff) as u32,
        8 => 0xff * ((lo >> 15) & 1),
        9 => 0xff * (lo >> 31),
        10 => 0xff * ((hi >> 15) & 1),
        11 => 0xff * (hi >> 31),
        12 => 0,
        _ => 0xff,
    }
}

fn tb_perm_byte(hi: u32, lo: u32, sel: u32) -> u32 {
    if sel >= 13 { return 0xff; }
    if sel == 12 { return 0; }
    if sel == 11 { return (hi >> 31) * 0xff; }
    if sel == 10 { return ((hi >> 15) & 1) * 0xff; }
    if sel == 9 { return (lo >> 31) * 0xff; }
    if sel == 8 { return ((lo >> 15) & 1) * 0xff; }
    if sel >= 4 { (hi >> ((sel - 4) * 8)) & 0xff } else { (lo >> (sel * 8)) & 0xff }
}

fn tb_perm(hi: u32, lo: u32, sel: u32) -> u32 {
    tb_perm_byte(hi, lo, sel & 0xff) | (tb_perm_byte(hi, lo, (sel >> 8) & 0xff) << 8)
        | (tb_perm_byte(hi, lo, (sel >> 16) & 0xff) << 16) | (tb_perm_byte(hi, lo, sel >> 24) << 24)
}

fn tb_ray_perm(a: u32, b: u32, selectors: u32) -> u32 {
    let mut result = 0;
    for i in 0..4 {
        let sel = (selectors >> (8 * i)) & 255;
        let byte_index = if sel < 8 { sel } else { 2 * sel + 1 };
        let word = if byte_index & 4 != 0 { a } else { b };
        let mut v = (word >> (8 * (byte_index & 3))) & 255;
        if sel >= 8 {
            v = if sel >= 13 || (sel < 12 && v >= 128) { 255 } else { 0 };
        }
        result |= v << (8 * i);
    }
    result
}

#[test]
fn every_selector_in_every_byte_matches_the_isa() {
    // Sign bits set and clear in each half, distinct bytes elsewhere.
    let words = [(0x8123_8456u32, 0x0789_7abcu32), (0x7f01_7f02, 0x8003_8004), (0, 0xffff_ffff), (0x0000_8000, 0x8000_0000)];
    for (hi, lo) in words {
        for sel in 0..=255u32 {
            for lane in 0..4 {
                let selector = 0x0c0c_0c0c & !(0xff << (8 * lane)) | (sel << (8 * lane));
                let want: u32 = (0..4).map(|i| isa_byte(hi, lo, (selector >> (8 * i)) & 0xff) << (8 * i)).sum();
                assert_eq!(tb_perm(hi, lo, selector), want, "tb_perm {hi:#x} {lo:#x} {selector:#x}");
                assert_eq!(tb_ray_perm(hi, lo, selector), want, "tb_ray_perm {hi:#x} {lo:#x} {selector:#x}");
            }
        }
    }
    assert_eq!(tb_perm(0x8000_8000, 0x8000_8000, 0x0b0a_0908), 0xffff_ffff);
    assert_eq!(tb_perm(0x1234_5678, 0x9abc_def0, 0x0d0c_0d0c), 0xff00_ff00);
}

#[test]
fn a_dynamic_selector_from_a_vgpr_goes_through_tb_perm() {
    // VOP3 v_perm_b32 v0, v0, v1, v2 (op 836): the selector is v2.
    let hlsl = translate_valu_probe(&[0xd744_0000, 0x040a_0300], 4).unwrap();
    assert!(hlsl.contains("tb_perm(v0, v1, v2)"), "{hlsl}");
}

/// VOP3 `op v0, v0, s0, s1` with `word0_bits` OR'd in (op_sel at 11-14, abs at 8-10).
fn permlane(op: u32, word0_bits: u32, word1: u32) -> String {
    translate_valu_probe(&[0xd400_0000 | (op << 16) | word0_bits, word1], 2).unwrap()
}

#[test]
fn permlane16_rows_fetch_inactive_and_bound_ctrl() {
    const SGPR_SELECTS: u32 = 0x0004_0100;
    let same = permlane(887, 0, SGPR_SELECTS);
    assert!(same.contains("(t0lane & ~15u) | ") && !same.contains("^ 16u"), "{same}");
    assert!(same.contains("WaveReadLaneAt(v0, t0src)") && same.contains("if (t0live)"), "{same}");
    let cross = permlane(888, 0, SGPR_SELECTS);
    assert!(cross.contains("((t0lane & ~15u) ^ 16u)"), "{cross}");
    // FI: the source is read whether or not it is active.
    let fi = permlane(887, 1 << 11, SGPR_SELECTS);
    assert!(!fi.contains("WaveActiveBallot"), "{fi}");
    // BOUND_CTRL: an inactive source writes 0.
    let bound = permlane(888, 1 << 12, SGPR_SELECTS);
    assert!(bound.contains("(t0live ? t0value : 0u)"), "{bound}");
}

#[test]
fn permlane16_undefined_encodings_are_logged_not_decoded() {
    for (op, bits, word1) in [
        (887, 1 << 8, 0x0004_0100),  // abs
        (888, 1 << 13, 0x0004_0100), // op_sel[2]
        (887, 1 << 15, 0x0004_0100), // clamp
        (888, 0, 0x0004_0002),       // SGPR source
    ] {
        let hlsl = permlane(op, bits, word1);
        assert!(hlsl.contains("source passed through") && !hlsl.contains("WaveReadLaneAt"), "{op} {bits:#x}: {hlsl}");
    }
}
