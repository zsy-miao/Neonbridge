// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// Group barriers between LDS writes and later LDS reads in the group-wave variant (<see cref="HlslTranslator.GroupWaveVariant"/>)
/// of a group of more than 32 threads. The console runs the group as one wave in lockstep, so a lane reads what another
/// wrote without s_barrier; 32-lane hardware runs it as two waves, and such a read races the other wave's write
/// (shadPS4's shared_memory_barrier_pass inserts these barriers for one-wave groups; facts only). The variant's control
/// flow is group-uniform (its branch conditions are wave operations, lane conditions are statements on <c>active</c>),
/// so a barrier may go at any statement outside a lane-conditioned block. A wave operation is a barrier already. A read
/// after a branch is ordered when either path wrote; a loop whose body writes orders the reads of its body from the
/// first iteration. An atomic needs ordering only after a plain store: atomics on LDS are atomic between waves.
/// </summary>
internal static partial class GroupLdsBarriers
{
    public const string Barrier = "  GroupMemoryBarrierWithGroupSync();   // LDS written by the other hardware wave";

    private static readonly string[] WaveOperations = ["GroupMemoryBarrierWithGroupSync", "vw_group_", "vw_read(", "vw_read_fill(", "vw_ballot(", "vw_any(", "vw_count("];

    /// <summary>The program with the barriers its LDS reads need.</summary>
    public static string Insert(string hlsl)
    {
        var lines = hlsl.Split('\n').ToList();
        int main = lines.FindIndex(line => line.Contains(" main(", StringComparison.Ordinal));

        if (main < 0)
        {
            return hlsl;
        }

        var frames = new Stack<Frame>();
        var inserts = new SortedSet<int>();
        var pending = Pending.None;

        for (int index = main + 1; index < lines.Count; index++)
        {
            string line = lines[index].Trim();

            if (line.StartsWith('}'))
            {
                pending = Close(frames, pending, line);
            }

            var access = Access(line);

            if (Needs(access, pending))
            {
                inserts.Add(BarrierLine(frames, index));
                pending = Pending.None;
            }

            pending |= access.Writes;

            if (WaveOperations.Any(operation => line.Contains(operation, StringComparison.Ordinal)))
            {
                pending = Pending.None;
            }

            if (line.EndsWith('{') && !line.StartsWith('}'))
            {
                bool loop = line.Contains("for (", StringComparison.Ordinal) || line.Contains("while (", StringComparison.Ordinal);
                frames.Push(new Frame(pending, index, line.StartsWith("if (active", StringComparison.Ordinal)));

                if (loop)
                {
                    pending |= BodyWrites(lines, index);
                }
            }
        }

        foreach (int at in inserts.Reverse())
        {
            lines.Insert(at, Barrier);
        }

        return string.Join('\n', lines);
    }

    /// <summary>Whether a line's accesses need a barrier first: a plain read after any write, an atomic after a plain store.</summary>
    private static bool Needs((bool PlainRead, bool Atomic, Pending Writes) access, Pending pending)
    {
        return (access.PlainRead && pending != Pending.None) || (access.Atomic && (pending & Pending.Plain) != 0);
    }

    /// <summary>
    /// The state after a block's closing line: the path through the block or past it (an if without else, a loop run
    /// no times); <c>} else {</c> starts the else path from the state before the if and keeps the then path's end.
    /// </summary>
    private static Pending Close(Stack<Frame> frames, Pending pending, string line)
    {
        if (frames.Count == 0)
        {
            return pending;
        }

        var frame = frames.Pop();

        if (line.StartsWith("} else", StringComparison.Ordinal) && line.EndsWith('{'))
        {
            frames.Push(frame with { ThenEnd = pending | frame.ThenEnd });

            return frame.AtOpen;
        }

        return pending | frame.AtOpen | frame.ThenEnd;
    }

    /// <summary>
    /// Where a barrier goes for a read at <paramref name="index"/>: before the outermost lane-conditioned block around it,
    /// whose blocks then open with nothing pending; else at the read.
    /// </summary>
    private static int BarrierLine(Stack<Frame> frames, int index)
    {
        var open = frames.Reverse().ToList();
        int outermost = open.FindIndex(frame => frame.LaneCondition);

        if (outermost < 0)
        {
            return index;
        }

        frames.Clear();

        for (int i = 0; i < open.Count; i++)
        {
            var frame = open[i];

            if (i >= outermost)
            {
                frame = frame with { AtOpen = Pending.None };
            }

            frames.Push(frame);
        }

        return open[outermost].Opener;
    }

    /// <summary>The writes of the block opened at <paramref name="opener"/>.</summary>
    private static Pending BodyWrites(List<string> lines, int opener)
    {
        int depth = 0;
        var writes = Pending.None;

        for (int index = opener; index < lines.Count; index++)
        {
            string line = lines[index];

            if (index > opener)
            {
                writes |= Access(line.Trim()).Writes;
            }

            depth += line.Count(character => character == '{') - line.Count(character => character == '}');

            if (depth <= 0)
            {
                break;
            }
        }

        return writes;
    }

    /// <summary>A line's LDS accesses: a plain read (any access but a store's target), an atomic, and the writes it leaves.</summary>
    private static (bool PlainRead, bool Atomic, Pending Writes) Access(string line)
    {
        bool atomic = line.Contains("Interlocked", StringComparison.Ordinal) && line.Contains("(g_lds[", StringComparison.Ordinal);
        int accesses = AccessPattern().Count(line);
        int stores = StorePattern().Count(line);
        int atomics = AtomicPattern().Count(line);
        var writes = Pending.None;

        if (stores > 0)
        {
            writes |= Pending.Plain;
        }

        if (atomic)
        {
            writes |= Pending.Atomic;
        }

        return (accesses > stores + atomics, atomic, writes);
    }

    [GeneratedRegex(@"g_lds\[")]
    private static partial Regex AccessPattern();

    [GeneratedRegex(@"g_lds\[[^;]*?\]\s*=(?!=)")]
    private static partial Regex StorePattern();

    [GeneratedRegex(@"Interlocked\w*\(g_lds\[")]
    private static partial Regex AtomicPattern();

    /// <summary>Kinds of LDS writes not yet ordered by a barrier.</summary>
    [Flags]
    private enum Pending
    {
        None = 0,
        Plain = 1,
        Atomic = 2,
    }

    /// <summary>An open block: the writes pending before it, its line, whether a lane condition opens it.</summary>
    private sealed record Frame(Pending AtOpen, int Opener, bool LaneCondition)
    {
        public Pending ThenEnd { get; init; }
    }
}
