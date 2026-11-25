// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XTaskQueueRegistrationTokens for GDK events that never happen offline (invites, activations, connectivity, device
/// association, audio endpoints, package installs). Registering checks the arguments and hands out a token; the
/// callback is never called. Unregistering a live token returns true.
/// </summary>
internal sealed unsafe class EventRegistrations
{
    private readonly ConcurrentDictionary<ulong, string> _live = new();
    private long _next;

    public int Count => _live.Count;

    /// <summary>The Register* half: E_INVALIDARG without a callback, E_POINTER without a token pointer.</summary>
    public int Register(string api, nint callback, ulong* token)
    {
        if (token == null)
        {
            return HResult.Pointer;
        }

        if (callback == 0)
        {
            return HResult.InvalidArgument;
        }

        ulong issued = (ulong)Interlocked.Increment(ref _next);
        _live[issued] = api;
        *token = issued;

        return HResult.Ok;
    }

    /// <summary>The Unregister* half; true when the token was live.</summary>
    public byte Unregister(ulong token)
    {
        if (_live.TryRemove(token, out _))
        {
            return GdkModule.True;
        }

        return GdkModule.False;
    }
}
