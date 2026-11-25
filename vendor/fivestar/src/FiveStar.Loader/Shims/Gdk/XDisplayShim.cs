// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XDisplay.h entry points; the work is in <see cref="DisplayService"/>.</summary>
internal static unsafe partial class XDisplayShim
{
    [Shim(GdkModule.Name, "XDisplayTryEnableHdrMode")]
    [UnmanagedCallersOnly]
    private static uint XDisplayTryEnableHdrMode(uint preference, float* info)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Display.TryEnableHdr(preference, info);
            }
            catch (Exception e)
            {
                gdk.Boundary.Report(nameof(XDisplayTryEnableHdrMode), e);

                return DisplayService.HdrDisabled;
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDisplayShim.XDisplayTryEnableHdrMode");

            return default;
        }
    }

    [Shim(GdkModule.Name, "XDisplayAcquireTimeoutDeferral")]
    [UnmanagedCallersOnly]
    private static int XDisplayAcquireTimeoutDeferral(nint* handle)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Display.AcquireDeferral(handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDisplayShim.XDisplayAcquireTimeoutDeferral");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XDisplayCloseTimeoutDeferralHandle")]
    [UnmanagedCallersOnly]
    private static void XDisplayCloseTimeoutDeferralHandle(nint handle)
    {
        try
        {
            ShimContext.Runtime.Gdk.Display.CloseDeferral(handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XDisplayShim.XDisplayCloseTimeoutDeferralHandle");
        }
    }
}
