// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// DS: local data share access (AMD "RDNA 2 Instruction Set Architecture", DS format). Fields: OFFSET0 [7:0], OFFSET1
/// [15:8], GDS [17], OP [25:18]; ADDR [39:32], DATA0 [47:40], DATA1 [55:48], VDST [63:56].
/// </summary>
internal static class LdsDecoder
{
    private const int Words = 2;
    private const int ByteWidth = 8;
    private const int Offset1Low = 8;
    private const int OffsetWidth = 16;
    private const int GdsBit = 17;
    private const int OpcodeLow = 18;
    private const int Data0Low = 8;
    private const int Data1Low = 16;
    private const int DestinationLow = 24;
    private const int PairCount = 2;

    /// <summary>Second-dword fields each shape leaves unused; they must be 0 for the dword to decode.</summary>
    private static readonly Dictionary<char, uint> UnusedFields = new()
    {
        ['w'] = 0xFFFF0000, ['a'] = 0xFFFF0000, ['W'] = 0xFF000000, ['c'] = 0xFF000000, ['r'] = 0x00FFFF00,
        ['s'] = 0x00FFFF00, ['R'] = 0x00FFFF00, ['t'] = 0x00FF0000, ['p'] = 0x00FF0000, ['C'] = 0, ['X'] = 0,
        ['n'] = 0x00FFFFFF, ['o'] = 0xFFFFFFFF,
    };

    public static Decoded? Decode(CodeWords code, int at)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var first = code[at];
        var second = code[at + 1];

        if (!LdsOpcodes.Table.TryGetValue(Bits.Field(first, OpcodeLow, ByteWidth), out var entry) || (second & UnusedFields[entry.Shape]) != 0)
        {
            return null;
        }

        var address = OperandText.Vgpr(Bits.Field(second, 0, ByteWidth), 1);
        var data0 = OperandText.Vgpr(Bits.Field(second, Data0Low, ByteWidth), entry.Dwords);
        var data1 = OperandText.Vgpr(Bits.Field(second, Data1Low, ByteWidth), entry.Dwords);
        var destination = OperandText.Vgpr(Bits.Field(second, DestinationLow, ByteWidth), entry.Dwords);
        var pairDestination = OperandText.Vgpr(Bits.Field(second, DestinationLow, ByteWidth), entry.Dwords * PairCount);
        List<string> operands;

        switch (entry.Shape)
        {
            case 'w':
            case 'a':
                operands = [address, data0];
                break;
            case 'W':
            case 'c':
                operands = [address, data0, data1];
                break;
            case 'r':
            case 's':
                operands = [destination, address];
                break;
            case 'R':
                operands = [pairDestination, address];
                break;
            case 't':
            case 'p':
                operands = [destination, address, data0];
                break;
            case 'C':
                operands = [destination, address, data0, data1];
                break;
            case 'X':
                operands = [pairDestination, address, data0, data1];
                break;
            case 'n':
                operands = [destination];
                break;
            default:
                operands = [];
                break;
        }

        return new Decoded(Sources.Join(entry.Name, operands) + Offsets(entry.Shape, first) + Gds(first), Words);
    }

    private static string Offsets(char shape, uint first)
    {
        var offset0 = Bits.Field(first, 0, ByteWidth);
        var offset1 = Bits.Field(first, Offset1Low, ByteWidth);

        if (shape is 'W' or 'R' or 'X')
        {
            var text = string.Empty;

            if (offset0 != 0)
            {
                text += string.Create(CultureInfo.InvariantCulture, $" offset0:{offset0}");
            }

            if (offset1 != 0)
            {
                text += string.Create(CultureInfo.InvariantCulture, $" offset1:{offset1}");
            }

            return text;
        }

        var offset = Bits.Field(first, 0, OffsetWidth);

        if (offset == 0)
        {
            return string.Empty;
        }

        if (shape == 's')
        {
            return " offset:" + Swizzle.Text(offset);
        }

        return string.Create(CultureInfo.InvariantCulture, $" offset:{offset}");
    }

    private static string Gds(uint first)
    {
        if (Bits.Flag(first, GdsBit))
        {
            return " gds";
        }

        return string.Empty;
    }
}
