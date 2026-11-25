// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Shims;

/// <summary>Allocation bases a shim handed out, so the memory map (--memory-map) can name their regions.</summary>
internal sealed class AllocationBases
{
    private readonly ConcurrentDictionary<nint, byte> _bases = new();

    public void Add(nint allocationBase)
    {
        if (allocationBase != 0)
        {
            _bases.TryAdd(allocationBase, 0);
        }
    }

    public void Remove(nint allocationBase)
    {
        _bases.TryRemove(allocationBase, out _);
    }

    public bool Contains(nint allocationBase)
    {
        return _bases.ContainsKey(allocationBase);
    }
}
