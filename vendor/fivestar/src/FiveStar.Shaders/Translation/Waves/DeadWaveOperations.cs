// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using FiveStar.Shaders.Translation.Structure;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// Drops the wave operations whose result nothing reads, before the superstep form cuts the blocks: every wave operation
/// there is a group round. The translator writes each lane-mask result into its SGPR halves (a ballot per half) and
/// each SALU mask operation's scc (<c>vw_any</c>), and most of those are overwritten before any read:
/// MSNC_VS_LinearDepthSkin_f1f2249c stopped 611 times, 424 of them at a ballot into an SGPR half and 100 at an scc.
/// A ballot (<c>uint _t = vw_ballot(..).x;</c> with <c>reg = _t;</c>) or an scc write is dropped when the register is
/// dead after it, by liveness over the block graph; wave operations have no other effect, so the program computes the
/// same. Writes inside braces do not count as kills, and a block whose successors are unknown keeps every register live.
/// </summary>
internal static partial class DeadWaveOperations
{
    private const int MaxPasses = 8;
    private const string Scc = "scc";

    /// <summary>A ballot's temporary is named twice: where it is set and where it is copied to its SGPR half.</summary>
    private const int TempMentions = 2;

    /// <summary>Removes dead wave operations from <paramref name="blocks"/> in place; <paramref name="pinned"/> are read after the program.</summary>
    public static int Remove(List<CodeBlock> blocks, IReadOnlySet<string> pinned)
    {
        BallotBits.Substitute(blocks);
        int removed = 0;

        for (int pass = 0; pass < MaxPasses; pass++)
        {
            int now = RemoveOnce(blocks, pinned);
            removed += now;

            if (now == 0)
            {
                break;
            }
        }

        return removed;
    }

    private static int RemoveOnce(List<CodeBlock> blocks, IReadOnlySet<string> pinned)
    {
        var names = Candidates(blocks);

        if (names.Count == 0)
        {
            return 0;
        }

        var liveIn = LiveIn(blocks, names, pinned);
        var graph = new FlowGraph(blocks);
        int removed = 0;

        for (int block = 0; block < blocks.Count; block++)
        {
            var live = LiveOut(graph, block, liveIn, names, pinned);
            removed += Sweep(blocks[block].Lines, live, names);
        }

        return removed;
    }

    /// <summary>The registers a dead-candidate line writes: ballot targets and scc.</summary>
    private static HashSet<string> Candidates(List<CodeBlock> blocks)
    {
        var names = new HashSet<string>(StringComparer.Ordinal);

        foreach (var lines in blocks.Select(block => block.Lines))
        {
            for (int i = 0; i < lines.Count; i++)
            {
                if (Ballot(lines, i) is string target)
                {
                    names.Add(target);
                }
                else if (SccPattern().IsMatch(lines[i]))
                {
                    names.Add(Scc);
                }
            }
        }

        return names;
    }

    /// <summary>The register a ballot at line <paramref name="i"/> writes through the next line, or null.</summary>
    private static string? Ballot(List<string> lines, int i)
    {
        var ballot = BallotPattern().Match(lines[i]);

        if (!ballot.Success || i + 1 >= lines.Count)
        {
            return null;
        }

        var copy = CopyPattern().Match(lines[i + 1]);

        string temp = ballot.Groups["temp"].Value;

        // The temporary must have no reader but the copy.
        if (!copy.Success || copy.Groups["value"].Value != temp || lines.Count(line => Regex.IsMatch(line, $@"\b{temp}\b")) != TempMentions)
        {
            return null;
        }

        return copy.Groups["name"].Value;
    }

    private static List<HashSet<string>> LiveIn(List<CodeBlock> blocks, HashSet<string> names, IReadOnlySet<string> pinned)
    {
        var graph = new FlowGraph(blocks);
        var liveIn = blocks.Select(_ => new HashSet<string>(StringComparer.Ordinal)).ToList();

        for (bool changed = true; changed;)
        {
            changed = false;

            for (int block = blocks.Count - 1; block >= 0; block--)
            {
                var live = LiveOut(graph, block, liveIn, names, pinned);
                Backward(blocks[block].Lines, live, names, lines: null);

                if (!live.SetEquals(liveIn[block]))
                {
                    liveIn[block] = live;
                    changed = true;
                }
            }
        }

        return liveIn;
    }

    /// <summary>What is live after a block: its successors' live-in; at the program's end the pinned registers.</summary>
    private static HashSet<string> LiveOut(FlowGraph graph, int block, List<HashSet<string>> liveIn, HashSet<string> names, IReadOnlySet<string> pinned)
    {
        var live = new HashSet<string>(StringComparer.Ordinal);

        foreach (int next in graph.Successors[block])
        {
            if (next == graph.End)
            {
                live.UnionWith(names.Where(pinned.Contains));
            }
            else
            {
                live.UnionWith(liveIn[next]);
            }
        }

        if (graph.Successors[block].Count == 0)
        {
            live.UnionWith(names);
        }

        return live;
    }

    /// <summary>Drops the block's dead ballots and scc writes (walking backward from <paramref name="live"/>); how many.</summary>
    private static int Sweep(List<string> lines, HashSet<string> live, HashSet<string> names)
    {
        var dead = new List<int>();
        Backward(lines, live, names, dead);

        foreach (int index in dead.OrderDescending())
        {
            lines.RemoveAt(index);
        }

        return dead.Count;
    }

    /// <summary>
    /// Walks the lines backward from <paramref name="live"/>, leaving the live-in set in it; with <paramref name="lines"/>
    /// collects the indices of dead candidate lines (a ballot's two lines, an scc write).
    /// </summary>
    private static void Backward(List<string> block, HashSet<string> live, HashSet<string> names, List<int>? lines)
    {
        var depths = Depths(block);

        for (int i = block.Count - 1; i >= 0; i--)
        {
            if (i > 0 && depths[i - 1] == 0 && Ballot(block, i - 1) is string target && !live.Contains(target))
            {
                lines?.AddRange([i - 1, i]);
                i--;

                continue;
            }

            if (depths[i] == 0 && SccPattern().IsMatch(block[i]) && !live.Contains(Scc))
            {
                lines?.Add(i);

                continue;
            }

            var write = depths[i] == 0 ? WritePattern().Match(block[i]) : Match.Empty;
            string reads = block[i];

            if (write.Success)
            {
                live.Remove(write.Groups["name"].Value);
                reads = write.Groups["value"].Value;
            }

            foreach (Match name in NamePattern().Matches(reads))
            {
                if (names.Contains(name.Value))
                {
                    live.Add(name.Value);
                }
            }
        }
    }

    /// <summary>The brace depth before each line (0 at the block's top level).</summary>
    private static int[] Depths(List<string> lines)
    {
        var depths = new int[lines.Count];
        int depth = 0;

        for (int i = 0; i < lines.Count; i++)
        {
            depths[i] = depth;
            depth += lines[i].Count(character => character == '{') - lines[i].Count(character => character == '}');
        }

        return depths;
    }

    [GeneratedRegex(@"^\s*uint (?<temp>_t\d+) = vw_ballot\([^;]*\)\.[xy];\s*$")]
    private static partial Regex BallotPattern();

    [GeneratedRegex(@"^\s*(?<name>[A-Za-z_]\w*) = (?<value>_t\d+);\s*$")]
    private static partial Regex CopyPattern();

    [GeneratedRegex(@"^\s*scc = vw_any\([^;]*\);\s*$")]
    private static partial Regex SccPattern();

    [GeneratedRegex(@"^\s*(?<name>[A-Za-z_]\w*)\s*=(?!=)\s*(?<value>[^;{}]*);\s*$")]
    private static partial Regex WritePattern();

    [GeneratedRegex(@"(?<![.\w])[A-Za-z_]\w*")]
    private static partial Regex NamePattern();
}
