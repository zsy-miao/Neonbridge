// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XGameInvite.h, XGameActivation.h and XGameProtocol.h: offline there are no invites and nothing activates the game,
/// so each Register* hands out a token whose callback never runs. The accept functions stay in the catalog.
/// </summary>
internal static unsafe partial class XGameInviteShim
{
    /// <summary>HRESULT XGameInviteRegisterForEvent(XTaskQueueHandle, void* context, XGameInviteEventCallback*, XTaskQueueRegistrationToken*).</summary>
    [Shim(GdkModule.Name, "XGameInviteRegisterForEvent")]
    [UnmanagedCallersOnly]
    private static int XGameInviteRegisterForEvent(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XGameInviteRegisterForEvent", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameInviteShim.XGameInviteRegisterForEvent");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameInviteRegisterForPendingEvent")]
    [UnmanagedCallersOnly]
    private static int XGameInviteRegisterForPendingEvent(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XGameInviteRegisterForPendingEvent", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameInviteShim.XGameInviteRegisterForPendingEvent");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameActivationRegisterForEvent")]
    [UnmanagedCallersOnly]
    private static int XGameActivationRegisterForEvent(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XGameActivationRegisterForEvent", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameInviteShim.XGameActivationRegisterForEvent");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameProtocolRegisterForActivation")]
    [UnmanagedCallersOnly]
    private static int XGameProtocolRegisterForActivation(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XGameProtocolRegisterForActivation", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameInviteShim.XGameProtocolRegisterForActivation");

            return HResult.Fail;
        }
    }

    /// <summary>bool X*Unregister*(XTaskQueueRegistrationToken token, bool wait).</summary>
    [Shim(GdkModule.Name, "XGameInviteUnregisterForEvent")]
    [Shim(GdkModule.Name, "XGameInviteUnregisterForPendingEvent")]
    [Shim(GdkModule.Name, "XGameActivationUnregisterForEvent")]
    [Shim(GdkModule.Name, "XGameProtocolUnregisterForActivation")]
    [UnmanagedCallersOnly]
    private static byte Unregister(ulong token, byte wait)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Unregister(token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameInviteShim.Unregister");

            return default;
        }
    }
}
