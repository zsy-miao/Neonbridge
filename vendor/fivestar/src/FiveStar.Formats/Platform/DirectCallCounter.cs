// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Platform;

/// <summary>
/// Counts the direct calls into a set of functions: <c>call rel32</c> to the function or to a <c>jmp rel32</c> stub
/// that forwards to it (incremental-link thunks), and tail calls (<c>jmp rel32</c> nobody calls). Calls through a
/// pointer are not seen. The scan is byte-wise, so it can also count a byte pair inside another instruction that
/// happens to land exactly on a function start; that is rare enough for a call-site tally.
/// </summary>
public static class DirectCallCounter
{
    private const string CodeSectionPrefix = ".text";
    private const byte CallRel32 = 0xE8;
    private const byte JmpRel32 = 0xE9;
    private const int BranchLength = 5;
    private const int BranchDisplacement = 1;

    /// <summary>The number of direct calls into each of <paramref name="functions"/> (every function has an entry).</summary>
    public static IReadOnlyDictionary<uint, int> Count(PeImage image, IReadOnlySet<uint> functions)
    {
        var counts = functions.ToDictionary(function => function, _ => 0);
        var jumps = Branches(image, JmpRel32, functions);
        var stubs = jumps.ToDictionary(jump => jump.Source, jump => jump.Target);
        var calledStubs = new HashSet<uint>();
        var callTargets = new HashSet<uint>(functions);
        callTargets.UnionWith(stubs.Keys);

        foreach (var (_, target) in Branches(image, CallRel32, callTargets))
        {
            if (counts.TryGetValue(target, out var direct))
            {
                counts[target] = direct + 1;
            }
            else if (stubs.TryGetValue(target, out var function))
            {
                counts[function]++;
                calledStubs.Add(target);
            }
        }

        foreach (var (source, target) in jumps)
        {
            if (!calledStubs.Contains(source))
            {
                counts[target]++;
            }
        }

        return counts;
    }

    private static List<(uint Source, uint Target)> Branches(PeImage image, byte opcode, IReadOnlySet<uint> targets)
    {
        var branches = new List<(uint Source, uint Target)>();

        foreach (var section in image.Sections.Where(section => section.Name.StartsWith(CodeSectionPrefix, StringComparison.Ordinal)))
        {
            var start = (int)image.RawStart(section);
            var length = (int)Math.Min(section.RawSize, section.VirtualSize);
            var code = image.Data.AsSpan(start, Math.Min(length, image.Data.Length - start));

            for (var at = 0; at + BranchLength <= code.Length; at++)
            {
                if (code[at] != opcode)
                {
                    continue;
                }

                var source = (uint)(section.VirtualAddress + at);
                var target = (uint)(source + BranchLength + BinaryPrimitives.ReadInt32LittleEndian(code[(at + BranchDisplacement)..]));

                if (targets.Contains(target))
                {
                    branches.Add((source, target));
                }
            }
        }

        return branches;
    }
}
