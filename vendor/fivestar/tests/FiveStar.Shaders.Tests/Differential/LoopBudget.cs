// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// Stops endless loops: synthetic data can send a real program's loop (a BVH walk, a search) around forever, and WARP
/// cannot cancel a running shader. Every translated loop gets <c>if (diff_budget()) break;</c> at its top; after
/// <see cref="Steps"/> iterations of one invocation the budget sets word 0 of heap slot <see cref="FlagSlot"/> and
/// leaves the loop. Such a run means nothing: the dispatch form counts blocks and the structured form loop iterations.
/// </summary>
public static class LoopBudget
{
    public const int FlagSlot = 52;
    public const int Steps = 1 << 12;

    private const string Include = "#include \"rdna_prelude.hlsli\"";

    private static readonly string[] Loops = ["[loop] for (;;) {", "[loop] while (pc < PC_YIELD) {"];

    public static string Insert(string hlsl)
    {
        var include = hlsl.IndexOf(Include, StringComparison.Ordinal);

        if (include < 0)
        {
            throw new NotSupportedException("no prelude include");
        }

        var function = string.Create(
            CultureInfo.InvariantCulture,
            $"\nstatic uint g_diff_steps = 0;\nbool diff_budget() {{\n  if (++g_diff_steps < {Steps}u) return false;\n"
            + $"  RWByteAddressBuffer f = ResourceDescriptorHeap[{FlagSlot}];\n  f.Store(0, 1u);\n  return true;\n}}\n");
        var text = hlsl.Insert(include + Include.Length, function);

        foreach (var loop in Loops)
        {
            text = text.Replace(loop, loop + " if (diff_budget()) break;", StringComparison.Ordinal);
        }

        return text;
    }
}
