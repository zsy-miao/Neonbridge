// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The VOPC opcodes, built from the layout of the AMD "RDNA 2 Instruction Set Architecture" reference guide (VOPC
/// opcode table): rows of 16 float or 8 integer conditions per type, each followed by its <c>v_cmpx</c> row.
/// </summary>
internal static class CompareOpcodes
{
    private const int FloatRow = 16;
    private const int CompareSources = 2;
    private const int IntegerRow = 8;
    private const int CompareX = 0x10;
    private const int Float32Row = 0x00;
    private const int Float64Row = 0x20;
    private const int Int32Row = 0x80;
    private const int Int64Row = 0xA0;
    private const int Uint32Row = 0xC0;
    private const int Uint64Row = 0xE0;
    private const int ClassFloat32 = 0x88;
    private const int ClassFloat16 = 0x8F;
    private const int ClassFloat64 = 0xA8;
    private const int Int16Start = 0x89;
    private const int Uint16Start = 0xA9;
    private const int Float16LowStart = 0xC8;
    private const int Float16HighStart = 0xE8;

    private static readonly string[] FloatConditions =
        ["f", "lt", "eq", "le", "gt", "lg", "ge", "o", "u", "nge", "nlg", "ngt", "nle", "neq", "nlt", "tru"];

    private static readonly string[] IntegerConditions = ["f", "lt", "eq", "le", "gt", "ne", "ge", "t"];

    public static readonly FrozenDictionary<int, OpcodeInfo> Table = OpcodeTable.Parse(Entries());

    private static List<string> Entries()
    {
        var entries = new List<string>();

        foreach (var exec in new[] { false, true })
        {
            var offset = 0;
            var prefix = "v_cmp_";

            if (exec)
            {
                offset = CompareX;
                prefix = "v_cmpx_";
            }

            Row(entries, prefix, FloatConditions, 0, FloatRow, Float32Row + offset, "f32", 'f');
            Row(entries, prefix, FloatConditions, 0, FloatRow, Float64Row + offset, "f64", 'F');
            Row(entries, prefix, IntegerConditions, 0, IntegerRow, Int32Row + offset, "i32", 'u');
            Row(entries, prefix, IntegerConditions, 0, IntegerRow, Int64Row + offset, "i64", 'U');
            Row(entries, prefix, IntegerConditions, 0, IntegerRow, Uint32Row + offset, "u32", 'u');
            Row(entries, prefix, IntegerConditions, 0, IntegerRow, Uint64Row + offset, "u64", 'U');

            Row(entries, prefix, IntegerConditions, 1, IntegerRow - 1, Int16Start + offset, "i16", 's');
            Row(entries, prefix, IntegerConditions, 1, IntegerRow - 1, Uint16Start + offset, "u16", 's');
            Row(entries, prefix, FloatConditions, 0, IntegerRow, Float16LowStart + offset, "f16", 'h');
            Row(entries, prefix, FloatConditions, IntegerRow, FloatRow, Float16HighStart + offset, "f16", 'h');

            Add(entries, ClassFloat32 + offset, prefix + "class_f32", exec, "fu");
            Add(entries, ClassFloat64 + offset, prefix + "class_f64", exec, "Fu");
            Add(entries, ClassFloat16 + offset, prefix + "class_f16", exec, "hs");
        }

        return entries;
    }

    private static void Row(List<string> entries, string prefix, string[] conditions, int first, int end, int opcode, string type, char letter)
    {
        var exec = prefix == "v_cmpx_";
        var sources = new string(letter, CompareSources);

        for (var index = first; index < end; index++)
        {
            Add(entries, opcode + index - first, $"{prefix}{conditions[index]}_{type}", exec, sources);
        }
    }

    private static void Add(List<string> entries, int opcode, string name, bool exec, string sources)
    {
        var destination = "m";

        if (exec)
        {
            destination = string.Empty;
        }

        entries.Add(OpcodeTable.Entry(opcode, name, destination + ":" + sources));
    }
}
