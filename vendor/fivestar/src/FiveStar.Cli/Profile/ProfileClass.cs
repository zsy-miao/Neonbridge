// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Profile;

/// <summary>The samples of one thread class: counts, threads and methods (<see cref="ManagedProfileSummary"/>).</summary>
internal sealed class ProfileClass
{
    public ProfileClass(string name)
    {
        Name = name;
    }

    public string Name { get; }

    /// <summary>Samples of threads running managed code.</summary>
    public int Managed { get; private set; }

    /// <summary>Samples of threads outside managed code: in native code, or waiting.</summary>
    public int External { get; private set; }

    public HashSet<int> Threads { get; } = [];

    /// <summary>Managed samples by innermost method.</summary>
    public Dictionary<string, int> Exclusive { get; } = new(StringComparer.Ordinal);

    /// <summary>Managed samples by every method on the stack, once per sample.</summary>
    public Dictionary<string, int> Inclusive { get; } = new(StringComparer.Ordinal);

    /// <summary>Counts a sample of <paramref name="thread"/> with its <paramref name="frames"/>, innermost first.</summary>
    public void Add(int thread, bool managed, IReadOnlyList<string> frames)
    {
        Threads.Add(thread);

        if (!managed)
        {
            External++;

            return;
        }

        Managed++;

        if (frames.Count == 0)
        {
            return;
        }

        Exclusive[frames[0]] = Exclusive.GetValueOrDefault(frames[0]) + 1;

        foreach (var method in frames.Distinct(StringComparer.Ordinal))
        {
            Inclusive[method] = Inclusive.GetValueOrDefault(method) + 1;
        }
    }
}
