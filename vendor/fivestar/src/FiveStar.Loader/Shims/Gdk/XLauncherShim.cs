// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XLauncher.h and XGame.h launch entry points; the work is in <see cref="LauncherService"/>.</summary>
internal static unsafe partial class XLauncherShim
{
    /// <summary>HRESULT XLaunchUri(XUserHandle requestingUser, const char* uri).</summary>
    [Shim(GdkModule.Name, "XLaunchUri")]
    [UnmanagedCallersOnly]
    private static int XLaunchUri(nint user, byte* uri)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Launcher.LaunchUri(Utf8Text.Read((nint)uri));
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XLaunchUri), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XLauncherShim.XLaunchUri");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XLaunchRestartOnCrash(const char* args, uint32_t reserved).</summary>
    [Shim(GdkModule.Name, "XLaunchRestartOnCrash")]
    [UnmanagedCallersOnly]
    private static int XLaunchRestartOnCrash(byte* arguments, uint reserved)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Launcher.RestartOnCrash(Utf8Text.Read((nint)arguments));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XLauncherShim.XLaunchRestartOnCrash");

            return HResult.Fail;
        }
    }

    /// <summary>void XLaunchNewGame(const char* exePath, const char* args, XUserHandle defaultUser).</summary>
    [Shim(GdkModule.Name, "XLaunchNewGame")]
    [UnmanagedCallersOnly]
    private static void XLaunchNewGame(byte* exePath, byte* arguments, nint user)
    {
        try
        {
            ShimContext.Runtime.Gdk.Launcher.LaunchNewGame(Utf8Text.Read((nint)exePath), Utf8Text.Read((nint)arguments));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XLauncherShim.XLaunchNewGame");
        }
    }
}
