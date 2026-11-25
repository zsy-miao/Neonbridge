// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// VOP3P: packed 16-bit math and the f16/f32 mix instructions (AMD "RDNA 2 Instruction Set Architecture", VOP3P format).
/// Fields: VDST [7:0], NEG_HI [10:8], OP_SEL [13:11], OP_SEL_HI(src2) [14], CLAMP [15], OP [22:16]; SRC0/1/2 as VOP3,
/// OP_SEL_HI(src0, src1) [60:59], NEG [63:61]. Mix instructions show NEG as <c>-x</c> and NEG_HI as <c>|x|</c>.
/// </summary>
internal static class PackedDecoder
{
    private const int Words = 2;
    private const int ByteWidth = 8;
    private const int NegHighLow = 8;
    private const int ThreeBits = 3;
    private const int OpSelLow = 11;
    private const int OpSelHigh2Bit = 14;
    private const int ClampBit = 15;
    private const int OpcodeLow = 16;
    private const int OpcodeWidth = 7;
    private const int SourceWidth = 9;
    private const int OpSelHighLow = 27;
    private const int TwoBits = 2;
    private const int NegLow = 29;
    private const int MixSources = 3;

    public static Decoded? Decode(CodeWords code, int at)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var first = code[at];
        var second = code[at + 1];

        if (!VectorOpcodes.Vop3P.TryGetValue(Bits.Field(first, OpcodeLow, OpcodeWidth), out var info))
        {
            return null;
        }

        var count = info.Sources.Count;
        var sources = Enumerable.Range(0, count).Select(index => Bits.Field(second, index * SourceWidth, SourceWidth)).ToArray();
        var words = Words;
        var negLow = Bits.Field(second, NegLow, ThreeBits);
        var negHigh = Bits.Field(first, NegHighLow, ThreeBits);

        if ((count < MixSources && Bits.Field(second, count * SourceWidth, SourceWidth) != 0)
            || (info.Sources[0] == OperandType.PackedInteger && (negLow | negHigh) != 0)
            || !Sources.Literal(code, at, sources, ref words, out var literal))
        {
            return null;
        }

        var mix = info.Name.Contains("_mix", StringComparison.Ordinal);
        var operands = new List<string> { OperandText.Vgpr(Bits.Field(first, 0, ByteWidth), 1) };

        for (var index = 0; index < count; index++)
        {
            var text = OperandText.Source(sources[index], info.Sources[index], code.Wave64, literal);

            if (text is null)
            {
                return null;
            }

            if (mix)
            {
                text = Modifiers.Apply(text, ((negLow >> index) & 1) != 0, ((negHigh >> index) & 1) != 0, Modifiers.IsConstant(sources[index]));
            }

            operands.Add(text);
        }

        var opSelHigh = Bits.Field(second, OpSelHighLow, TwoBits) | (Bits.Field(first, OpSelHigh2Bit, 1) << TwoBits);
        var tail = Tail(first, count, opSelHigh, mix, negLow, negHigh);

        return new Decoded(Sources.Join(info.Name, operands) + tail, words);
    }

    private static string Tail(uint first, int count, int opSelHigh, bool mix, int negLow, int negHigh)
    {
        var mask = (1 << count) - 1;
        var opSel = Bits.Field(first, OpSelLow, ThreeBits) & mask;
        var text = new StringBuilder();
        var defaultHigh = mask;

        if (mix)
        {
            defaultHigh = 0;
            count = MixSources;
        }

        if (opSel != 0)
        {
            text.Append(" op_sel:").Append(Modifiers.BitList(opSel, count));
        }

        if ((opSelHigh & mask) != defaultHigh)
        {
            text.Append(" op_sel_hi:").Append(Modifiers.BitList(opSelHigh, count));
        }

        if (!mix && (negLow & mask) != 0)
        {
            text.Append(" neg_lo:").Append(Modifiers.BitList(negLow, count));
        }

        if (!mix && (negHigh & mask) != 0)
        {
            text.Append(" neg_hi:").Append(Modifiers.BitList(negHigh, count));
        }

        if (Bits.Flag(first, ClampBit))
        {
            text.Append(" clamp");
        }

        return text.ToString();
    }
}
