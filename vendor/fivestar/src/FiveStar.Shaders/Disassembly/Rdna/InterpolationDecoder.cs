// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// Attribute interpolation (AMD "RDNA 2 Instruction Set Architecture", VINTRP format and the VOP3 interpolation
/// opcodes). VINTRP: VSRC [7:0], ATTRCHAN [9:8], ATTR [15:10], OP [17:16], VDST [25:18]. In VOP3 the attribute sits in
/// SRC0: ATTR [5:0], ATTRCHAN [7:6], HIGH [8].
/// </summary>
internal static class InterpolationDecoder
{
    private const int ByteWidth = 8;
    private const int ChannelLow = 8;
    private const int ChannelWidth = 2;
    private const int AttributeLow = 10;
    private const int AttributeWidth = 6;
    private const int OpcodeLow = 16;
    private const int OpcodeWidth = 2;
    private const int DestinationLow = 18;
    private const int Vop3ChannelLow = 6;
    private const int HighBit = 8;
    private const int Move = 2;
    private const int Source2 = 2;
    private const int Vop3Words = 2;
    private const int P1F32 = 0x200;
    private const int MoveF32 = 0x202;
    private const int P1LlF16 = 0x342;
    private const int P1LvF16 = 0x343;
    private const int P2F16 = 0x35A;

    private static readonly string[] Names = ["v_interp_p1_f32", "v_interp_p2_f32", "v_interp_mov_f32"];

    private static readonly string[] Parameters = ["p10", "p20", "p0"];

    private static readonly string[] Channels = ["x", "y", "z", "w"];

    public static bool IsVop3(int opcode)
    {
        return opcode is (>= P1F32 and <= MoveF32) or P1LlF16 or P1LvF16 or P2F16;
    }

    public static Decoded? Decode(CodeWords code, int at)
    {
        var word = code[at];
        var opcode = Bits.Field(word, OpcodeLow, OpcodeWidth);

        if (opcode >= Names.Length)
        {
            return null;
        }

        var source = Bits.Field(word, 0, ByteWidth);
        var sourceText = OperandText.Vgpr(source, 1);

        if (opcode == Move)
        {
            sourceText = "invalid_param_" + source.ToString(CultureInfo.InvariantCulture);

            if (source < Parameters.Length)
            {
                sourceText = Parameters[source];
            }
        }

        var destination = OperandText.Vgpr(Bits.Field(word, DestinationLow, ByteWidth), 1);
        var attribute = Attribute(Bits.Field(word, AttributeLow, AttributeWidth), Bits.Field(word, ChannelLow, ChannelWidth));

        return new Decoded($"{Names[opcode]}_e32 {destination}, {sourceText}, {attribute}", 1);
    }

    public static Decoded? DecodeVop3(Vop3Fields fields)
    {
        if ((fields.Opcode <= MoveF32 || fields.Opcode == P1LlF16) && fields.SourceCodes[Source2] != 0)
        {
            return null;
        }

        var attributeCode = fields.SourceCodes[0];
        var attribute = Attribute(Bits.Field((uint)attributeCode, 0, AttributeWidth), Bits.Field((uint)attributeCode, Vop3ChannelLow, ChannelWidth));
        var operands = new List<string> { OperandText.Vgpr(fields.Destination, 1) };
        string name;

        if (fields.Opcode <= MoveF32)
        {
            name = Names[fields.Opcode - P1F32] + "_e64";
            operands.Add(Interpolant(fields, 1));
            operands.Add(attribute);
        }
        else
        {
            name = Vop3Name(fields.Opcode);
            operands.Add(Interpolant(fields, 1));
            operands.Add(attribute);

            if (fields.Opcode != P1LlF16)
            {
                operands.Add(Interpolant(fields, Source2));
            }
        }

        var text = new StringBuilder(Sources.Join(name, operands));

        if (Bits.Flag((uint)attributeCode, HighBit))
        {
            text.Append(" high");
        }

        if (fields.Clamp)
        {
            text.Append(" clamp");
        }

        text.Append(Modifiers.Output(fields.Omod));

        return new Decoded(text.ToString(), Vop3Words);
    }

    private static string Vop3Name(int opcode)
    {
        switch (opcode)
        {
            case P1LlF16:
                return "v_interp_p1ll_f16";
            case P1LvF16:
                return "v_interp_p1lv_f16";
            default:
                return "v_interp_p2_f16";
        }
    }

    private static string Interpolant(Vop3Fields fields, int index)
    {
        var code = fields.SourceCodes[index];
        var text = OperandText.Source(code, OperandType.Real, false, 0) ?? string.Empty;

        if (fields.Opcode == MoveF32 && index == 1)
        {
            text = Parameters.ElementAtOrDefault(code) ?? text;
        }

        return Modifiers.Apply(text, ((fields.Neg >> index) & 1) != 0, ((fields.Abs >> index) & 1) != 0, false);
    }

    private static string Attribute(int attribute, int channel)
    {
        return string.Create(CultureInfo.InvariantCulture, $"attr{attribute}.{Channels[channel]}");
    }
}
