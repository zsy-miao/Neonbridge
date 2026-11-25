// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Desktop descriptors of the shader-visible view heap handed out in turn, for descriptors the game writes into its
/// CPU-only view heaps. Every write takes fresh descriptors, so a record the game copied earlier keeps naming the view
/// it copied until the ring wraps (the console copies descriptor bytes). Lock-free.
/// </summary>
internal sealed class DescriptorRing
{
    private readonly uint _first;
    private readonly uint _size;
    private uint _next;

    /// <param name="first">The desktop index of the ring's first descriptor.</param>
    /// <param name="size">Descriptors in the ring.</param>
    public DescriptorRing(uint first, uint size)
    {
        _first = first;
        _size = size;
    }

    /// <summary><paramref name="count"/> consecutive desktop indices; <see cref="DescriptorRemap.Unmapped"/> when the ring is smaller.</summary>
    [HotPath]
    public uint Next(uint count)
    {
        if (count == 0 || count > _size)
        {
            return DescriptorRemap.Unmapped;
        }

        while (true)
        {
            uint start = Volatile.Read(ref _next);
            uint begin = start;
            uint end = start + count;

            if (end > _size)
            {
                begin = 0;
                end = count;
            }

            if (Interlocked.CompareExchange(ref _next, end, start) == start)
            {
                return _first + begin;
            }
        }
    }
}
