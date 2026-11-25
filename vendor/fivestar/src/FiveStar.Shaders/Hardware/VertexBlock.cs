// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>Decodes the vertex-shader block of an FSSM header (hardware stage 4).</summary>
internal static class VertexBlock
{
    private const int VgprComponentCountField = 0x2BC;
    private const int ParamExportsField = 0x29C;
    private const int FetchVgprBaseField = 0xD8;

    // Flags @0xD4: bit 2 = the program calls a fetch shader, bits 24..31 = SGPR pair holding its PC.
    private const uint FetchShaderFlag = 0x4;
    private const int FetchPcShift = 24;

    // Fetch table @0x138 .. 0x1D8: 5-byte entries {type, attribute, mask, first VGPR, components - 1}; type 0 ends it.
    private const int FetchTableStart = 0x138;
    private const int FetchTableEnd = 0x1D8;
    private const int FetchEntrySize = 5;
    private const int AttributeField = 1;
    private const int MaskField = 2;
    private const int VgprField = 3;
    private const int ComponentsField = 4;

    public static VertexConfig Decode(ReadOnlySpan<byte> program, uint flags)
    {
        var fetch = (flags & FetchShaderFlag) != 0;
        var config = new VertexConfig
        {
            VgprComponentCount = (int)FssmHeader.U32(program, VgprComponentCountField),
            ParamExports = (int)FssmHeader.U32(program, ParamExportsField),
            FetchShader = fetch,
            FetchInputs = [],
        };

        if (!fetch)
        {
            return config;
        }

        return config with
        {
            FetchPcSgpr = (int)(flags >> FetchPcShift),
            FetchVgprBase = (int)FssmHeader.U32(program, FetchVgprBaseField),
            FetchInputs = FetchTable(program[FetchTableStart..FetchTableEnd]),
        };
    }

    private static List<FetchInput> FetchTable(ReadOnlySpan<byte> table)
    {
        var inputs = new List<FetchInput>();

        for (var at = 0; at + ComponentsField < table.Length; at += FetchEntrySize)
        {
            var type = table[at];

            if (type == 0)
            {
                break;
            }

            var entry = new FetchInput(
                type, table[at + AttributeField], table[at + MaskField], table[at + VgprField], table[at + ComponentsField] + 1);
            inputs.Add(entry);
        }

        return inputs;
    }
}
