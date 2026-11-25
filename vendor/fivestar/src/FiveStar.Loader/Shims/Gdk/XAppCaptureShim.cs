// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XAppCapture.h: capture is reported unavailable; the rest of XAppCapture is in the not-implemented catalog.</summary>
internal static unsafe partial class XAppCaptureShim
{
    /// <summary>HRESULT XAppCaptureTakeScreenshot(XUserHandle requestingUser, XAppCaptureTakeScreenshotResult* result).</summary>
    [Shim(GdkModule.Name, "XAppCaptureTakeScreenshot")]
    [UnmanagedCallersOnly]
    private static int XAppCaptureTakeScreenshot(nint user, nint result)
    {
        try
        {
            ShimContext.Runtime.Gdk.Boundary.Info("XAppCaptureTakeScreenshot not supported");

            return HResult.NotSupported;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XAppCaptureShim.XAppCaptureTakeScreenshot");

            return HResult.Fail;
        }
    }
}
