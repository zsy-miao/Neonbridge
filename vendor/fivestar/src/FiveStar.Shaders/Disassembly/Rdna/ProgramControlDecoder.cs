// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// SOPP: program control with a 16-bit immediate (AMD "RDNA 2 Instruction Set Architecture", SOPP format). Fields:
/// SIMM16 [15:0], OP [22:16]. Each opcode prints its immediate its own way: nothing, decimal, hex, a branch or a
/// symbolic counter or message.
/// </summary>
internal static class ProgramControlDecoder
{
    private const int ImmediateWidth = 16;
    private const int OpcodeLow = 16;
    private const int OpcodeWidth = 7;

    /// <summary>Opcodes without an operand; any immediate other than 0 does not decode.</summary>
    private static readonly FrozenSet<string> Plain = new[]
    {
        "s_wakeup", "s_barrier", "s_icache_inv", "s_ttracedata", "s_code_end", "s_endpgm_saved",
        "s_endpgm_ordered_ps_done",
    }.ToFrozenSet(StringComparer.Ordinal);

    private static readonly FrozenSet<string> Hexadecimal = new[]
    {
        "s_inst_prefetch", "s_clause", "s_round_mode", "s_ttracedata_imm",
    }.ToFrozenSet(StringComparer.Ordinal);

    public static Decoded? Decode(CodeWords code, int at)
    {
        var word = code[at];

        if (!ScalarOpcodes.Sopp.TryGetValue(Bits.Field(word, OpcodeLow, OpcodeWidth), out var name))
        {
            return null;
        }

        var immediate = Bits.Field(word, 0, ImmediateWidth);
        var operand = Operand(name, at, immediate);

        if (Plain.Contains(name))
        {
            if (immediate != 0)
            {
                return null;
            }

            operand = string.Empty;
        }

        if (operand.Length == 0)
        {
            return new Decoded(name, 1);
        }

        return new Decoded(name + " " + operand, 1);
    }

    private static string Operand(string name, int at, int immediate)
    {
        if (name.StartsWith("s_cbranch_", StringComparison.Ordinal) || name == "s_branch")
        {
            return Branch.Text(at, immediate);
        }

        if (Hexadecimal.Contains(name))
        {
            return OperandText.Hex((uint)immediate);
        }

        switch (name)
        {
            case "s_endpgm":
                return EndOperand(immediate);
            case "s_waitcnt":
                return WaitCounts.Text(immediate);
            case "s_waitcnt_depctr":
                return WaitCounts.DependencyText(immediate);
            case "s_sendmsg":
            case "s_sendmsghalt":
                return Message.Text(immediate);
            default:
                return OperandText.Constant((uint)immediate, OperandType.Word);
        }
    }

    /// <summary>s_endpgm prints its immediate in decimal, and nothing for 0.</summary>
    private static string EndOperand(int immediate)
    {
        if (immediate == 0)
        {
            return string.Empty;
        }

        return immediate.ToString(CultureInfo.InvariantCulture);
    }
}
