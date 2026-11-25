//! DS_SWIZZLE_B32 in an ordinary compute shader, exchanged through group shared memory.
//!
//! RDNA2 ISA: offset[15] selects QDMode, where lane i of each quad reads lane
//! offset[2*i+1 : 2*i] of the same quad. Otherwise offset[14:0] holds the
//! and/or/xor bit masks applied within each group of 32 lanes.
//! `tools/check_compute_swizzle.py` runs the same fixture on a GPU.
use super::translate;
use crate::rdna2::disassemble;
use crate::{ShaderExtract, Stage};

mod fixture {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/support/compute_swizzle_fixture.rs"));
}

fn swizzle(offset: u32, word0_extra: u32) -> Result<String, String> {
    let ex = fixture::shader(offset, word0_extra);
    assert_eq!(disassemble(&ex.isa)[0].mnemonic, "ds_swizzle_b32");
    translate(&ex).map(|t| t.hlsl)
}

#[test]
fn quad_permute_mode_reads_the_selected_lane_of_each_quad() {
    // swizzle(QUAD_PERM, 3, 2, 1, 0): 0b00_01_10_11 reverses every quad.
    let hlsl = swizzle(0x801b, 0).unwrap();
    assert!(hlsl.contains("tb_swizzle[tb_lane] = ((exec_lo & 1u) != 0u) ? v1 : 0u;"), "{hlsl}");
    assert!(hlsl.contains("tb_swizzle[(tb_lane & ~3u) | ((0x1bu >> (2u * (tb_lane & 3u))) & 3u)]"), "{hlsl}");
    assert!(!hlsl.contains("& ~31u"), "{hlsl}");
}

#[test]
fn bit_mask_mode_is_unchanged() {
    // swizzle(BITMASK_PERM, "01pip"): and 0x1f, or 0, xor 1.
    let hlsl = swizzle(0x041f, 0).unwrap();
    assert!(hlsl.contains("tb_swizzle[(tb_lane & ~31u) | (((tb_lane & 31u) | 0u) ^ 1u)]"), "{hlsl}");
}

#[test]
fn other_high_offset_bits_and_gds_stay_refused() {
    for (offset, extra) in [(0x8100, 0), (0xe000, 0), (0xffff, 0), (0x801b, 1 << 16), (0x041f, 1 << 17)] {
        let error = swizzle(offset, extra).unwrap_err();
        assert!(error.contains("unsupported compute DS swizzle encoding"), "{offset:#x}/{extra:#x}: {error}");
    }
}
