// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Numerics;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// MIMG: image access through a T# (AMD "RDNA 2 Instruction Set Architecture", MIMG format). Fields: OP[7] [0], NSA
/// [2:1], DIM [5:3], DLC [7], DMASK [11:8], UNORM [12], GLC [13], R128 [15], TFE [16], LWE [17], OP [24:18], SLC [25];
/// VADDR [39:32], VDATA [47:40], SRSRC [52:48], SSAMP [57:53], A16 [62], D16 [63]. NSA adds dwords holding one more
/// address VGPR per byte. When they hold fewer addresses than the instruction needs, llvm-objdump lists the NSA slots
/// and a single data VGPR. R128 does not change how the resource prints.
/// </summary>
internal static class ImageDecoder
{
    private const int Words = 2;
    private const int NsaLow = 1;
    private const int NsaWidth = 2;
    private const int DimLow = 3;
    private const int DimWidth = 3;
    private const int DlcBit = 7;
    private const int DmaskLow = 8;
    private const int DmaskWidth = 4;
    private const int UnormBit = 12;
    private const int GlcBit = 13;
    private const int R128Bit = 15;
    private const int TfeBit = 16;
    private const int LweBit = 17;
    private const int OpcodeLow = 18;
    private const int OpcodeWidth = 7;
    private const int SlcBit = 25;
    private const int ByteWidth = 8;
    private const int DataLow = 8;
    private const int ResourceLow = 16;
    private const int SamplerLow = 21;
    private const int RegisterGroupWidth = 5;
    private const int A16Bit = 30;
    private const int D16Bit = 31;
    private const int BytesPerWord = 4;
    private const int QuadDwords = 4;
    private const int ResourceDwords = 8;
    private const int HalvesPerDword = 2;

    private static readonly string[] Dimensions =
        ["1D", "2D", "3D", "CUBE", "1D_ARRAY", "2D_ARRAY", "2D_MSAA", "2D_MSAA_ARRAY"];

    public static Decoded? Decode(CodeWords code, int at)
    {
        var first = code[at];
        var nsa = Bits.Field(first, NsaLow, NsaWidth);
        var words = Words + nsa;

        if (!code.Has(at, words))
        {
            return null;
        }

        var opcode = Bits.Field(first, OpcodeLow, OpcodeWidth) | ((int)(first & 1) << OpcodeWidth);

        if (!ImageOpcodes.Table.TryGetValue(opcode, out var name))
        {
            return null;
        }

        var second = code[at + 1];
        var samples = name.Contains("sample", StringComparison.Ordinal) || name.Contains("gather4", StringComparison.Ordinal) || name == "image_get_lod";

        if (!samples && Bits.Field(second, SamplerLow, RegisterGroupWidth) != 0)
        {
            return null;
        }

        var dim = Bits.Field(first, DimLow, DimWidth);
        var a16 = Bits.Flag(second, A16Bit);
        var ray = name.StartsWith("image_bvh", StringComparison.Ordinal);

        var count = ImageAddress.Count(name, dim, a16);
        var dataDwords = DataDwords(name, first, second, ray);
        var slots = 1 + (nsa * BytesPerWord);
        var resourceDwords = ResourceDwords;

        if (nsa > 0 && count > slots)
        {
            count = slots;
            dataDwords = 1;
        }

        if (ray)
        {
            resourceDwords = QuadDwords;
        }

        var operands = new List<string>
        {
            OperandText.Vgpr(Bits.Field(second, DataLow, ByteWidth), dataDwords),
            Addresses(code, at, nsa, Bits.Field(second, 0, ByteWidth), count),
            OperandText.Scalar(Bits.Field(second, ResourceLow, RegisterGroupWidth) * QuadDwords, resourceDwords) ?? OperandText.Undecodable,
        };

        if (samples)
        {
            operands.Add(OperandText.Scalar(Bits.Field(second, SamplerLow, RegisterGroupWidth) * QuadDwords, QuadDwords) ?? OperandText.Undecodable);
        }

        return new Decoded(Sources.Join(name, operands) + Tail(first, second, dim, ray), words);
    }

    private static int DataDwords(string name, uint first, uint second, bool ray)
    {
        if (ray)
        {
            return QuadDwords;
        }

        var dwords = Math.Max(BitOperations.PopCount((uint)Bits.Field(first, DmaskLow, DmaskWidth)), 1);

        if (name.Contains("gather4", StringComparison.Ordinal))
        {
            dwords = QuadDwords;
        }

        if (Bits.Flag(second, D16Bit))
        {
            dwords = (dwords + 1) / HalvesPerDword;
        }

        if (Bits.Flag(first, TfeBit))
        {
            dwords++;
        }

        return dwords;
    }

    private static string Addresses(CodeWords code, int at, int nsa, int first, int count)
    {
        if (nsa == 0)
        {
            return OperandText.Vgpr(first, count);
        }

        var registers = new List<string> { OperandText.Vgpr(first, 1) };

        for (var index = 1; index < count; index++)
        {
            var word = code[at + Words + ((index - 1) / BytesPerWord)];
            registers.Add(OperandText.Vgpr(Bits.Field(word, ((index - 1) % BytesPerWord) * ByteWidth, ByteWidth), 1));
        }

        return "[" + string.Join(", ", registers) + "]";
    }

    private static string Tail(uint first, uint second, int dim, bool ray)
    {
        var text = new StringBuilder();

        if (!ray)
        {
            var dmask = Bits.Field(first, DmaskLow, DmaskWidth);

            if (dmask != 0)
            {
                text.Append(CultureInfo.InvariantCulture, $" dmask:0x{dmask:x}");
            }

            text.Append(" dim:SQ_RSRC_IMG_").Append(Dimensions[dim]);
            AppendFlag(text, first, UnormBit, " unorm");
            AppendFlag(text, first, GlcBit, " glc");
            AppendFlag(text, first, SlcBit, " slc");
            AppendFlag(text, first, DlcBit, " dlc");
            AppendFlag(text, first, R128Bit, " r128");
        }

        AppendFlag(text, second, A16Bit, " a16");

        if (!ray)
        {
            AppendFlag(text, first, TfeBit, " tfe");
            AppendFlag(text, first, LweBit, " lwe");
            AppendFlag(text, second, D16Bit, " d16");
        }

        return text.ToString();
    }

    private static void AppendFlag(StringBuilder text, uint word, int bit, string flag)
    {
        if (Bits.Flag(word, bit))
        {
            text.Append(flag);
        }
    }
}
