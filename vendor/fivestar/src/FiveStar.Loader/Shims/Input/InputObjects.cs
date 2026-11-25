// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// IUnknown of the GameInput objects and the way back from a native object to its session. Every object counts its
/// references; IGameInput and the devices live as long as the session even at 0, readings return to the history ring.
/// </summary>
internal static unsafe class InputObjects
{
    /// <summary>The session that owns a native object.</summary>
    [HotPath]
    public static GameInputSession Session(nint self)
    {
        return (GameInputSession)GCHandle.FromIntPtr(((ObjectHeader*)self)->Owner).Target!;
    }

    [UnmanagedCallersOnly]
    public static uint AddRef(nint self)
    {
        try
        {
            return (uint)Interlocked.Increment(ref ((ObjectHeader*)self)->References);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "InputObjects.AddRef");

            return default;
        }
    }

    [UnmanagedCallersOnly]
    public static uint Release(nint self)
    {
        try
        {
            return Drop(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "InputObjects.Release");

            return default;
        }
    }

    /// <summary>Gives up one reference; a count that would go below 0 stays at 0.</summary>
    [HotPath]
    public static uint Drop(nint self)
    {
        ref int references = ref ((ObjectHeader*)self)->References;
        int count = Interlocked.Decrement(ref references);

        if (count < 0)
        {
            Interlocked.Increment(ref references);

            return 0;
        }

        return (uint)count;
    }

    /// <summary>QueryInterface for one interface: IUnknown and <paramref name="expected"/> answer with the object itself.</summary>
    public static int Query(nint self, Guid* iid, nint* result, Guid expected)
    {
        if (iid == null || result == null)
        {
            return HResult.Pointer;
        }

        if (*iid != expected && *iid != GameInputLayout.UnknownIid)
        {
            *result = 0;
            Session(self).LogQuery(*iid);

            return HResult.NoInterface;
        }

        Interlocked.Increment(ref ((ObjectHeader*)self)->References);
        *result = self;

        return HResult.Ok;
    }
}
