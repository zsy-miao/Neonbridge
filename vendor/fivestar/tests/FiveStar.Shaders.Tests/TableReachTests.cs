// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>How far programs read past the table pointers their user SGPRs hold.</summary>
public sealed class TableReachTests
{
    [Fact]
    public void The_composite_vertex_program_reaches_record_41_of_its_table_in_s9()
    {
        // VS_PassthroughComposite_97148495's prologue: the T# at 0x380 and the constant buffer's V# at 0x520 of the
        // table in s9, through the pair (s9, 0) moved into s[8:9].
        string[] program =
        [
            "s_swappc_b64 s[4:5], s[4:5]",
            "s_mov_b32 s8, s9",
            "s_mov_b32 s9, 0",
            "s_load_dwordx8 s[0:7], s[8:9], 0x380",
            "image_load v[0:1], v8, s[0:7] dmask:0x3 dim:SQ_RSRC_IMG_2D unorm a16",
            "s_load_dwordx4 s[8:11], s[8:9], 0x520",
            "s_buffer_load_dwordx8 s[0:7], s[8:11], null",
            "s_endpgm",
        ];

        var reach = TableReach.Find(Instructions(program));

        Assert.Equal(new Dictionary<int, uint> { [9] = 0x530 }, reach);
    }

    [Fact]
    public void A_pointer_with_an_unknown_high_dword_or_one_overwritten_reaches_nothing()
    {
        string[] program =
        [
            "s_load_dwordx4 s[0:3], s[8:9], 0x40",
            "s_mov_b32 s11, 0",
            "s_mov_b32 s10, s12",
            "s_add_u32 s10, s10, 0x100",
            "s_load_dwordx4 s[4:7], s[10:11], 0x80",
            "s_mov_b32 s15, 0",
            "s_load_dword s16, s[14:15], 0x10",
            "s_endpgm",
        ];

        var reach = TableReach.Find(Instructions(program));

        // s[8:9]'s high dword is the entry s9, not 0; s10 was changed after the copy; s14 is read as it entered.
        Assert.Equal(new Dictionary<int, uint> { [14] = 0x14 }, reach);
    }

    [Fact]
    public void A_load_at_a_computed_offset_reaches_the_whole_table()
    {
        // TL_PS_PBR_MATERIAL_STANDARD: a probe's T# at the table in s14 plus 32 x an index it computed.
        string[] program =
        [
            "s_mov_b32 s15, 0",
            "s_lshl_b32 s1, s2, 5",
            "s_load_dwordx8 s[16:23], s[14:15], s1",
            "s_endpgm",
        ];

        Assert.Equal(new Dictionary<int, uint> { [14] = TableReach.DynamicReach }, TableReach.Find(Instructions(program)));
    }

    private static List<Instruction> Instructions(string[] lines)
    {
        return [.. lines.Select((text, index) => new Instruction(index * 8, 8, text))];
    }
}
