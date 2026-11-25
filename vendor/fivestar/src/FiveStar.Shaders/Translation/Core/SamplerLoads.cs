// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Disassembly;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// The s_load dwords that load a sampler's S# dword 0: for each image instruction that samples with S# registers, the
/// nearest instruction before it that writes the S#'s first register, when that is an s_load. A 32-bit pointer is
/// relative to its heap on the console, and the view and the sampler heap each start at 0: Scaleform's FTexTG programs
/// read their S# at a root constant (the sampler heap) + index * 16, MS_VS_Transform_PositionOnly its V# at a root
/// constant (the view heap) + index * 32. Such a load reads the window's sampler records (prelude SamplerPtrLoad).
/// </summary>
internal static class SamplerLoads
{
    private const int SamplerOperand = 3;

    /// <summary>
    /// Scalar instructions whose first operand is read, not written: s_setreg's is a <c>hwreg(...)</c> operand, which
    /// no register parse takes (the round-mode write before a sample in 176 deferred-pass programs).
    /// </summary>
    private static readonly string[] Readers = ["s_cmp", "s_bitcmp", "s_cbranch", "s_branch", "s_setpc", "s_store", "s_buffer_store", "s_atomic", "s_sendmsg", "s_waitcnt", "s_setreg"];

    /// <summary>(instruction address, dword of its destination) of each load of an S#'s dword 0.</summary>
    public static HashSet<(int Address, int Dword)> Find(IReadOnlyList<Instruction> instructions)
    {
        var loads = new HashSet<(int Address, int Dword)>();

        for (int index = 0; index < instructions.Count; index++)
        {
            var (mnemonic, arguments) = Split(instructions[index]);

            if (!Samples(mnemonic) || arguments.Count <= SamplerOperand)
            {
                continue;
            }

            var sampler = OperandParser.Parse(arguments[SamplerOperand]);

            if (sampler.Kind == OperandKind.Scalar)
            {
                FindLoad(instructions, index, sampler.Low, loads);
            }
        }

        return loads;
    }

    /// <summary>The nearest write of SGPR <paramref name="register"/> before <paramref name="use"/>, when an s_load.</summary>
    private static void FindLoad(IReadOnlyList<Instruction> instructions, int use, int register, HashSet<(int Address, int Dword)> loads)
    {
        for (int index = use - 1; index >= 0; index--)
        {
            var (mnemonic, arguments) = Split(instructions[index]);

            if (arguments.Count == 0 || !Writes(mnemonic))
            {
                continue;
            }

            if (Readers.Any(prefix => mnemonic.StartsWith(prefix, StringComparison.Ordinal)))
            {
                continue;
            }

            var destination = OperandParser.Parse(arguments[0]);

            if (destination.Kind != OperandKind.Scalar || register < destination.Low || register > destination.High)
            {
                continue;
            }

            if (mnemonic.StartsWith("s_load_", StringComparison.Ordinal))
            {
                loads.Add((instructions[index].Address, register - destination.Low));
            }

            return;
        }
    }

    /// <summary>Whether the instruction may write SGPRs through its first operand.</summary>
    private static bool Writes(string mnemonic)
    {
        return mnemonic.StartsWith("s_", StringComparison.Ordinal) || mnemonic.StartsWith("v_read", StringComparison.Ordinal)
            || mnemonic.StartsWith("v_cmp", StringComparison.Ordinal);
    }

    private static bool Samples(string mnemonic)
    {
        return mnemonic.StartsWith("image_sample", StringComparison.Ordinal) || mnemonic.StartsWith("image_gather", StringComparison.Ordinal)
            || mnemonic.StartsWith("image_get_lod", StringComparison.Ordinal);
    }

    private static (string Mnemonic, List<string> Arguments) Split(Instruction instruction)
    {
        var line = instruction.Text.Split(';')[0].Split("//")[0].Trim();
        var space = line.IndexOfAny([' ', '\t']);

        if (space < 0)
        {
            return (line, []);
        }

        return (line[..space], ArgumentList.Split(InstructionModifiers.Strip(line[space..].TrimStart())));
    }
}
