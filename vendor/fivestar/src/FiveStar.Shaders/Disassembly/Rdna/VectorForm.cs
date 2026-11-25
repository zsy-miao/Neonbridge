// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The operand layout of one VOP1, VOP2 or VOPC instruction, shared by its plain, DPP and SDWA forms: the destination,
/// the implicit VCC of carries, v_cndmask and compares, and the constant of v_madmk-style instructions.
/// </summary>
internal sealed class VectorForm
{
    private const int Source0Width = 9;
    private const int Source1Low = 9;
    private const int RegisterWidth = 8;
    private const int DestinationLow = 17;
    private const int ThirdSource = 2;

    private readonly uint _word;
    private readonly bool _wave64;

    public VectorForm(VectorKind kind, OpcodeInfo info, uint word, bool wave64)
    {
        Kind = kind;
        Info = info;
        _word = word;
        _wave64 = wave64;
    }

    public VectorKind Kind { get; }

    public OpcodeInfo Info { get; }

    public int Source0 => Bits.Field(_word, 0, Source0Width);

    public int Source1 => Bits.Field(_word, Source1Low, RegisterWidth);

    public int Destination => Bits.Field(_word, DestinationLow, RegisterWidth);

    /// <summary>Whether the instruction has a VGPR second source in VSRC1.</summary>
    public bool HasSource1 => Kind != VectorKind.Vop1 && Info.Sources.Count > 1;

    /// <summary>v_madmk/v_madak-style: a mandatory literal constant.</summary>
    public bool HasConstant => Info.Name.EndsWith("mk_f32", StringComparison.Ordinal) || Info.Name.EndsWith("ak_f32", StringComparison.Ordinal)
        || Info.Name.EndsWith("mk_f16", StringComparison.Ordinal) || Info.Name.EndsWith("ak_f16", StringComparison.Ordinal);

    /// <summary>Whether a DPP form exists: not for compares, single-form or 64-bit instructions.</summary>
    public bool HasDpp => Kind != VectorKind.Vopc && HasExtendedForms && Info.Name != "v_mac_legacy_f32";

    /// <summary>Whether an SDWA form exists: not for single-form or 64-bit instructions, nor the v_mac family.</summary>
    public bool HasSdwa => HasExtendedForms && !(Info.Sources.Count > ThirdSource && Info.Sources[ThirdSource] != OperandType.LaneMask);

    /// <summary>A compare that writes only EXEC and names no destination.</summary>
    public bool WritesExecOnly => Kind == VectorKind.Vopc && Info.Destination == OperandType.None;

    public string Vcc
    {
        get
        {
            var name = "vcc_lo";

            if (_wave64)
            {
                name = "vcc";
            }

            return name;
        }
    }

    private bool HasExtendedForms => !Info.Bare && !IsWide(Info.Destination) && !Info.Sources.Any(IsWide);

    public string Mnemonic(string suffix)
    {
        if (Info.Bare)
        {
            return Info.Name;
        }

        return Info.Name + suffix;
    }

    /// <summary>The operand list around the formatted sources; <paramref name="constant"/> is the madmk-style literal.</summary>
    public List<string> Operands(string source0, string? source1, string? constant)
    {
        var operands = new List<string>();

        if (Kind == VectorKind.Vopc)
        {
            if (!WritesExecOnly)
            {
                operands.Add(Vcc);
            }

            operands.Add(source0);
            operands.Add(source1!);

            return operands;
        }

        if (Info.Destination == OperandType.None)
        {
            return operands;
        }

        operands.Add(DestinationText());

        if (Info.Name.Contains("_co_ci_", StringComparison.Ordinal))
        {
            operands.Add(Vcc);
        }

        operands.Add(source0);
        AddRest(operands, source1, constant);

        return operands;
    }

    private void AddRest(List<string> operands, string? source1, string? constant)
    {
        if (Info.Name.Contains("mk_f", StringComparison.Ordinal))
        {
            operands.Add(constant!);
        }

        if (source1 is not null)
        {
            operands.Add(source1);
        }

        if (Info.Name.Contains("ak_f", StringComparison.Ordinal))
        {
            operands.Add(constant!);
        }

        if (Info.Sources.Count > ThirdSource && Info.Sources[ThirdSource] == OperandType.LaneMask)
        {
            operands.Add(Vcc);
        }
    }

    private static bool IsWide(OperandType type)
    {
        return type is OperandType.WideInteger or OperandType.WideReal;
    }

    private string DestinationText()
    {
        if (Info.Name == "v_readfirstlane_b32")
        {
            return OperandText.Scalar(Destination, 1) ?? OperandText.Source(Destination, OperandType.Word, _wave64, 0) ?? string.Empty;
        }

        return OperandText.Vgpr(Destination, OperandText.Dwords(Info.Destination, _wave64));
    }
}
