//! A stand-in for the auto-exposure average (`VICTORY_EXPOSURE_STUB`): when
//! that compute shader does not translate, the frame's exposure is never
//! written and the final image comes out black. The stand-in keeps the
//! original's bindings (its root layout, read from the container) and
//! writes a constant where the original writes its results.
//!
//! The original (`avgLuminanceCompute`, measured on the title's shader
//! pack, hash `e622b718b6d9b057`) ends, in thread 0, with
//! `image_store v[4:7], v[10:11], s[8:15]` (the descriptor at byte 0x80 of
//! the table `s5` points to, at coordinates `(s6, s7)` from user data) and
//! `buffer_store_format_xyzw v[0:3]` at element 0 of the typed buffer at
//! byte 0x40 of that table. The stand-in stores the value in every channel
//! of both.
// shortcut: the channels' meanings (exposure, its inverse, the previous
// frame's) are not known; every one gets the same value.

use super::HostShader;
use crate::ShaderError;

/// The containers the stand-in may replace by default.
pub const HASHES: &[&str] = &["e622b718b6d9b057"];

/// The mid-grey average luminance written by default.
pub const VALUE: f32 = 0.18;

const S_ENDPGM: u32 = 0xBF81_0000;

/// The stand-in's GFX10 machine code (wave64 compute): thread 0 stores
/// `value` in every channel of the original's two outputs.
pub fn code(value: f32) -> Vec<u32> {
    let k = value.to_bits();
    let lit = 0xFF;
    let v_mov = |dst: u32, src: u32| 0x7E00_0000 | (dst << 17) | (1 << 9) | src;
    let mut c = vec![
        0xBE84_0305, // s_mov_b32 s4, s5
        0xBE85_0380, // s_mov_b32 s5, 0
        0x7D84_0080, // v_cmp_eq_u32 vcc, 0, v0
        0xBE80_246A, // s_and_saveexec_b64 s[0:1], vcc
    ];
    let branch = c.len();
    c.push(0); // s_cbranch_execz to the end
    c.extend([0xF40C_0202, 0xFA00_0080]); // s_load_dwordx8 s[8:15], s[4:5], 0x80
    c.extend([0xF408_0002, 0xFA00_0040]); // s_load_dwordx4 s[0:3], s[4:5], 0x40
    c.push(0xBF8C_C07F); // s_waitcnt lgkmcnt(0)
    for v in [4, 5, 6, 7, 0, 1, 2, 3] {
        c.extend([v_mov(v, lit), k]);
    }
    c.push(v_mov(10, 6)); // v_mov_b32 v10, s6
    c.push(v_mov(11, 7)); // v_mov_b32 v11, s7
    // image_store v[4:7], v[10:11], s[8:15] dmask:0xf dim:2D unorm
    c.extend([0xF020_1F08, 0x0002_040A]);
    // buffer_store_format_xyzw v[0:3], v0, s[0:3], 0
    c.extend([0xE01C_0000, 0x8000_0000]);
    let end = c.len();
    c[branch] = 0xBF88_0000 | (end - branch - 1) as u32; // s_cbranch_execz
    c.push(S_ENDPGM);
    c
}

/// The stand-in for `container` (one of [`HASHES`]): its compute program
/// with [`code`] in place of its own, translated.
pub fn stub(container: &[u8], value: f32) -> Result<HostShader, ShaderError> {
    let bytes = code(value).iter().flat_map(|w| w.to_le_bytes()).collect();
    super::translate_code(container, Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::code;

    #[test]
    fn stand_in_decodes_to_the_original_stores() {
        let bytes: Vec<u8> = code(0.18).iter().flat_map(|w| w.to_le_bytes()).collect();
        let insts = crate::gfx10::decode(&bytes).unwrap();
        let names: Vec<&str> = insts.iter().filter_map(|i| i.name()).collect();
        assert_eq!(
            names.iter().filter(|n| **n == "v_mov_b32").count(),
            10,
            "{names:?}"
        );
        for want in [
            "v_cmp_eq_u32",
            "s_and_saveexec_b64",
            "s_cbranch_execz",
            "s_load_dwordx8",
            "s_load_dwordx4",
            "image_store",
            "buffer_store_format_xyzw",
            "s_endpgm",
        ] {
            assert!(names.contains(&want), "{want} missing: {names:?}");
        }
        // The branch skips to the end.
        let br = insts
            .iter()
            .find(|i| i.name() == Some("s_cbranch_execz"))
            .unwrap();
        let end = insts.last().unwrap();
        let delta = (br.words[0] & 0xFFFF) as usize;
        assert_eq!(br.offset as usize + 4 + 4 * delta, end.offset as usize);
    }
}
