// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>Counts of <see cref="GpuResourceEvent"/>s, updated without locks.</summary>
internal sealed class GpuResourceCounters
{
    private readonly long[] _counts = new long[Enum.GetValues<GpuResourceEvent>().Length];

    [HotPath]
    public long Add(GpuResourceEvent kind)
    {
        return Interlocked.Increment(ref _counts[(int)kind]);
    }

    public long this[GpuResourceEvent kind] => Volatile.Read(ref _counts[(int)kind]);

    public override string ToString()
    {
        return string.Join(", ", Enum.GetValues<GpuResourceEvent>().Select(kind => $"{kind} {this[kind]}"));
    }
}
