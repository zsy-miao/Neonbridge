// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// A branch whose target is out of simm16 range is compiled as
/// <c>s_getpc_b64 s[n:n+1]; s_add_u32 sn, sn, C; s_addc_u32 sn+1, sn+1, 0; s_setpc_b64 s[n:n+1]</c> (sub/subb backwards).
/// The s_setpc becomes a static <c>s_branch</c> to getpc address + 4 + C when that address is an instruction.
/// <para>
/// Some console programs hold a jump whose C is measured from the end of the s_add/s_sub instead of from the s_getpc
/// result, so it lands 8 bytes early: on the <c>s_branch</c> of a relaxed branch <c>s_cbranch_* L; s_branch FAR</c>,
/// which re-enters the code holding the jump forever. When C measured from the end of the s_add/s_sub gives exactly
/// that conditional branch's target L (the join every correct jump of this shape returns to), the target is L.
/// </para>
/// </summary>
internal static partial class LongJumpRewriter
{
    // The four-instruction sequence ends at the s_setpc; the s_getpc sits three instructions before it.
    private const int SequenceLength = 3;
    private const int AddOffset = 2;
    private const uint AddressMask = 0xffffffff;
    private const string ConditionalBranchPrefix = "s_cbranch_";
    private const string Branch = "s_branch";

    public static void Rewrite(List<Instruction> instructions)
    {
        var indexOf = new Dictionary<long, int>();

        for (var index = 0; index < instructions.Count; index++)
        {
            indexOf[instructions[index].Address] = index;
        }

        for (var index = SequenceLength; index < instructions.Count; index++)
        {
            var jump = LongJump(instructions, index);

            if (jump is null || !indexOf.ContainsKey(jump.Value.Target))
            {
                continue;
            }

            var target = Repaired(instructions, indexOf, jump.Value.Target, jump.Value.FromAddEnd);
            var setpc = instructions[index];
            instructions[index] = setpc with { Text = $"s_branch 0 {BranchTargets.Marker(target)}" };
        }
    }

    // The target from the s_getpc result (the hardware's), and the one C gives measured from the end of the s_add/s_sub.
    private static (long Target, long FromAddEnd)? LongJump(List<Instruction> instructions, int index)
    {
        var setpc = SetPcPattern().Match(instructions[index].Text);
        var getpcInstruction = instructions[index - SequenceLength];
        var getpc = GetPcPattern().Match(getpcInstruction.Text);
        var addInstruction = instructions[index - AddOffset];
        var add = AddPattern().Match(addInstruction.Text);
        var carry = CarryPattern().Match(instructions[index - 1].Text);

        if (!setpc.Success || !getpc.Success || !add.Success || !carry.Success)
        {
            return null;
        }

        var low = setpc.Groups["lo"].Value;
        var high = setpc.Groups["hi"].Value;
        var sameLow = low == getpc.Groups["lo"].Value && low == add.Groups["dst"].Value && low == add.Groups["src"].Value;
        var sameHigh = carry.Groups["dst"].Value == high && carry.Groups["src"].Value == high;
        var forward = add.Groups["op"].Value == "add";

        if (!sameLow || !sameHigh || forward != (carry.Groups["op"].Value == "addc"))
        {
            return null;
        }

        var distance = ParseImmediate(add.Groups["imm"].Value);
        var delta = forward ? distance : -distance;

        return ((getpcInstruction.Next + delta) & AddressMask, (addInstruction.Next + delta) & AddressMask);
    }

    private static long Repaired(List<Instruction> instructions, Dictionary<long, int> indexOf, long target, long fromAddEnd)
    {
        var index = indexOf[target];

        if (index == 0 || instructions[index].Mnemonic != Branch || !indexOf.ContainsKey(fromAddEnd))
        {
            return target;
        }

        var previous = instructions[index - 1];

        if (!previous.Mnemonic.StartsWith(ConditionalBranchPrefix, StringComparison.Ordinal))
        {
            return target;
        }

        if (BranchTargets.Target(previous.Text) != fromAddEnd)
        {
            return target;
        }

        return fromAddEnd;
    }

    private static long ParseImmediate(string text)
    {
        const string HexPrefix = "0x";

        if (text.StartsWith(HexPrefix, StringComparison.Ordinal))
        {
            return long.Parse(text[HexPrefix.Length..], NumberStyles.HexNumber, CultureInfo.InvariantCulture);
        }

        return long.Parse(text, CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"^s_setpc_b64 s\[(?<lo>\d+):(?<hi>\d+)\]")]
    private static partial Regex SetPcPattern();

    [GeneratedRegex(@"^s_getpc_b64 s\[(?<lo>\d+):(?<hi>\d+)\]")]
    private static partial Regex GetPcPattern();

    [GeneratedRegex(@"^s_(?<op>add|sub)_u32 s(?<dst>\d+), s(?<src>\d+), (?<imm>0x[0-9a-f]+|-?\d+)$")]
    private static partial Regex AddPattern();

    [GeneratedRegex(@"^s_(?<op>addc|subb)_u32 s(?<dst>\d+), s(?<src>\d+), 0$")]
    private static partial Regex CarryPattern();
}
