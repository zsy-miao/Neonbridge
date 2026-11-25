// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>The IAcpHal slots (<see cref="AcpHalLayout"/>); the object lives as long as the process.</summary>
internal static unsafe class AcpHalMethods
{
    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, uint, uint, int> initialize = &Initialize;
        delegate* unmanaged<nint, int> shutdown = &Shutdown;
        delegate* unmanaged<nint, uint, uint, uint, uint*, uint, int> sendMessage = &SendMessage;
        delegate* unmanaged<nint, int> unknown = &Unknown;
        delegate* unmanaged<nint, uint> release = &Release;

        vtable[AcpHalLayout.InitializeSlot] = (nint)initialize;
        vtable[AcpHalLayout.ShutdownSlot] = (nint)shutdown;
        vtable[AcpHalLayout.SendMessageSlot] = (nint)sendMessage;
        vtable[AcpHalLayout.UnknownSlot3] = (nint)unknown;
        vtable[AcpHalLayout.UnknownSlot4] = (nint)unknown;
        vtable[AcpHalLayout.ReleaseSlot] = (nint)release;
    }

    /// <summary>Called with (0x1000, 0x1000) right after AcpHalCreate; the game fails start-up on a negative result.</summary>
    [UnmanagedCallersOnly]
    private static int Initialize(nint self, uint first, uint second)
    {
        return HResult.Ok;
    }

    [UnmanagedCallersOnly]
    private static int Shutdown(nint self)
    {
        return HResult.Ok;
    }

    [UnmanagedCallersOnly]
    private static int SendMessage(nint self, uint type, uint cookie, uint timeout, uint* payload, uint flags)
    {
        try
        {
            return ShimContext.Runtime.AcpHal.SendMessage(type, cookie, payload);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AcpHalMethods.SendMessage");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int Unknown(nint self)
    {
        return HResult.NotImplemented;
    }

    [UnmanagedCallersOnly]
    private static uint Release(nint self)
    {
        return 0;
    }
}
