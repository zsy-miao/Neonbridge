// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// EXP: export to a render target, position, primitive or parameter (AMD "RDNA 2 Instruction Set Architecture", EXP
/// format). Fields: EN [3:0], TARGET [9:4], COMPR [10], DONE [11], VM [12]; VSRC0-3 one byte each in the second dword.
/// Compressed exports use VSRC0 for the first two channels and VSRC1 for the last two.
/// </summary>
internal static class ExportDecoder
{
    private const int Words = 2;
    private const int Channels = 4;
    private const int ChannelsPerRegister = 2;
    private const int TargetLow = 4;
    private const int TargetWidth = 6;
    private const int ComprBit = 10;
    private const int DoneBit = 11;
    private const int VmBit = 12;
    private const int ByteWidth = 8;
    private const int LastMrt = 7;
    private const int Mrtz = 8;
    private const int NullTarget = 9;
    private const int FirstPosition = 12;
    private const int LastPosition = 16;
    private const int Primitive = 20;
    private const int FirstParameter = 32;

    public static Decoded? Decode(CodeWords code, int at)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var first = code[at];
        var second = code[at + 1];
        var target = Target(Bits.Field(first, TargetLow, TargetWidth));
        var compressed = Bits.Flag(first, ComprBit);
        var sources = new List<string>();

        for (var channel = 0; channel < Channels; channel++)
        {
            var slot = channel;

            if (compressed)
            {
                slot = channel / ChannelsPerRegister;
            }

            var source = OperandText.Vgpr(Bits.Field(second, slot * ByteWidth, ByteWidth), 1);

            if (!Bits.Flag(first, channel))
            {
                source = "off";
            }

            sources.Add(source);
        }

        var text = new StringBuilder($"exp {target} {string.Join(", ", sources)}");
        AppendFlag(text, first, DoneBit, " done");
        AppendFlag(text, first, ComprBit, " compr");
        AppendFlag(text, first, VmBit, " vm");

        return new Decoded(text.ToString(), Words);
    }

    private static string Target(int target)
    {
        if (target <= LastMrt)
        {
            return "mrt" + target.ToString(CultureInfo.InvariantCulture);
        }

        switch (target)
        {
            case Mrtz:
                return "mrtz";
            case NullTarget:
                return "null";
            case Primitive:
                return "prim";
        }

        if (target is >= FirstPosition and <= LastPosition)
        {
            return "pos" + (target - FirstPosition).ToString(CultureInfo.InvariantCulture);
        }

        if (target >= FirstParameter)
        {
            return "param" + (target - FirstParameter).ToString(CultureInfo.InvariantCulture);
        }

        return "invalid_target_" + target.ToString(CultureInfo.InvariantCulture);
    }

    private static void AppendFlag(StringBuilder text, uint word, int bit, string flag)
    {
        if (Bits.Flag(word, bit))
        {
            text.Append(flag);
        }
    }
}
