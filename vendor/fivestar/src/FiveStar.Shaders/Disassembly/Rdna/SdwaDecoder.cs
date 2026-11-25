// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The SDWA form of VOP1/VOP2/VOPC (AMD "RDNA 2 Instruction Set Architecture", SDWA and SDWAB formats). Dword: SRC0
/// [7:0], DST_SEL [10:8], DST_UNUSED [12:11], CLAMP [13], OMOD [15:14] (VOPC: SDST [14:8], SD [15]), SRC0_SEL [18:16],
/// SRC0_SEXT/NEG/ABS [19]/[20]/[21], S0 [23], SRC1_SEL [26:24], SRC1_SEXT/NEG/ABS [27]/[28]/[29], S1 [31].
/// </summary>
internal static class SdwaDecoder
{
    private const int Words = 2;
    private const int ByteWidth = 8;
    private const int SelectWidth = 3;
    private const int DestinationSelectLow = 8;
    private const int UnusedLow = 11;
    private const int UnusedWidth = 2;
    private const int ClampBit = 13;
    private const int OmodLow = 14;
    private const int OmodWidth = 2;
    private const int CompareDestinationLow = 8;
    private const int CompareDestinationWidth = 7;
    private const int CompareDestinationBit = 15;
    private const int Source0Low = 16;
    private const int Source1Low = 24;
    private const int SextOffset = 3;
    private const int NegateOffset = 4;
    private const int AbsoluteOffset = 5;
    private const int ScalarOffset = 7;

    private static readonly string[] Selects = ["BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD"];

    private static readonly string[] Unused = ["UNUSED_PAD", "UNUSED_SEXT", "UNUSED_PRESERVE"];

    public static Decoded? Decode(CodeWords code, int at, VectorForm form)
    {
        if (!code.Has(at, Words))
        {
            return null;
        }

        var sdwa = code[at + 1];
        var source0 = Source(form, 0, Bits.Field(sdwa, 0, ByteWidth), sdwa, Source0Low);
        string? source1 = null;

        if (form.HasSource1)
        {
            source1 = Source(form, 1, form.Source1, sdwa, Source1Low);

            if (source1 is null)
            {
                return null;
            }
        }

        if (source0 is null)
        {
            return null;
        }

        if (form.Kind == VectorKind.Vopc)
        {
            return Compare(form, sdwa, source0, source1!, code.Wave64);
        }

        var tail = Tail(form, sdwa);

        if (tail is null)
        {
            return null;
        }

        return new Decoded(Sources.Join(form.Mnemonic("_sdwa"), form.Operands(source0, source1, null)) + tail, Words);
    }

    private static Decoded? Compare(VectorForm form, uint sdwa, string source0, string source1, bool wave64)
    {
        var selects = Select(sdwa, Source0Low, "src0_sel") + Select(sdwa, Source1Low, "src1_sel");
        var operands = form.Operands(source0, source1, null);

        if (Bits.Flag(sdwa, CompareDestinationBit))
        {
            if (form.WritesExecOnly)
            {
                return null;
            }

            var mask = OperandText.Scalar(Bits.Field(sdwa, CompareDestinationLow, CompareDestinationWidth), OperandText.Dwords(OperandType.LaneMask, wave64));

            if (mask is null)
            {
                return null;
            }

            operands[0] = mask;
        }

        if (selects.Contains('<', StringComparison.Ordinal))
        {
            return null;
        }

        return new Decoded(Sources.Join(form.Mnemonic("_sdwa"), operands) + selects, Words);
    }

    private static string? Tail(VectorForm form, uint sdwa)
    {
        var text = new StringBuilder();

        if (Bits.Flag(sdwa, ClampBit))
        {
            text.Append(" clamp");
        }

        text.Append(Modifiers.Output(Bits.Field(sdwa, OmodLow, OmodWidth)));
        text.Append(Select(sdwa, DestinationSelectLow, "dst_sel"));

        var unused = Bits.Field(sdwa, UnusedLow, UnusedWidth);

        if (unused >= Unused.Length)
        {
            return null;
        }

        text.Append(" dst_unused:").Append(Unused[unused]);
        text.Append(Select(sdwa, Source0Low, "src0_sel"));

        if (form.HasSource1)
        {
            text.Append(Select(sdwa, Source1Low, "src1_sel"));
        }

        var result = text.ToString();

        if (result.Contains('<', StringComparison.Ordinal))
        {
            return null;
        }

        return result;
    }

    /// <summary>" name:SEL", or a marker the caller rejects when the select value has no name.</summary>
    private static string Select(uint sdwa, int low, string name)
    {
        var value = Bits.Field(sdwa, low, SelectWidth);

        if (value >= Selects.Length)
        {
            return " <invalid>";
        }

        return " " + name + ":" + Selects[value];
    }

    /// <summary>A source: a VGPR, or with the S bit an SGPR or constant; float types take neg/abs, integers sext.</summary>
    private static string? Source(VectorForm form, int index, int register, uint sdwa, int selectLow)
    {
        var type = form.Info.Sources[index];
        var code = register + OperandText.FirstVgpr;

        if (Bits.Flag(sdwa, selectLow + ScalarOffset))
        {
            code = register;
        }

        if (code == OperandText.Literal)
        {
            return null;
        }

        var text = OperandText.Source(code, type, false, 0);

        if (text is null)
        {
            return null;
        }

        var negate = Bits.Flag(sdwa, selectLow + NegateOffset);
        var absolute = Bits.Flag(sdwa, selectLow + AbsoluteOffset);

        if (Modifiers.IsFloat(type))
        {
            return Modifiers.Apply(text, negate, absolute, Modifiers.IsConstant(code));
        }

        if (Bits.Flag(sdwa, selectLow + SextOffset))
        {
            text = "sext(" + text + ")";
        }

        return text;
    }
}
