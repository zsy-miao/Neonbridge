// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.States;

/// <summary>
/// The textures one command list moved out of their home state since it was last closed, with the state each is in
/// now, and the states the list's type may name in a barrier. A list is recorded by one thread at a time, so nothing
/// here locks.
/// </summary>
internal sealed class ListResourceStates
{
    /// <summary>Textures tracked before the table first grows; it doubles when full.</summary>
    public const int InitialCapacity = 64;

    private const int NotFound = -1;
    private const int GrowthFactor = 2;

    private nint[] _resources = new nint[InitialCapacity];
    private int[] _states = new int[InitialCapacity];
    private int[] _homes = new int[InitialCapacity];

    public ListResourceStates(int legalStates)
    {
        LegalStates = legalStates;
    }

    public int Count { get; private set; }

    /// <summary>The D3D12_RESOURCE_STATES bits a barrier on this list may name (<see cref="ResourceStates.Legal"/>).</summary>
    public int LegalStates { get; }

    /// <summary>The state <paramref name="resource"/> is in on this list: the tracked one, else <paramref name="home"/>.</summary>
    [HotPath]
    public int Current(nint resource, int home)
    {
        int index = IndexOf(resource);

        if (index < 0)
        {
            return home;
        }

        return _states[index];
    }

    /// <summary>Records <paramref name="resource"/> in <paramref name="state"/>.</summary>
    [HotPath]
    public void Set(nint resource, int home, int state)
    {
        int index = IndexOf(resource);

        if (index < 0)
        {
            if (Count == _resources.Length)
            {
                Grow();
            }

            index = Count;
            Count++;
            _resources[index] = resource;
            _homes[index] = home;
        }

        _states[index] = state;
    }

    /// <summary>The tracked texture at <paramref name="index"/> with its home and current state.</summary>
    [HotPath]
    public (nint Resource, int Home, int State) At(int index)
    {
        return (_resources[index], _homes[index], _states[index]);
    }

    [HotPath]
    public void Clear()
    {
        Count = 0;
    }

    private void Grow()
    {
        int size = _resources.Length * GrowthFactor;
        Array.Resize(ref _resources, size);
        Array.Resize(ref _states, size);
        Array.Resize(ref _homes, size);
    }

    [HotPath]
    private int IndexOf(nint resource)
    {
        for (int i = 0; i < Count; i++)
        {
            if (_resources[i] == resource)
            {
                return i;
            }
        }

        return NotFound;
    }
}
