// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// VOP1, VOP2 and VOPC (AMD "RDNA 2 Instruction Set Architecture", VOP1/VOP2/VOPC formats): SRC0 selects SDWA (249),
/// DPP16 (250), DPP8 (233, 234 with fetch-inactive) or a literal (255) in the next dword.
/// </summary>
internal static class VectorDecoder
{
    private const int KindShift = 25;
    private const uint Vop1Prefix = 0x3F;
    private const uint VopcPrefix = 0x3E;
    private const int Vop1OpcodeLow = 9;
    private const int VopcOpcodeLow = 17;
    private const int Vop2OpcodeLow = 25;
    private const int ByteWidth = 8;
    private const int Vop2OpcodeWidth = 6;
    private const int LdsDirect = 254;
    private const string ReadFirstLaneClass = "VRegOrLds_32";

    public static Decoded? Decode(CodeWords code, int at)
    {
        var form = Form(code[at], code.Wave64);

        if (form is null)
        {
            return null;
        }

        if (form.Info.Sources.Count == 0)
        {
            return NoOperands(form);
        }

        switch (form.Source0)
        {
            case OperandText.Sdwa when form.HasSdwa:
                return SdwaDecoder.Decode(code, at, form);
            case OperandText.Dpp16 when form.HasDpp:
                return DppDecoder.Decode16(code, at, form);
            case OperandText.Dpp8 when form.HasDpp:
            case OperandText.Dpp8FetchInactive when form.HasDpp:
                return DppDecoder.Decode8(code, at, form);
            case OperandText.Sdwa:
            case OperandText.Dpp16:
            case OperandText.Dpp8:
            case OperandText.Dpp8FetchInactive:
                return null;
            default:
                return Plain(code, at, form);
        }
    }

    private static VectorForm? Form(uint word, bool wave64)
    {
        var prefix = word >> KindShift;
        OpcodeInfo? info;
        var kind = VectorKind.Vop2;

        if (prefix == Vop1Prefix)
        {
            kind = VectorKind.Vop1;
            info = VectorOpcodes.Vop1.GetValueOrDefault(Bits.Field(word, Vop1OpcodeLow, ByteWidth));
        }
        else if (prefix == VopcPrefix)
        {
            kind = VectorKind.Vopc;
            info = CompareOpcodes.Table.GetValueOrDefault(Bits.Field(word, VopcOpcodeLow, ByteWidth));
        }
        else
        {
            info = VectorOpcodes.Vop2.GetValueOrDefault(Bits.Field(word, Vop2OpcodeLow, Vop2OpcodeWidth));
        }

        if (info is null)
        {
            return null;
        }

        return new VectorForm(kind, info, word, wave64);
    }

    /// <summary>The register class of instructions whose source must be a VGPR, or null.</summary>
    private static string? VgprSourceClass(string name)
    {
        switch (name)
        {
            case "v_readfirstlane_b32":
                return ReadFirstLaneClass;
            case "v_movrels_b32":
            case "v_movrelsd_b32":
            case "v_movrelsd_2_b32":
            case "v_swap_b32":
            case "v_swaprel_b32":
                return "VGPR_32";
            default:
                return null;
        }
    }

    /// <summary>A source that must be a VGPR (v_readfirstlane also takes LDS_DIRECT); llvm-objdump flags any other it decodes.</summary>
    private static string VgprSource(int code, string text, string registerClass)
    {
        if (code >= OperandText.FirstVgpr || (code == LdsDirect && registerClass == ReadFirstLaneClass))
        {
            return text;
        }

        if (Modifiers.IsConstant(code))
        {
            return text + "/*Invalid immediate*/";
        }

        return OperandText.Invalid(text, registerClass);
    }

    /// <summary>v_nop and friends: the source field is ignored, but a destination field other than 0 does not decode.</summary>
    private static Decoded? NoOperands(VectorForm form)
    {
        if (form.Destination != 0)
        {
            return null;
        }

        return new Decoded(form.Mnemonic("_e32"), 1);
    }

    private static Decoded? Plain(CodeWords code, int at, VectorForm form)
    {
        var words = 1;
        var source0Code = form.Source0;
        var literalUsers = new List<int> { source0Code };

        if (form.HasConstant)
        {
            literalUsers.Add(OperandText.Literal);
        }

        if (!Sources.Literal(code, at, literalUsers, ref words, out var literal))
        {
            return null;
        }

        var source0 = OperandText.Source(source0Code, form.Info.Sources[0], code.Wave64, literal);

        if (source0 is null)
        {
            return null;
        }

        var vgprClass = VgprSourceClass(form.Info.Name);

        if (vgprClass is not null)
        {
            source0 = VgprSource(source0Code, source0, vgprClass);
        }

        string? source1 = null;

        if (form.HasSource1)
        {
            source1 = OperandText.Vgpr(form.Source1, OperandText.Dwords(form.Info.Sources[1], code.Wave64));
        }

        var constant = OperandText.Hex(literal);
        var operands = form.Operands(source0, source1, constant);

        return new Decoded(Sources.Join(form.Mnemonic("_e32"), operands), words);
    }
}
