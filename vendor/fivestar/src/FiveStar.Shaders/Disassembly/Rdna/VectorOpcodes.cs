// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// Vector ALU opcodes (VOP1, VOP2, VOP3-only, VOP3P) with their numbers from the opcode tables of the AMD "RDNA 2
/// Instruction Set Architecture" reference guide, chapter 13. VOPC is built by <see cref="CompareOpcodes"/>. The
/// VOP3 encoding holds VOPC at 0x000, VOP2 at 0x100 and VOP1 at 0x180 plus their opcode.
/// </summary>
internal static class VectorOpcodes
{
    public const int Vop2Base = 0x100;
    public const int Vop2End = 0x140;
    public const int Vop1Base = 0x180;
    public const int Vop1End = 0x200;

    public static readonly FrozenDictionary<int, OpcodeInfo> Vop2 = OpcodeTable.Parse(
    [
        "01 v_cndmask_b32 u:uum", "03 v_add_f32 f:ff", "04 v_sub_f32 f:ff", "05 v_subrev_f32 f:ff",
        "06 v_mac_legacy_f32 f:fff", "07 v_mul_legacy_f32 f:ff", "08 v_mul_f32 f:ff", "09 v_mul_i32_i24 u:uu",
        "0a v_mul_hi_i32_i24 u:uu", "0b v_mul_u32_u24 u:uu", "0c v_mul_hi_u32_u24 u:uu", "0f v_min_f32 f:ff",
        "10 v_max_f32 f:ff", "11 v_min_i32 u:uu", "12 v_max_i32 u:uu", "13 v_min_u32 u:uu", "14 v_max_u32 u:uu",
        "16 v_lshrrev_b32 u:uu", "18 v_ashrrev_i32 u:uu", "1a v_lshlrev_b32 u:uu", "1b v_and_b32 u:uu",
        "1c v_or_b32 u:uu", "1d v_xor_b32 u:uu", "1e v_xnor_b32 u:uu", "1f v_mac_f32 f:fff", "20 v_madmk_f32 f:fff bare",
        "21 v_madak_f32 f:fff bare", "25 v_add_nc_u32 u:uu", "26 v_sub_nc_u32 u:uu", "27 v_subrev_nc_u32 u:uu",
        "28 v_add_co_ci_u32 u:uum", "29 v_sub_co_ci_u32 u:uum", "2a v_subrev_co_ci_u32 u:uum", "2b v_fmac_f32 f:fff",
        "2c v_fmamk_f32 f:fff bare", "2d v_fmaak_f32 f:fff bare", "2f v_cvt_pkrtz_f16_f32 u:ff", "32 v_add_f16 h:hh",
        "33 v_sub_f16 h:hh", "34 v_subrev_f16 h:hh", "35 v_mul_f16 h:hh", "36 v_fmac_f16 h:hhh",
        "37 v_fmamk_f16 h:hhh bare", "38 v_fmaak_f16 h:hhh bare", "39 v_max_f16 h:hh", "3a v_min_f16 h:hh",
        "3b v_ldexp_f16 h:hs", "3c v_pk_fmac_f16 u:HHH bare",
    ]);

    public static readonly FrozenDictionary<int, OpcodeInfo> Vop1 = OpcodeTable.Parse(
    [
        "00 v_nop : bare", "01 v_mov_b32 u:u", "02 v_readfirstlane_b32 u:u bare", "03 v_cvt_i32_f64 u:F",
        "04 v_cvt_f64_i32 F:u", "05 v_cvt_f32_i32 f:u", "06 v_cvt_f32_u32 f:u", "07 v_cvt_u32_f32 u:f",
        "08 v_cvt_i32_f32 u:f", "0a v_cvt_f16_f32 h:f", "0b v_cvt_f32_f16 f:h", "0c v_cvt_rpi_i32_f32 u:f",
        "0d v_cvt_flr_i32_f32 u:f", "0e v_cvt_off_f32_i4 f:u", "0f v_cvt_f32_f64 f:F", "10 v_cvt_f64_f32 F:f",
        "11 v_cvt_f32_ubyte0 f:u", "12 v_cvt_f32_ubyte1 f:u", "13 v_cvt_f32_ubyte2 f:u", "14 v_cvt_f32_ubyte3 f:u",
        "15 v_cvt_u32_f64 u:F", "16 v_cvt_f64_u32 F:u", "17 v_trunc_f64 F:F", "18 v_ceil_f64 F:F", "19 v_rndne_f64 F:F",
        "1a v_floor_f64 F:F", "1b v_pipeflush : bare", "20 v_fract_f32 f:f", "21 v_trunc_f32 f:f", "22 v_ceil_f32 f:f",
        "23 v_rndne_f32 f:f", "24 v_floor_f32 f:f", "25 v_exp_f32 f:f", "27 v_log_f32 f:f", "2a v_rcp_f32 f:f",
        "2b v_rcp_iflag_f32 f:f", "2e v_rsq_f32 f:f", "2f v_rcp_f64 F:F", "31 v_rsq_f64 F:F", "33 v_sqrt_f32 f:f",
        "34 v_sqrt_f64 F:F", "35 v_sin_f32 f:f", "36 v_cos_f32 f:f", "37 v_not_b32 u:u", "38 v_bfrev_b32 u:u",
        "39 v_ffbh_u32 u:u", "3a v_ffbl_b32 u:u", "3b v_ffbh_i32 u:u", "3c v_frexp_exp_i32_f64 u:F",
        "3d v_frexp_mant_f64 F:F", "3e v_fract_f64 F:F", "3f v_frexp_exp_i32_f32 u:f", "40 v_frexp_mant_f32 f:f",
        "41 v_clrexcp : bare", "42 v_movreld_b32 u:u", "43 v_movrels_b32 u:u", "44 v_movrelsd_b32 u:u",
        "48 v_movrelsd_2_b32 u:u", "50 v_cvt_f16_u16 h:s", "51 v_cvt_f16_i16 h:s", "52 v_cvt_u16_f16 s:h",
        "53 v_cvt_i16_f16 s:h", "54 v_rcp_f16 h:h", "55 v_sqrt_f16 h:h", "56 v_rsq_f16 h:h", "57 v_log_f16 h:h",
        "58 v_exp_f16 h:h", "59 v_frexp_mant_f16 h:h", "5a v_frexp_exp_i16_f16 s:h", "5b v_floor_f16 h:h",
        "5c v_ceil_f16 h:h", "5d v_trunc_f16 h:h", "5e v_rndne_f16 h:h", "5f v_fract_f16 h:h", "60 v_sin_f16 h:h",
        "61 v_cos_f16 h:h", "62 v_sat_pk_u8_i16 u:u", "63 v_cvt_norm_i16_f16 s:h", "64 v_cvt_norm_u16_f16 s:h",
        "65 v_swap_b32 u:u bare", "68 v_swaprel_b32 u:u bare",
    ]);

    /// <summary>Opcodes that exist only in the VOP3 encoding; carry-out forms are listed in <see cref="CarryOut"/>.</summary>
    public static readonly FrozenDictionary<int, OpcodeInfo> Vop3Only = OpcodeTable.Parse(
    [
        "140 v_mad_legacy_f32 f:fff", "141 v_mad_f32 f:fff", "142 v_mad_i32_i24 u:uuu", "143 v_mad_u32_u24 u:uuu",
        "144 v_cubeid_f32 f:fff", "145 v_cubesc_f32 f:fff", "146 v_cubetc_f32 f:fff", "147 v_cubema_f32 f:fff",
        "148 v_bfe_u32 u:uuu", "149 v_bfe_i32 u:uuu", "14a v_bfi_b32 u:uuu", "14b v_fma_f32 f:fff", "14c v_fma_f64 F:FFF",
        "14d v_lerp_u8 u:uuu", "14e v_alignbit_b32 u:uuu", "14f v_alignbyte_b32 u:uuu", "150 v_mullit_f32 f:fff",
        "151 v_min3_f32 f:fff", "152 v_min3_i32 u:uuu", "153 v_min3_u32 u:uuu", "154 v_max3_f32 f:fff",
        "155 v_max3_i32 u:uuu", "156 v_max3_u32 u:uuu", "157 v_med3_f32 f:fff", "158 v_med3_i32 u:uuu",
        "159 v_med3_u32 u:uuu", "15a v_sad_u8 u:uuu", "15b v_sad_hi_u8 u:uuu", "15c v_sad_u16 u:uuu",
        "15d v_sad_u32 u:uuu", "15e v_cvt_pk_u8_f32 u:fuu", "15f v_div_fixup_f32 f:fff", "160 v_div_fixup_f64 F:FFF",
        "164 v_add_f64 F:FF", "165 v_mul_f64 F:FF", "166 v_min_f64 F:FF", "167 v_max_f64 F:FF", "168 v_ldexp_f64 F:Fu",
        "169 v_mul_lo_u32 u:uu", "16a v_mul_hi_u32 u:uu", "16c v_mul_hi_i32 u:uu", "16f v_div_fmas_f32 f:fff",
        "170 v_div_fmas_f64 F:FFF", "171 v_msad_u8 u:uuu", "172 v_qsad_pk_u16_u8 U:UuU", "173 v_mqsad_pk_u16_u8 U:UuU",
        "174 v_trig_preop_f64 F:Fu", "175 v_mqsad_u32_u8 Q:UuQ", "2ff v_lshlrev_b64 U:uU", "300 v_lshrrev_b64 U:uU",
        "301 v_ashrrev_i64 U:uU", "303 v_add_nc_u16 s:ss", "304 v_sub_nc_u16 s:ss", "305 v_mul_lo_u16 s:ss",
        "307 v_lshrrev_b16 s:ss", "308 v_ashrrev_i16 s:ss", "309 v_max_u16 s:ss", "30a v_max_i16 s:ss",
        "30b v_min_u16 s:ss", "30c v_min_i16 s:ss", "30d v_add_nc_i16 s:ss", "30e v_sub_nc_i16 s:ss",
        "311 v_pack_b32_f16 u:hh", "312 v_cvt_pknorm_i16_f16 u:hh", "313 v_cvt_pknorm_u16_f16 u:hh",
        "314 v_lshlrev_b16 s:ss", "340 v_mad_u16 s:sss", "344 v_perm_b32 u:uuu", "345 v_xad_u32 u:uuu",
        "346 v_lshl_add_u32 u:uuu", "347 v_add_lshl_u32 u:uuu", "34b v_fma_f16 h:hhh", "351 v_min3_f16 h:hhh",
        "352 v_min3_i16 s:sss", "353 v_min3_u16 s:sss", "354 v_max3_f16 h:hhh", "355 v_max3_i16 s:sss",
        "356 v_max3_u16 s:sss", "357 v_med3_f16 h:hhh", "358 v_med3_i16 s:sss", "359 v_med3_u16 s:sss",
        "35e v_mad_i16 s:sss", "35f v_div_fixup_f16 h:hhh", "360 v_readlane_b32 u:uu", "361 v_writelane_b32 u:uu",
        "362 v_ldexp_f32 f:fu", "363 v_bfm_b32 u:uu", "364 v_bcnt_u32_b32 u:uu", "365 v_mbcnt_lo_u32_b32 u:uu",
        "366 v_mbcnt_hi_u32_b32 u:uu", "368 v_cvt_pknorm_i16_f32 u:ff", "369 v_cvt_pknorm_u16_f32 u:ff",
        "36a v_cvt_pk_u16_u32 u:uu", "36b v_cvt_pk_i16_i32 u:uu", "36d v_add3_u32 u:uuu", "36f v_lshl_or_b32 u:uuu",
        "371 v_and_or_b32 u:uuu", "372 v_or3_b32 u:uuu", "178 v_xor3_b32 u:uuu", "373 v_mad_u32_u16 u:ssu", "375 v_mad_i32_i16 u:ssu",
        "376 v_sub_nc_i32 u:uu", "377 v_permlane16_b32 u:uuu", "378 v_permlanex16_b32 u:uuu", "37f v_add_nc_i32 u:uu",
    ]);

    /// <summary>VOP3 opcodes with a scalar carry-out or condition destination (the VOP3b layout).</summary>
    public static readonly FrozenDictionary<int, OpcodeInfo> CarryOut = OpcodeTable.Parse(
    [
        "128 v_add_co_ci_u32 u:uum", "129 v_sub_co_ci_u32 u:uum", "12a v_subrev_co_ci_u32 u:uum",
        "16d v_div_scale_f32 f:fff", "16e v_div_scale_f64 F:FFF", "176 v_mad_u64_u32 U:uuU", "177 v_mad_i64_i32 U:uuU",
        "30f v_add_co_u32 u:uu", "310 v_sub_co_u32 u:uu", "319 v_subrev_co_u32 u:uu",
    ]);

    public static readonly FrozenDictionary<int, OpcodeInfo> Vop3P = OpcodeTable.Parse(
    [
        "00 v_pk_mad_i16 S:SSS", "01 v_pk_mul_lo_u16 S:SS", "02 v_pk_add_i16 S:SS", "03 v_pk_sub_i16 S:SS",
        "04 v_pk_lshlrev_b16 S:SS", "05 v_pk_lshrrev_b16 S:SS", "06 v_pk_ashrrev_i16 S:SS", "07 v_pk_max_i16 S:SS",
        "08 v_pk_min_i16 S:SS", "09 v_pk_mad_u16 S:SSS", "0a v_pk_add_u16 S:SS", "0b v_pk_sub_u16 S:SS",
        "0c v_pk_max_u16 S:SS", "0d v_pk_min_u16 S:SS", "0e v_pk_fma_f16 H:HHH", "0f v_pk_add_f16 H:HH",
        "10 v_pk_mul_f16 H:HH", "11 v_pk_min_f16 H:HH", "12 v_pk_max_f16 H:HH", "20 v_fma_mix_f32 f:fff",
        "21 v_fma_mixlo_f16 f:fff", "22 v_fma_mixhi_f16 f:fff",
    ]);
}
