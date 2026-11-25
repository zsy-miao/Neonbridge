// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// The console wave a thread belongs to when a program wave of <c>wave</c> lanes runs on hardware waves of
/// <c>hardware</c> lanes: the threads of its hardware wave that share its program-lane origin. On a hardware wave at
/// least as wide that is the whole console wave; on a narrower one it is the part of it the hardware wave holds (the
/// other lanes are missing). Lanes come from the thread's hardware lane, or from its group index with
/// <c>groupLanes</c> (mesh shaders).
/// </summary>
public sealed class ProgramWave
{
    private readonly int _wave;
    private readonly int _hardware;
    private readonly bool _groupLanes;

    public ProgramWave(int wave, int hardware, bool groupLanes)
    {
        _wave = wave;
        _hardware = hardware;
        _groupLanes = groupLanes;
    }

    /// <summary>The program lane of a thread (its group index).</summary>
    public int Lane(int thread)
    {
        if (_groupLanes)
        {
            return thread & (_wave - 1);
        }

        return (thread % _hardware) & (_wave - 1);
    }

    /// <summary>The threads of the console wave <paramref name="thread"/> is in that the hardware runs with it.</summary>
    public IReadOnlyList<int> Members(int thread)
    {
        var hardwareStart = thread - (thread % _hardware);

        return [.. Enumerable.Range(hardwareStart, _hardware).Where(other => Origin(other) == Origin(thread))];
    }

    /// <summary>The thread holding program lane <paramref name="lane"/> in this thread's wave, or null when missing.</summary>
    public int? Holder(int thread, int lane)
    {
        var members = Members(thread).Where(other => Lane(other) == (lane & (_wave - 1))).ToList();

        if (members.Count == 0)
        {
            return null;
        }

        return members[0];
    }

    /// <summary>The 64-bit lane mask of the wave's lanes where <paramref name="condition"/> holds.</summary>
    public ulong Ballot(int thread, Func<int, bool> condition)
    {
        return Members(thread).Where(condition).Aggregate(0UL, (mask, other) => mask | (1UL << Lane(other)));
    }

    private int Origin(int thread)
    {
        return (thread % _hardware) - Lane(thread);
    }
}
