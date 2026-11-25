// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// FLAT, SCRATCH and GLOBAL memory access (AMD "RDNA 2 Instruction Set Architecture", FLAT format). Fields: OFFSET
/// [11:0], DLC [12], LDS [13], SEG [15:14], GLC [16], SLC [17], OP [24:18]; ADDR [39:32], DATA [47:40], SADDR [54:48],
/// VDST [63:56]. A GLOBAL or SCRATCH SADDR of null means "off"; a load with LDS writes LDS and names no VGPR.
/// </summary>
internal static class FlatDecoder
{
    private const int Words = 2;
    private const int OffsetWidth = 12;
    private const int DlcBit = 12;
    private const int LdsBit = 13;
    private const int SegmentLow = 14;
    private const int SegmentWidth = 2;
    private const int GlcBit = 16;
    private const int SlcBit = 17;
    private const int OpcodeLow = 18;
    private const int OpcodeWidth = 7;
    private const int ByteWidth = 8;
    private const int DataLow = 8;
    private const int ScalarLow = 16;
    private const int ScalarWidth = 7;
    private const int DestinationLow = 24;
    private const int PairDwords = 2;
    private const int Scratch = 1;
    private const int Global = 2;

    private static readonly string[] Segments = ["flat_", "scratch_", "global_"];

    /// <summary>The loads that can write LDS directly (LDS [13]); they name no destination VGPR.</summary>
    private static readonly HashSet<string> LdsLoads = ["load_ubyte", "load_sbyte", "load_ushort", "load_sshort", "load_dword"];

    public static Decoded? Decode(CodeWords code, int at)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var first = code[at];
        var second = code[at + 1];
        var segment = Bits.Field(first, SegmentLow, SegmentWidth);

        if (segment >= Segments.Length || !MemoryOpcodes.Flat.TryGetValue(Bits.Field(first, OpcodeLow, OpcodeWidth), out var entry))
        {
            return null;
        }

        var atomic = entry.Name.StartsWith("atomic", StringComparison.Ordinal);
        var scalarField = Bits.Field(second, ScalarLow, ScalarWidth);

        var lds = Bits.Flag(first, LdsBit);

        if ((segment == 0 && scalarField != OperandText.Null) || (segment == Scratch && atomic) || (lds && (segment == 0 || !LdsLoads.Contains(entry.Name))))
        {
            return null;
        }

        var address = Address(segment, Bits.Field(second, 0, ByteWidth), scalarField);
        var operands = Operands(entry, address, second, lds, Bits.Flag(first, GlcBit));
        var scalar = ScalarAddress(segment, scalarField);

        if (scalar is not null)
        {
            operands.Add(scalar);
        }

        return new Decoded(Sources.Join(Segments[segment] + entry.Name, operands) + Tail(first, segment), Words);
    }

    /// <summary>The VGPR operands: the destination of a load or returning atomic, the address and the data.</summary>
    private static List<string> Operands((string Name, int Dwords) entry, string address, uint second, bool lds, bool glc)
    {
        var operands = new List<string>();
        var data = OperandText.Vgpr(Bits.Field(second, DataLow, ByteWidth), entry.Dwords);

        if (lds)
        {
            operands.Add(address);
        }
        else if (entry.Name.StartsWith("load", StringComparison.Ordinal))
        {
            operands.Add(OperandText.Vgpr(Bits.Field(second, DestinationLow, ByteWidth), entry.Dwords));
            operands.Add(address);
        }
        else if (entry.Name.StartsWith("store", StringComparison.Ordinal))
        {
            operands.Add(address);
            operands.Add(data);
        }
        else
        {
            if (glc)
            {
                operands.Add(OperandText.Vgpr(Bits.Field(second, DestinationLow, ByteWidth), ReturnDwords(entry.Name, entry.Dwords)));
            }

            operands.Add(address);
            operands.Add(data);
        }

        return operands;
    }

    private static int ReturnDwords(string name, int dwords)
    {
        if (name.Contains("cmpswap", StringComparison.Ordinal))
        {
            return dwords / PairDwords;
        }

        return dwords;
    }

    private static string Address(int segment, int register, int scalar)
    {
        if (segment == Scratch)
        {
            if (scalar != OperandText.Null)
            {
                return "off";
            }

            return OperandText.Vgpr(register, 1);
        }

        if (segment == Global && scalar != OperandText.Null)
        {
            return OperandText.Vgpr(register, 1);
        }

        return OperandText.Vgpr(register, PairDwords);
    }

    private static string? ScalarAddress(int segment, int scalar)
    {
        if (segment == 0)
        {
            return null;
        }

        if (scalar == OperandText.Null)
        {
            return "off";
        }

        var dwords = PairDwords;

        if (segment == Scratch)
        {
            dwords = 1;
        }

        return OperandText.Scalar(scalar, dwords) ?? OperandText.Undecodable;
    }

    private static string Tail(uint first, int segment)
    {
        var text = new StringBuilder();
        var offset = Bits.Field(first, 0, OffsetWidth);

        if (segment != 0)
        {
            offset = Bits.Signed(first, 0, OffsetWidth);
        }

        if (offset != 0)
        {
            text.Append(CultureInfo.InvariantCulture, $" offset:{offset}");
        }

        if (Bits.Flag(first, GlcBit))
        {
            text.Append(" glc");
        }

        if (Bits.Flag(first, SlcBit))
        {
            text.Append(" slc");
        }

        if (Bits.Flag(first, DlcBit))
        {
            text.Append(" dlc");
        }

        if (Bits.Flag(first, LdsBit))
        {
            text.Append(" lds");
        }

        return text.ToString();
    }
}
