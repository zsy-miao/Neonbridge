// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// DS opcodes with their numbers from the opcode table of the AMD "RDNA 2 Instruction Set Architecture" reference
/// guide (DS instructions). Each entry is "opcode name shape dwords": shape <c>w</c> write, <c>W</c> write2, <c>r</c>
/// read, <c>R</c> read2, <c>a</c> atomic, <c>t</c> atomic with return, <c>c</c> compare-swap, <c>C</c> compare-swap with
/// return, <c>X</c> exchange2 with return, <c>p</c> permute, <c>s</c> swizzle, <c>n</c> append/consume, <c>o</c> nop.
/// </summary>
internal static class LdsOpcodes
{
    private const int OpcodeField = 0;
    private const int NameField = 1;
    private const int ShapeField = 2;
    private const int DwordsField = 3;

    public static readonly FrozenDictionary<int, (string Name, char Shape, int Dwords)> Table = Parse(
    [
        "00 ds_add_u32 a 1", "01 ds_sub_u32 a 1", "02 ds_rsub_u32 a 1", "03 ds_inc_u32 a 1", "04 ds_dec_u32 a 1",
        "05 ds_min_i32 a 1", "06 ds_max_i32 a 1", "07 ds_min_u32 a 1", "08 ds_max_u32 a 1", "09 ds_and_b32 a 1",
        "0a ds_or_b32 a 1", "0b ds_xor_b32 a 1", "0c ds_mskor_b32 c 1", "0d ds_write_b32 w 1", "0e ds_write2_b32 W 1",
        "0f ds_write2st64_b32 W 1", "10 ds_cmpst_b32 c 1", "11 ds_cmpst_f32 c 1", "12 ds_min_f32 a 1", "13 ds_max_f32 a 1",
        "14 ds_nop o 1", "15 ds_add_f32 a 1", "1e ds_write_b8 w 1", "1f ds_write_b16 w 1", "20 ds_add_rtn_u32 t 1",
        "21 ds_sub_rtn_u32 t 1", "22 ds_rsub_rtn_u32 t 1", "23 ds_inc_rtn_u32 t 1", "24 ds_dec_rtn_u32 t 1",
        "25 ds_min_rtn_i32 t 1", "26 ds_max_rtn_i32 t 1", "27 ds_min_rtn_u32 t 1", "28 ds_max_rtn_u32 t 1",
        "29 ds_and_rtn_b32 t 1", "2a ds_or_rtn_b32 t 1", "2b ds_xor_rtn_b32 t 1", "2c ds_mskor_rtn_b32 C 1",
        "2d ds_wrxchg_rtn_b32 t 1", "2e ds_wrxchg2_rtn_b32 X 1", "2f ds_wrxchg2st64_rtn_b32 X 1", "30 ds_cmpst_rtn_b32 C 1",
        "31 ds_cmpst_rtn_f32 C 1", "32 ds_min_rtn_f32 t 1", "33 ds_max_rtn_f32 t 1", "34 ds_wrap_rtn_b32 C 1",
        "35 ds_swizzle_b32 s 1", "36 ds_read_b32 r 1", "37 ds_read2_b32 R 1", "38 ds_read2st64_b32 R 1",
        "39 ds_read_i8 r 1", "3a ds_read_u8 r 1", "3b ds_read_i16 r 1", "3c ds_read_u16 r 1", "3d ds_consume n 1",
        "3e ds_append n 1", "40 ds_add_u64 a 2", "41 ds_sub_u64 a 2", "42 ds_rsub_u64 a 2", "43 ds_inc_u64 a 2",
        "44 ds_dec_u64 a 2", "45 ds_min_i64 a 2", "46 ds_max_i64 a 2", "47 ds_min_u64 a 2", "48 ds_max_u64 a 2",
        "49 ds_and_b64 a 2", "4a ds_or_b64 a 2", "4b ds_xor_b64 a 2", "4c ds_mskor_b64 c 2", "4d ds_write_b64 w 2",
        "4e ds_write2_b64 W 2", "4f ds_write2st64_b64 W 2", "50 ds_cmpst_b64 c 2", "51 ds_cmpst_f64 c 2",
        "52 ds_min_f64 a 2", "53 ds_max_f64 a 2", "60 ds_add_rtn_u64 t 2", "61 ds_sub_rtn_u64 t 2",
        "62 ds_rsub_rtn_u64 t 2", "63 ds_inc_rtn_u64 t 2", "64 ds_dec_rtn_u64 t 2", "65 ds_min_rtn_i64 t 2",
        "66 ds_max_rtn_i64 t 2", "67 ds_min_rtn_u64 t 2", "68 ds_max_rtn_u64 t 2", "69 ds_and_rtn_b64 t 2",
        "6a ds_or_rtn_b64 t 2", "6b ds_xor_rtn_b64 t 2", "6c ds_mskor_rtn_b64 C 2", "6d ds_wrxchg_rtn_b64 t 2",
        "6e ds_wrxchg2_rtn_b64 X 2", "6f ds_wrxchg2st64_rtn_b64 X 2", "70 ds_cmpst_rtn_b64 C 2", "71 ds_cmpst_rtn_f64 C 2",
        "72 ds_min_rtn_f64 t 2", "73 ds_max_rtn_f64 t 2", "76 ds_read_b64 r 2", "77 ds_read2_b64 R 2",
        "78 ds_read2st64_b64 R 2", "a0 ds_write_b8_d16_hi w 1", "a1 ds_write_b16_d16_hi w 1", "a2 ds_read_u8_d16 r 1",
        "a3 ds_read_u8_d16_hi r 1", "a4 ds_read_i8_d16 r 1", "a5 ds_read_i8_d16_hi r 1", "a6 ds_read_u16_d16 r 1",
        "a7 ds_read_u16_d16_hi r 1", "b2 ds_permute_b32 p 1", "b3 ds_bpermute_b32 p 1", "de ds_write_b96 w 3",
        "df ds_write_b128 w 4", "fe ds_read_b96 r 3", "ff ds_read_b128 r 4",
    ]);

    private static FrozenDictionary<int, (string Name, char Shape, int Dwords)> Parse(IEnumerable<string> entries)
    {
        var table = new Dictionary<int, (string Name, char Shape, int Dwords)>();

        foreach (var entry in entries)
        {
            var fields = entry.Split(' ');
            var opcode = int.Parse(fields[OpcodeField], NumberStyles.HexNumber, CultureInfo.InvariantCulture);
            table[opcode] = (fields[NameField], fields[ShapeField][0], int.Parse(fields[DwordsField], CultureInfo.InvariantCulture));
        }

        return table.ToFrozenDictionary();
    }
}
