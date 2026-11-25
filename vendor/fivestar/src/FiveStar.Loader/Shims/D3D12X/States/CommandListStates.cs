// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.States;

/// <summary>The <see cref="ListResourceStates"/> of every command-list wrapper that recorded a barrier.</summary>
internal sealed unsafe class CommandListStates
{
    /// <summary>ID3D12CommandList::GetType (d3d12.h).</summary>
    private const int GetTypeSlot = 8;

    private readonly ConcurrentDictionary<nint, ListResourceStates> _lists = new();

    /// <summary>The states of <paramref name="list"/>, made on its first barrier.</summary>
    [HotPath]
    public ListResourceStates For(XboxObject* list)
    {
        if (_lists.TryGetValue((nint)list, out var states))
        {
            return states;
        }

        return Add(list);
    }

    /// <summary>The states of <paramref name="list"/> when it ever recorded a barrier, else null.</summary>
    [HotPath]
    public ListResourceStates? Find(XboxObject* list)
    {
        _lists.TryGetValue((nint)list, out var states);

        return states;
    }

    private ListResourceStates Add(XboxObject* list)
    {
        nint inner = list->Inner;
        int type = ((delegate* unmanaged<nint, int>)ComObject.Slot(inner, GetTypeSlot))(inner);

        return _lists.GetOrAdd((nint)list, new ListResourceStates(ResourceStates.Legal(type)));
    }
}
