// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// Scalar ALU opcodes (SOP2, SOP1, SOPC, SOPK, SOPP) with their numbers from the opcode tables of the AMD "RDNA 2
/// Instruction Set Architecture" reference guide, chapter 13.
/// </summary>
internal static class ScalarOpcodes
{
    public static readonly FrozenDictionary<int, OpcodeInfo> Sop2 = OpcodeTable.Parse(
    [
        "00 s_add_u32 u:uu", "01 s_sub_u32 u:uu", "02 s_add_i32 u:uu", "03 s_sub_i32 u:uu", "04 s_addc_u32 u:uu",
        "05 s_subb_u32 u:uu", "06 s_min_i32 u:uu", "07 s_min_u32 u:uu", "08 s_max_i32 u:uu", "09 s_max_u32 u:uu",
        "0a s_cselect_b32 u:uu", "0b s_cselect_b64 U:UU", "0e s_and_b32 u:uu", "0f s_and_b64 U:UU", "10 s_or_b32 u:uu",
        "11 s_or_b64 U:UU", "12 s_xor_b32 u:uu", "13 s_xor_b64 U:UU", "14 s_andn2_b32 u:uu", "15 s_andn2_b64 U:UU",
        "16 s_orn2_b32 u:uu", "17 s_orn2_b64 U:UU", "18 s_nand_b32 u:uu", "19 s_nand_b64 U:UU", "1a s_nor_b32 u:uu",
        "1b s_nor_b64 U:UU", "1c s_xnor_b32 u:uu", "1d s_xnor_b64 U:UU", "1e s_lshl_b32 u:uu", "1f s_lshl_b64 U:Uu",
        "20 s_lshr_b32 u:uu", "21 s_lshr_b64 U:Uu", "22 s_ashr_i32 u:uu", "23 s_ashr_i64 U:Uu", "24 s_bfm_b32 u:uu",
        "25 s_bfm_b64 U:uu", "26 s_mul_i32 u:uu", "27 s_bfe_u32 u:uu", "28 s_bfe_i32 u:uu", "29 s_bfe_u64 U:Uu",
        "2a s_bfe_i64 U:Uu", "2c s_absdiff_i32 u:uu", "2e s_lshl1_add_u32 u:uu", "2f s_lshl2_add_u32 u:uu",
        "30 s_lshl3_add_u32 u:uu", "31 s_lshl4_add_u32 u:uu", "32 s_pack_ll_b32_b16 u:uu", "33 s_pack_lh_b32_b16 u:uu",
        "34 s_pack_hh_b32_b16 u:uu", "35 s_mul_hi_u32 u:uu", "36 s_mul_hi_i32 u:uu",
    ]);

    public static readonly FrozenDictionary<int, OpcodeInfo> Sop1 = OpcodeTable.Parse(
    [
        "03 s_mov_b32 u:u", "04 s_mov_b64 U:U", "05 s_cmov_b32 u:u", "06 s_cmov_b64 U:U", "07 s_not_b32 u:u",
        "08 s_not_b64 U:U", "09 s_wqm_b32 u:u", "0a s_wqm_b64 U:U", "0b s_brev_b32 u:u", "0c s_brev_b64 U:U",
        "0d s_bcnt0_i32_b32 u:u", "0e s_bcnt0_i32_b64 u:U", "0f s_bcnt1_i32_b32 u:u", "10 s_bcnt1_i32_b64 u:U",
        "11 s_ff0_i32_b32 u:u", "12 s_ff0_i32_b64 u:U", "13 s_ff1_i32_b32 u:u", "14 s_ff1_i32_b64 u:U",
        "15 s_flbit_i32_b32 u:u", "16 s_flbit_i32_b64 u:U", "17 s_flbit_i32 u:u", "18 s_flbit_i32_i64 u:U",
        "19 s_sext_i32_i8 u:u", "1a s_sext_i32_i16 u:u", "1b s_bitset0_b32 u:u", "1c s_bitset0_b64 U:u",
        "1d s_bitset1_b32 u:u", "1e s_bitset1_b64 U:u", "1f s_getpc_b64 U:", "20 s_setpc_b64 :U", "21 s_swappc_b64 U:U",
        "22 s_rfe_b64 :U", "24 s_and_saveexec_b64 U:U", "25 s_or_saveexec_b64 U:U", "26 s_xor_saveexec_b64 U:U",
        "27 s_andn2_saveexec_b64 U:U", "28 s_orn2_saveexec_b64 U:U", "29 s_nand_saveexec_b64 U:U",
        "2a s_nor_saveexec_b64 U:U", "2b s_xnor_saveexec_b64 U:U", "2c s_quadmask_b32 u:u", "2d s_quadmask_b64 U:U",
        "2e s_movrels_b32 u:u", "2f s_movrels_b64 U:U", "30 s_movreld_b32 u:u", "31 s_movreld_b64 U:U",
        "34 s_abs_i32 u:u", "37 s_andn1_saveexec_b64 U:U", "38 s_orn1_saveexec_b64 U:U", "39 s_andn1_wrexec_b64 U:U",
        "3a s_andn2_wrexec_b64 U:U", "3b s_bitreplicate_b64_b32 U:u", "3c s_and_saveexec_b32 u:u",
        "3d s_or_saveexec_b32 u:u", "3e s_xor_saveexec_b32 u:u", "3f s_andn2_saveexec_b32 u:u",
        "40 s_orn2_saveexec_b32 u:u", "41 s_nand_saveexec_b32 u:u", "42 s_nor_saveexec_b32 u:u",
        "43 s_xnor_saveexec_b32 u:u", "44 s_andn1_saveexec_b32 u:u", "45 s_orn1_saveexec_b32 u:u",
        "46 s_andn1_wrexec_b32 u:u", "47 s_andn2_wrexec_b32 u:u", "49 s_movrelsd_2_b32 u:u",
    ]);

    public static readonly FrozenDictionary<int, OpcodeInfo> Sopc = OpcodeTable.Parse(
    [
        "00 s_cmp_eq_i32 :uu", "01 s_cmp_lg_i32 :uu", "02 s_cmp_gt_i32 :uu", "03 s_cmp_ge_i32 :uu", "04 s_cmp_lt_i32 :uu",
        "05 s_cmp_le_i32 :uu", "06 s_cmp_eq_u32 :uu", "07 s_cmp_lg_u32 :uu", "08 s_cmp_gt_u32 :uu", "09 s_cmp_ge_u32 :uu",
        "0a s_cmp_lt_u32 :uu", "0b s_cmp_le_u32 :uu", "0c s_bitcmp0_b32 :uu", "0d s_bitcmp1_b32 :uu",
        "0e s_bitcmp0_b64 :Uu", "0f s_bitcmp1_b64 :Uu", "12 s_cmp_eq_u64 :UU", "13 s_cmp_lg_u64 :UU",
    ]);

    /// <summary>SOPK opcodes; the decoder gives each its operand form by name.</summary>
    public static readonly FrozenDictionary<int, string> Sopk = new Dictionary<int, string>
    {
        [0x00] = "s_movk_i32", [0x01] = "s_version", [0x02] = "s_cmovk_i32", [0x03] = "s_cmpk_eq_i32",
        [0x04] = "s_cmpk_lg_i32", [0x05] = "s_cmpk_gt_i32", [0x06] = "s_cmpk_ge_i32", [0x07] = "s_cmpk_lt_i32",
        [0x08] = "s_cmpk_le_i32", [0x09] = "s_cmpk_eq_u32", [0x0a] = "s_cmpk_lg_u32", [0x0b] = "s_cmpk_gt_u32",
        [0x0c] = "s_cmpk_ge_u32", [0x0d] = "s_cmpk_lt_u32", [0x0e] = "s_cmpk_le_u32", [0x0f] = "s_addk_i32",
        [0x10] = "s_mulk_i32", [0x12] = "s_getreg_b32", [0x13] = "s_setreg_b32", [0x15] = "s_setreg_imm32_b32",
        [0x16] = "s_call_b64", [0x17] = "s_waitcnt_vscnt", [0x18] = "s_waitcnt_vmcnt", [0x19] = "s_waitcnt_expcnt",
        [0x1a] = "s_waitcnt_lgkmcnt", [0x1b] = "s_subvector_loop_begin", [0x1c] = "s_subvector_loop_end",
    }.ToFrozenDictionary();

    /// <summary>SOPP opcodes; the decoder gives each its operand form by name.</summary>
    public static readonly FrozenDictionary<int, string> Sopp = new Dictionary<int, string>
    {
        [0x00] = "s_nop", [0x01] = "s_endpgm", [0x02] = "s_branch", [0x03] = "s_wakeup", [0x04] = "s_cbranch_scc0",
        [0x05] = "s_cbranch_scc1", [0x06] = "s_cbranch_vccz", [0x07] = "s_cbranch_vccnz", [0x08] = "s_cbranch_execz",
        [0x09] = "s_cbranch_execnz", [0x0a] = "s_barrier", [0x0b] = "s_setkill", [0x0c] = "s_waitcnt",
        [0x0d] = "s_sethalt", [0x0e] = "s_sleep", [0x0f] = "s_setprio", [0x10] = "s_sendmsg", [0x11] = "s_sendmsghalt",
        [0x12] = "s_trap", [0x13] = "s_icache_inv", [0x14] = "s_incperflevel", [0x15] = "s_decperflevel",
        [0x16] = "s_ttracedata", [0x17] = "s_cbranch_cdbgsys", [0x18] = "s_cbranch_cdbguser",
        [0x19] = "s_cbranch_cdbgsys_or_user", [0x1a] = "s_cbranch_cdbgsys_and_user", [0x1b] = "s_endpgm_saved",
        [0x1e] = "s_endpgm_ordered_ps_done", [0x1f] = "s_code_end", [0x20] = "s_inst_prefetch", [0x21] = "s_clause",
        [0x23] = "s_waitcnt_depctr", [0x24] = "s_round_mode", [0x25] = "s_denorm_mode", [0x28] = "s_ttracedata_imm",
    }.ToFrozenDictionary();
}
