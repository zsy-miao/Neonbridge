// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// Builds an opcode table from compact entries <c>"&lt;hex opcode&gt; &lt;mnemonic&gt; &lt;signature&gt; [bare]"</c>. The
/// signature is <c>destination:sources</c>, one letter per operand: <c>u</c> 32-bit integer, <c>f</c> f32, <c>U</c> 64-bit
/// integer, <c>F</c> f64, <c>s</c> 16-bit integer, <c>h</c> f16, <c>S</c>/<c>H</c> packed 16-bit integer/float, <c>m</c> lane
/// mask, <c>Q</c> 128 bits; an empty side means no operand.
/// </summary>
internal static class OpcodeTable
{
    private const int OpcodeField = 0;
    private const int NameField = 1;
    private const int SignatureField = 2;
    private const int FlagField = 3;
    private const string BareFlag = "bare";

    public static FrozenDictionary<int, OpcodeInfo> Parse(IEnumerable<string> entries)
    {
        var table = new Dictionary<int, OpcodeInfo>();

        foreach (var entry in entries)
        {
            var fields = entry.Split(' ', StringSplitOptions.RemoveEmptyEntries);
            var opcode = int.Parse(fields[OpcodeField], NumberStyles.HexNumber, CultureInfo.InvariantCulture);
            var signature = fields[SignatureField].Split(':');
            var bare = fields.Length > FlagField && fields[FlagField] == BareFlag;
            table[opcode] = new OpcodeInfo(fields[NameField], Single(signature[0]), signature[1].Select(Type).ToArray(), bare);
        }

        return table.ToFrozenDictionary();
    }

    /// <summary>An entry for code that builds tables from name patterns.</summary>
    public static string Entry(int opcode, string name, string signature)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{opcode:x} {name} {signature}");
    }

    private static OperandType Single(string letters)
    {
        if (letters.Length == 0)
        {
            return OperandType.None;
        }

        return Type(letters[0]);
    }

    private static OperandType Type(char letter)
    {
        switch (letter)
        {
            case 'u':
                return OperandType.Word;
            case 'f':
                return OperandType.Real;
            case 'U':
                return OperandType.WideInteger;
            case 'F':
                return OperandType.WideReal;
            case 's':
                return OperandType.HalfInteger;
            case 'h':
                return OperandType.HalfReal;
            case 'S':
                return OperandType.PackedInteger;
            case 'H':
                return OperandType.PackedReal;
            case 'm':
                return OperandType.LaneMask;
            case 'Q':
                return OperandType.Quad;
            default:
                throw new ArgumentOutOfRangeException(nameof(letter), letter, "unknown operand letter");
        }
    }
}
