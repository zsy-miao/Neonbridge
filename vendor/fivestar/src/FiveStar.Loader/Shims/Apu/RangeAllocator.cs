// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// First-fit allocation of aligned ranges inside [0, capacity), with freed neighbours merged back. Not thread-safe;
/// the owner locks.
/// </summary>
internal sealed class RangeAllocator
{
    private readonly SortedDictionary<ulong, ulong> _free = [];
    private readonly Dictionary<ulong, ulong> _used = [];

    public RangeAllocator(ulong capacity)
    {
        Capacity = capacity;

        if (capacity > 0)
        {
            _free[0] = capacity;
        }
    }

    public ulong Capacity { get; }

    /// <summary>The bytes handed out and not yet freed.</summary>
    public ulong UsedBytes { get; private set; }

    /// <summary>The offset of a new range of <paramref name="size"/> bytes aligned to <paramref name="alignment"/> (a power of two), or null when nothing fits.</summary>
    public ulong? Allocate(ulong size, ulong alignment)
    {
        if (size == 0 || alignment == 0 || (alignment & (alignment - 1)) != 0)
        {
            return null;
        }

        foreach (var (start, length) in _free)
        {
            ulong aligned = (start + alignment - 1) & ~(alignment - 1);
            ulong end = start + length;

            if (aligned + size > end)
            {
                continue;
            }

            Take(start, end, aligned, size);

            return aligned;
        }

        return null;
    }

    /// <summary>Returns the range at <paramref name="offset"/>; false when no range starts there.</summary>
    public bool Free(ulong offset)
    {
        if (!_used.Remove(offset, out ulong size))
        {
            return false;
        }

        UsedBytes -= size;
        ulong start = offset;
        ulong end = offset + size;
        var before = _free.LastOrDefault(pair => pair.Key < start);

        if (before.Value != 0 && before.Key + before.Value == start)
        {
            _free.Remove(before.Key);
            start = before.Key;
        }

        if (_free.Remove(end, out ulong after))
        {
            end += after;
        }

        _free[start] = end - start;

        return true;
    }

    /// <summary>Whether [offset, offset + size) lies inside one allocated range.</summary>
    public bool Contains(ulong offset, ulong size)
    {
        foreach (var (start, length) in _used)
        {
            if (offset >= start && offset + size <= start + length)
            {
                return true;
            }
        }

        return false;
    }

    private void Take(ulong start, ulong end, ulong aligned, ulong size)
    {
        _free.Remove(start);

        if (aligned > start)
        {
            _free[start] = aligned - start;
        }

        if (aligned + size < end)
        {
            _free[aligned + size] = end - (aligned + size);
        }

        _used[aligned] = size;
        UsedBytes += size;
    }
}
