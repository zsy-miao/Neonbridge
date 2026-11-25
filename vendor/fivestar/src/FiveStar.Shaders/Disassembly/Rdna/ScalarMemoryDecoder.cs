// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// SMEM: scalar memory loads (AMD "RDNA 2 Instruction Set Architecture", SMEM format). Fields: SBASE [5:0] (register
/// pair index), SDATA [12:6], DLC [14], GLC [16], OP [25:18]; second dword OFFSET [20:0] (signed), SOFFSET [31:25].
/// </summary>
internal static class ScalarMemoryDecoder
{
    private const int Words = 2;
    private const int BaseWidth = 6;
    private const int DataLow = 6;
    private const int DataWidth = 7;
    private const int DlcBit = 14;
    private const int GlcBit = 16;
    private const int OpcodeLow = 18;
    private const int OpcodeWidth = 8;
    private const int OffsetWidth = 21;
    private const int SoffsetLow = 25;
    private const int SoffsetWidth = 7;
    private const int PairDwords = 2;
    private const int BufferDwords = 4;
    private const int M0 = 124;
    private const int ExecHi = 127;
    private const int BitsPerDword = 32;

    public static Decoded? Decode(CodeWords code, int at)
    {
        var word = code[at];

        if (!MemoryOpcodes.ScalarMemory.TryGetValue(Bits.Field(word, OpcodeLow, OpcodeWidth), out var entry) || !code.Has(at, Words))
        {
            return null;
        }

        var dataField = Bits.Field(word, DataLow, DataWidth);
        var baseText = OperandText.Scalar(Bits.Field(word, 0, BaseWidth) * PairDwords, PairDwords) ?? OperandText.Undecodable;

        if (entry.Name.Contains("_buffer", StringComparison.Ordinal))
        {
            baseText = Data(Bits.Field(word, 0, BaseWidth) * PairDwords, BufferDwords) ?? OperandText.Undecodable;
        }

        var offset = Offset(code[at + 1]);
        var address = $"{baseText}, {offset}{Flags(word)}";

        switch (entry.Kind)
        {
            case 'n':
                return new Decoded(entry.Name, Words);
            case 'p':
                return new Decoded($"{entry.Name} {OperandText.Constant((uint)dataField, OperandType.Word)}, {baseText}, {offset}", Words);
            case 'd':
                return new Decoded($"{entry.Name} {baseText}, {offset}", Words);
        }

        var data = Data(dataField, entry.Dwords);

        if (data is null)
        {
            return null;
        }

        if (entry.Kind == 't')
        {
            return new Decoded($"{entry.Name} {data}", Words);
        }

        return new Decoded($"{entry.Name} {data}, {address}", Words);
    }

    /// <summary>Data or base registers; m0 and EXEC are outside the class a load may write, and VCC or EXEC as 128 bits or more, which print flagged.</summary>
    private static string? Data(int code, int dwords)
    {
        var text = OperandText.Scalar(code, dwords);

        if (dwords == 1 && code is M0 or OperandText.ExecLo or ExecHi)
        {
            return OperandText.Invalid(text!, "SReg_32_XM0_XEXEC");
        }

        if (dwords == PairDwords && code == OperandText.ExecLo)
        {
            return OperandText.Invalid(text!, "SReg_64_XEXEC");
        }

        if (dwords > PairDwords && code is OperandText.VccLo or OperandText.ExecLo)
        {
            var name = OperandText.Scalar(code, PairDwords)!;

            return OperandText.Invalid(name, string.Create(CultureInfo.InvariantCulture, $"SReg_{dwords * BitsPerDword}"));
        }

        return text;
    }

    private static string Offset(uint word)
    {
        var offset = Bits.Signed(word, 0, OffsetWidth);
        var soffset = Bits.Field(word, SoffsetLow, SoffsetWidth);

        if (soffset == OperandText.Null)
        {
            if (offset == 0)
            {
                return "null";
            }

            return OperandText.SignedHex(offset);
        }

        var register = OperandText.Scalar(soffset, 1);

        if (offset == 0)
        {
            return register!;
        }

        return $"{register} offset:{OperandText.SignedHex(offset)}";
    }

    private static string Flags(uint word)
    {
        var flags = string.Empty;

        if (Bits.Flag(word, GlcBit))
        {
            flags += " glc";
        }

        if (Bits.Flag(word, DlcBit))
        {
            flags += " dlc";
        }

        return flags;
    }
}
