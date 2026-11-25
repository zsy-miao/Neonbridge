// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// SOPK: a scalar register and a 16-bit immediate (AMD "RDNA 2 Instruction Set Architecture", SOPK format). Fields:
/// SIMM16 [15:0], SDST [22:16], OP [27:23]. The immediate prints in hex, as a hardware register or as a branch.
/// </summary>
internal static class ScalarImmediateDecoder
{
    private const int ImmediateWidth = 16;
    private const int DestinationLow = 16;
    private const int DestinationWidth = 7;
    private const int OpcodeLow = 23;
    private const int OpcodeWidth = 5;
    private const int PairDwords = 2;

    public static Decoded? Decode(CodeWords code, int at)
    {
        var word = code[at];

        if (!ScalarOpcodes.Sopk.TryGetValue(Bits.Field(word, OpcodeLow, OpcodeWidth), out var name))
        {
            return null;
        }

        var immediate = Bits.Field(word, 0, ImmediateWidth);
        var register = Bits.Field(word, DestinationLow, DestinationWidth);

        switch (name)
        {
            case "s_version":
                return new Decoded(name + " " + OperandText.Hex((uint)immediate), 1);
            case "s_getreg_b32":
                return WithRegister(name, register, 1, HardwareRegister.Text(immediate), true);
            case "s_setreg_b32":
                return WithRegister(name, register, 1, HardwareRegister.Text(immediate), false);
            case "s_setreg_imm32_b32":
                return SetRegisterImmediate(code, at, name, immediate);
            case "s_call_b64":
                return WithRegister(name, register, PairDwords, Branch.Text(at, immediate), true);
            case "s_subvector_loop_begin":
            case "s_subvector_loop_end":
                return WithRegister(name, register, 1, Branch.Text(at, immediate), true);
            default:
                return WithRegister(name, register, 1, OperandText.Hex((uint)immediate), true);
        }
    }

    private static Decoded? WithRegister(string name, int register, int dwords, string immediate, bool registerFirst)
    {
        var text = OperandText.Scalar(register, dwords);

        if (text is null)
        {
            return null;
        }

        if (registerFirst)
        {
            return new Decoded(string.Create(CultureInfo.InvariantCulture, $"{name} {text}, {immediate}"), 1);
        }

        return new Decoded(string.Create(CultureInfo.InvariantCulture, $"{name} {immediate}, {text}"), 1);
    }

    private static Decoded? SetRegisterImmediate(CodeWords code, int at, string name, int immediate)
    {
        if (!code.Has(at + 1, 1))
        {
            return null;
        }

        var value = OperandText.Constant(code[at + 1], OperandType.Word);

        return new Decoded($"{name} {HardwareRegister.Text(immediate)}, {value}", PairDwords);
    }
}
