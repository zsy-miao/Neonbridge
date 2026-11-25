// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// SOP2, SOP1 and SOPC: scalar ALU with register or constant sources and an optional literal (AMD "RDNA 2 Instruction
/// Set Architecture", "Scalar ALU Operations" formats). Fields: SSRC0 [7:0], SSRC1 [15:8], SDST [22:16].
/// </summary>
internal static class ScalarDecoder
{
    public const uint Sop1Prefix = 0x17D;
    public const uint SopcPrefix = 0x17E;
    public const uint SoppPrefix = 0x17F;
    public const int PrefixShift = 23;
    public const uint SopkPrefix = 0xB;
    public const int SopkShift = 28;

    private const int ByteWidth = 8;
    private const int Source1Low = 8;
    private const int DestinationLow = 16;
    private const int DestinationWidth = 7;
    private const int Sop2OpcodeLow = 23;
    private const int Sop2OpcodeWidth = 7;
    private const int Sop1OpcodeLow = 8;
    private const int SopcOpcodeLow = 16;
    private const int LdsDirect = 254;

    public static Decoded? Decode(CodeWords code, int at)
    {
        var word = code[at];
        var prefix = word >> PrefixShift;

        if (prefix == SoppPrefix)
        {
            return ProgramControlDecoder.Decode(code, at);
        }

        if (prefix == Sop1Prefix)
        {
            return Format(code, at, ScalarOpcodes.Sop1.GetValueOrDefault(Bits.Field(word, Sop1OpcodeLow, ByteWidth)), [Bits.Field(word, 0, ByteWidth)]);
        }

        if (prefix == SopcPrefix)
        {
            return Format(code, at, ScalarOpcodes.Sopc.GetValueOrDefault(Bits.Field(word, SopcOpcodeLow, DestinationWidth)), Pair(word));
        }

        if (word >> SopkShift == SopkPrefix)
        {
            return ScalarImmediateDecoder.Decode(code, at);
        }

        return Format(code, at, ScalarOpcodes.Sop2.GetValueOrDefault(Bits.Field(word, Sop2OpcodeLow, Sop2OpcodeWidth)), Pair(word));
    }

    /// <summary>
    /// A scalar source. LDS_DIRECT decodes but is flagged (32-bit) or does not decode (64-bit); the instructions that
    /// read a register by index or jump to one take no constant, which llvm-objdump flags.
    /// </summary>
    private static string? Source(OpcodeInfo info, int index, int code, bool wave64, uint literal)
    {
        var type = info.Sources[index];
        var text = OperandText.Source(code, type, wave64, literal);

        if (text is null)
        {
            return null;
        }

        if (code == LdsDirect)
        {
            if (OperandText.Dwords(type, wave64) > 1)
            {
                return null;
            }

            return OperandText.Invalid(text, "SReg_32");
        }

        if (info.Name is "s_movrels_b32" or "s_movrels_b64" or "s_setpc_b64" or "s_rfe_b64" && Modifiers.IsConstant(code))
        {
            return text + "/*Invalid immediate*/";
        }

        return text;
    }

    private static int[] Pair(uint word)
    {
        return [Bits.Field(word, 0, ByteWidth), Bits.Field(word, Source1Low, ByteWidth)];
    }

    private static Decoded? Format(CodeWords code, int at, OpcodeInfo? info, int[] sources)
    {
        if (info is null)
        {
            return null;
        }

        var words = 1;
        var used = sources.Take(info.Sources.Count).ToArray();

        if (!Sources.Literal(code, at, used, ref words, out var literal))
        {
            return null;
        }

        var operands = new List<string>();

        if (info.Destination != OperandType.None)
        {
            var destination = OperandText.Scalar(Bits.Field(code[at], DestinationLow, DestinationWidth), OperandText.Dwords(info.Destination, code.Wave64));

            if (destination is null)
            {
                return null;
            }

            operands.Add(destination);
        }

        for (var index = 0; index < used.Length; index++)
        {
            var source = Source(info, index, used[index], code.Wave64, literal);

            if (source is null)
            {
                return null;
            }

            operands.Add(source);
        }

        return new Decoded(Sources.Join(info.Name, operands), words);
    }
}
