// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// Memory opcodes (SMEM, MUBUF, MTBUF, FLAT, MIMG) with their numbers from the opcode tables of the AMD "RDNA 2
/// Instruction Set Architecture" reference guide, chapter 13. Buffer and flat entries carry the data dwords;
/// <see cref="LdsOpcodes"/> holds DS.
/// </summary>
internal static class MemoryOpcodes
{
    private const int KindField = 2;

    /// <summary>
    /// SMEM: opcode to name, operand kind and data dwords. Kinds: <c>l</c> data, base and offset; <c>n</c> no operands;
    /// <c>t</c> data only; <c>p</c> probe (the data field as a number), base and offset; <c>d</c> base and offset.
    /// </summary>
    public static readonly FrozenDictionary<int, (string Name, char Kind, int Dwords)> ScalarMemory = ParseKinds(
    [
        "00 s_load_dword l 1", "01 s_load_dwordx2 l 2", "02 s_load_dwordx4 l 4", "03 s_load_dwordx8 l 8",
        "04 s_load_dwordx16 l 16", "05 s_scratch_load_dword l 1", "06 s_scratch_load_dwordx2 l 2",
        "07 s_scratch_load_dwordx4 l 4", "08 s_buffer_load_dword l 1", "09 s_buffer_load_dwordx2 l 2",
        "0a s_buffer_load_dwordx4 l 4", "0b s_buffer_load_dwordx8 l 8", "0c s_buffer_load_dwordx16 l 16",
        "10 s_store_dword l 1", "11 s_store_dwordx2 l 2", "12 s_store_dwordx4 l 4", "15 s_scratch_store_dword l 1",
        "16 s_scratch_store_dwordx2 l 2", "17 s_scratch_store_dwordx4 l 4", "18 s_buffer_store_dword l 1",
        "19 s_buffer_store_dwordx2 l 2", "1a s_buffer_store_dwordx4 l 4", "1f s_gl1_inv n 0", "20 s_dcache_inv n 0",
        "21 s_dcache_wb n 0", "24 s_memtime t 2", "25 s_memrealtime t 2", "26 s_atc_probe p 0",
        "27 s_atc_probe_buffer p 0", "28 s_dcache_discard d 0", "29 s_dcache_discard_x2 d 0",
        "2a s_get_waveid_in_workgroup t 1", "40 s_buffer_atomic_swap l 1", "41 s_buffer_atomic_cmpswap l 2",
        "42 s_buffer_atomic_add l 1", "43 s_buffer_atomic_sub l 1", "44 s_buffer_atomic_smin l 1",
        "45 s_buffer_atomic_umin l 1", "46 s_buffer_atomic_smax l 1", "47 s_buffer_atomic_umax l 1",
        "48 s_buffer_atomic_and l 1", "49 s_buffer_atomic_or l 1", "4a s_buffer_atomic_xor l 1",
        "4b s_buffer_atomic_inc l 1", "4c s_buffer_atomic_dec l 1", "60 s_buffer_atomic_swap_x2 l 2",
        "61 s_buffer_atomic_cmpswap_x2 l 4", "62 s_buffer_atomic_add_x2 l 2", "63 s_buffer_atomic_sub_x2 l 2",
        "64 s_buffer_atomic_smin_x2 l 2", "65 s_buffer_atomic_umin_x2 l 2", "66 s_buffer_atomic_smax_x2 l 2",
        "67 s_buffer_atomic_umax_x2 l 2", "68 s_buffer_atomic_and_x2 l 2", "69 s_buffer_atomic_or_x2 l 2",
        "6a s_buffer_atomic_xor_x2 l 2", "6b s_buffer_atomic_inc_x2 l 2", "6c s_buffer_atomic_dec_x2 l 2",
        "80 s_atomic_swap l 1", "81 s_atomic_cmpswap l 2", "82 s_atomic_add l 1", "83 s_atomic_sub l 1",
        "84 s_atomic_smin l 1", "85 s_atomic_umin l 1", "86 s_atomic_smax l 1", "87 s_atomic_umax l 1",
        "88 s_atomic_and l 1", "89 s_atomic_or l 1", "8a s_atomic_xor l 1", "8b s_atomic_inc l 1", "8c s_atomic_dec l 1",
        "a0 s_atomic_swap_x2 l 2", "a1 s_atomic_cmpswap_x2 l 4", "a2 s_atomic_add_x2 l 2", "a3 s_atomic_sub_x2 l 2",
        "a4 s_atomic_smin_x2 l 2", "a5 s_atomic_umin_x2 l 2", "a6 s_atomic_smax_x2 l 2", "a7 s_atomic_umax_x2 l 2",
        "a8 s_atomic_and_x2 l 2", "a9 s_atomic_or_x2 l 2", "aa s_atomic_xor_x2 l 2", "ab s_atomic_inc_x2 l 2",
        "ac s_atomic_dec_x2 l 2",
    ]);

    /// <summary>MUBUF: opcode to "name dwords".</summary>
    public static readonly FrozenDictionary<int, (string Name, int Dwords)> Buffer = Parse(
    [
        "00 buffer_load_format_x 1", "01 buffer_load_format_xy 2", "02 buffer_load_format_xyz 3",
        "03 buffer_load_format_xyzw 4", "04 buffer_store_format_x 1", "05 buffer_store_format_xy 2",
        "06 buffer_store_format_xyz 3", "07 buffer_store_format_xyzw 4", "08 buffer_load_ubyte 1",
        "09 buffer_load_sbyte 1", "0a buffer_load_ushort 1", "0b buffer_load_sshort 1", "0c buffer_load_dword 1",
        "0d buffer_load_dwordx2 2", "0e buffer_load_dwordx4 4", "0f buffer_load_dwordx3 3", "18 buffer_store_byte 1",
        "19 buffer_store_byte_d16_hi 1", "1a buffer_store_short 1", "1b buffer_store_short_d16_hi 1",
        "1c buffer_store_dword 1", "1d buffer_store_dwordx2 2", "1e buffer_store_dwordx4 4", "1f buffer_store_dwordx3 3",
        "20 buffer_load_ubyte_d16 1", "21 buffer_load_ubyte_d16_hi 1", "22 buffer_load_sbyte_d16 1",
        "23 buffer_load_sbyte_d16_hi 1", "24 buffer_load_short_d16 1", "25 buffer_load_short_d16_hi 1",
        "30 buffer_atomic_swap 1",
        "31 buffer_atomic_cmpswap 2", "32 buffer_atomic_add 1", "33 buffer_atomic_sub 1", "35 buffer_atomic_smin 1",
        "36 buffer_atomic_umin 1", "37 buffer_atomic_smax 1", "38 buffer_atomic_umax 1", "39 buffer_atomic_and 1",
        "3a buffer_atomic_or 1", "3b buffer_atomic_xor 1", "3c buffer_atomic_inc 1", "3d buffer_atomic_dec 1",
        "3e buffer_atomic_fcmpswap 2", "3f buffer_atomic_fmin 1", "40 buffer_atomic_fmax 1", "50 buffer_atomic_swap_x2 2",
        "51 buffer_atomic_cmpswap_x2 4", "52 buffer_atomic_add_x2 2", "53 buffer_atomic_sub_x2 2",
        "55 buffer_atomic_smin_x2 2", "56 buffer_atomic_umin_x2 2", "57 buffer_atomic_smax_x2 2",
        "58 buffer_atomic_umax_x2 2", "59 buffer_atomic_and_x2 2", "5a buffer_atomic_or_x2 2", "5b buffer_atomic_xor_x2 2",
        "5c buffer_atomic_inc_x2 2", "5d buffer_atomic_dec_x2 2", "5e buffer_atomic_fcmpswap_x2 4",
        "5f buffer_atomic_fmin_x2 2", "60 buffer_atomic_fmax_x2 2", "71 buffer_gl0_inv 0", "72 buffer_gl1_inv 0",
        "80 buffer_load_format_d16_x 1", "81 buffer_load_format_d16_xy 1", "82 buffer_load_format_d16_xyz 2",
        "83 buffer_load_format_d16_xyzw 2", "84 buffer_store_format_d16_x 1", "85 buffer_store_format_d16_xy 1",
        "86 buffer_store_format_d16_xyz 2", "87 buffer_store_format_d16_xyzw 2",
    ]);

    /// <summary>MTBUF: opcode to "name dwords".</summary>
    public static readonly FrozenDictionary<int, (string Name, int Dwords)> TypedBuffer = Parse(
    [
        "0 tbuffer_load_format_x 1", "1 tbuffer_load_format_xy 2", "2 tbuffer_load_format_xyz 3",
        "3 tbuffer_load_format_xyzw 4", "4 tbuffer_store_format_x 1", "5 tbuffer_store_format_xy 2",
        "6 tbuffer_store_format_xyz 3", "7 tbuffer_store_format_xyzw 4", "8 tbuffer_load_format_d16_x 1",
        "9 tbuffer_load_format_d16_xy 1", "a tbuffer_load_format_d16_xyz 2", "b tbuffer_load_format_d16_xyzw 2",
        "c tbuffer_store_format_d16_x 1", "d tbuffer_store_format_d16_xy 1", "e tbuffer_store_format_d16_xyz 2",
        "f tbuffer_store_format_d16_xyzw 2",
    ]);

    /// <summary>FLAT, SCRATCH and GLOBAL share opcodes; the segment adds the prefix.</summary>
    public static readonly FrozenDictionary<int, (string Name, int Dwords)> Flat = Parse(
    [
        "08 load_ubyte 1", "09 load_sbyte 1", "0a load_ushort 1", "0b load_sshort 1", "0c load_dword 1",
        "0d load_dwordx2 2", "0e load_dwordx4 4", "0f load_dwordx3 3", "18 store_byte 1", "19 store_byte_d16_hi 1",
        "1a store_short 1", "1b store_short_d16_hi 1", "1c store_dword 1", "1d store_dwordx2 2", "1e store_dwordx4 4",
        "1f store_dwordx3 3", "20 load_ubyte_d16 1", "21 load_ubyte_d16_hi 1", "22 load_sbyte_d16 1",
        "23 load_sbyte_d16_hi 1", "24 load_short_d16 1", "25 load_short_d16_hi 1", "30 atomic_swap 1",
        "31 atomic_cmpswap 2", "32 atomic_add 1", "33 atomic_sub 1", "35 atomic_smin 1", "36 atomic_umin 1",
        "37 atomic_smax 1", "38 atomic_umax 1", "39 atomic_and 1", "3a atomic_or 1", "3b atomic_xor 1",
        "3c atomic_inc 1", "3d atomic_dec 1", "3e atomic_fcmpswap 2", "3f atomic_fmin 1", "40 atomic_fmax 1",
        "50 atomic_swap_x2 2", "51 atomic_cmpswap_x2 4", "52 atomic_add_x2 2", "53 atomic_sub_x2 2",
        "55 atomic_smin_x2 2", "56 atomic_umin_x2 2", "57 atomic_smax_x2 2", "58 atomic_umax_x2 2", "59 atomic_and_x2 2",
        "5a atomic_or_x2 2", "5b atomic_xor_x2 2", "5c atomic_inc_x2 2", "5d atomic_dec_x2 2",
        "5e atomic_fcmpswap_x2 4", "5f atomic_fmin_x2 2", "60 atomic_fmax_x2 2",
    ]);

    private static FrozenDictionary<int, (string Name, char Kind, int Dwords)> ParseKinds(IEnumerable<string> entries)
    {
        var table = new Dictionary<int, (string Name, char Kind, int Dwords)>();

        foreach (var entry in entries)
        {
            var fields = entry.Split(' ');
            var opcode = int.Parse(fields[0], NumberStyles.HexNumber, CultureInfo.InvariantCulture);
            table[opcode] = (fields[1], fields[KindField][0], int.Parse(fields[^1], CultureInfo.InvariantCulture));
        }

        return table.ToFrozenDictionary();
    }

    private static FrozenDictionary<int, (string Name, int Dwords)> Parse(IEnumerable<string> entries)
    {
        var table = new Dictionary<int, (string Name, int Dwords)>();

        foreach (var entry in entries)
        {
            var fields = entry.Split(' ');
            var opcode = int.Parse(fields[0], NumberStyles.HexNumber, CultureInfo.InvariantCulture);
            table[opcode] = (fields[1], int.Parse(fields[^1], CultureInfo.InvariantCulture));
        }

        return table.ToFrozenDictionary();
    }
}
