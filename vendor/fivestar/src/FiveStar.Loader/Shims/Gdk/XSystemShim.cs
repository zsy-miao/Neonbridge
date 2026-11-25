// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XSystem.h, XThread.h, XGame.h, XGameEvent.h, XError.h and XPersistentLocalStorage.h entry points; the work is in
/// <see cref="SystemService"/>.
/// </summary>
internal static unsafe partial class XSystemShim
{
    [Shim(GdkModule.Name, "XSystemGetDeviceType")]
    [UnmanagedCallersOnly]
    private static uint XSystemGetDeviceType()
    {
        return ShimContext.Runtime.Gdk.System.DeviceType;
    }

    [Shim(GdkModule.Name, "XSystemGetAnalyticsInfo")]
    [UnmanagedCallersOnly]
    private static byte* XSystemGetAnalyticsInfo(byte* result)
    {
        try
        {
            return ShimContext.Runtime.Gdk.System.GetAnalyticsInfo(result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XSystemGetAnalyticsInfo");

            return default;
        }
    }

    /// <summary>HRESULT XSystemGetConsoleId(size_t consoleIdSize, char* consoleId, size_t* consoleIdUsed); XSystemConsoleIdBytes = 39.</summary>
    [Shim(GdkModule.Name, "XSystemGetConsoleId")]
    [UnmanagedCallersOnly]
    private static int XSystemGetConsoleId(nuint size, byte* id, nuint* used)
    {
        try
        {
            return SystemService.GetConsoleId(size, id, used);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XSystemGetConsoleId");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XSystemGetXboxLiveSandboxId")]
    [UnmanagedCallersOnly]
    private static int XSystemGetXboxLiveSandboxId(nuint size, byte* id, nuint* used)
    {
        try
        {
            return SystemService.GetSandboxId(size, id, used);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XSystemGetXboxLiveSandboxId");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XSystemGetAppSpecificDeviceId")]
    [UnmanagedCallersOnly]
    private static int XSystemGetAppSpecificDeviceId(nuint size, byte* id, nuint* used)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.System.GetAppSpecificDeviceId(size, id, used);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XSystemGetAppSpecificDeviceId), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XSystemGetAppSpecificDeviceId");

            return HResult.Fail;
        }
    }

    /// <summary>void XSystemAllowFullDownloadBandwidth(bool enable): nothing downloads offline.</summary>
    [Shim(GdkModule.Name, "XSystemAllowFullDownloadBandwidth")]
    [UnmanagedCallersOnly]
    private static void XSystemAllowFullDownloadBandwidth(byte enable)
    {
    }

    /// <summary>bool XSystemIsHandleValid(XSystemHandle): handle tracking is off, so any non-null handle counts as valid.</summary>
    [Shim(GdkModule.Name, "XSystemIsHandleValid")]
    [UnmanagedCallersOnly]
    private static byte XSystemIsHandleValid(nint handle)
    {
        if (handle == 0)
        {
            return GdkModule.False;
        }

        return GdkModule.True;
    }

    /// <summary>HRESULT XThreadSetTimeSensitive(bool isTimeSensitiveThread): per thread.</summary>
    [Shim(GdkModule.Name, "XThreadSetTimeSensitive")]
    [UnmanagedCallersOnly]
    private static int XThreadSetTimeSensitive(byte isTimeSensitive)
    {
        try
        {
            ShimContext.Runtime.Gdk.System.SetTimeSensitive(isTimeSensitive != 0);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XThreadSetTimeSensitive");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XThreadIsTimeSensitive")]
    [UnmanagedCallersOnly]
    private static byte XThreadIsTimeSensitive()
    {
        try
        {
            bool sensitive = ShimContext.Runtime.Gdk.System.IsTimeSensitive();
            byte result = sensitive ? GdkModule.True : GdkModule.False;

            return result;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XThreadIsTimeSensitive");

            return default;
        }
    }

    [Shim(GdkModule.Name, "XThreadAssertNotTimeSensitive")]
    [UnmanagedCallersOnly]
    private static void XThreadAssertNotTimeSensitive()
    {
        try
        {
            ShimContext.Runtime.Gdk.System.AssertNotTimeSensitive();
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XThreadAssertNotTimeSensitive");
        }
    }

    /// <summary>
    /// XThreadVerifyNotTimeSensitive: exported by the PC library, declared in no public header. Warns like
    /// XThreadAssertNotTimeSensitive and returns S_OK, which also suits a void caller.
    /// </summary>
    [Shim(GdkModule.Name, "XThreadVerifyNotTimeSensitive")]
    [UnmanagedCallersOnly]
    private static int XThreadVerifyNotTimeSensitive()
    {
        try
        {
            ShimContext.Runtime.Gdk.System.AssertNotTimeSensitive();

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XThreadVerifyNotTimeSensitive");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameGetXboxTitleId")]
    [UnmanagedCallersOnly]
    private static int XGameGetXboxTitleId(uint* titleId)
    {
        try
        {
            return ShimContext.Runtime.Gdk.System.GetTitleId(titleId);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XGameGetXboxTitleId");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameEventWrite(XUserHandle, const char* serviceConfigId, const char* playSessionId, const char* eventName, const char* dimensionsJson, const char* measurementsJson).</summary>
    [Shim(GdkModule.Name, "XGameEventWrite")]
    [UnmanagedCallersOnly]
    private static int XGameEventWrite(nint user, byte* serviceConfigId, byte* playSessionId, byte* eventName, byte* dimensions, byte* measurements)
    {
        try
        {
            return ShimContext.Runtime.Gdk.System.WriteEvent(eventName);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XGameEventWrite");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XErrorSetCallback(XErrorCallback*, void* context): accepted, never called.</summary>
    [Shim(GdkModule.Name, "XErrorSetCallback")]
    [UnmanagedCallersOnly]
    private static int XErrorSetCallback(nint callback, nint context)
    {
        return HResult.Ok;
    }

    /// <summary>void XErrorSetOptions(XErrorOptions optionsDebuggerPresent, XErrorOptions optionsDebuggerNotPresent).</summary>
    [Shim(GdkModule.Name, "XErrorSetOptions")]
    [UnmanagedCallersOnly]
    private static void XErrorSetOptions(uint debuggerPresent, uint debuggerNotPresent)
    {
    }

    [Shim(GdkModule.Name, "XPersistentLocalStorageGetPathSize")]
    [UnmanagedCallersOnly]
    private static int XPersistentLocalStorageGetPathSize(nuint* size)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.System.GetLocalStoragePathSize(size);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPersistentLocalStorageGetPathSize), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XPersistentLocalStorageGetPathSize");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XPersistentLocalStorageGetPath")]
    [UnmanagedCallersOnly]
    private static int XPersistentLocalStorageGetPath(nuint size, byte* path, nuint* used)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.System.GetLocalStoragePath(size, path, used);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPersistentLocalStorageGetPath), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XPersistentLocalStorageGetPath");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XPersistentLocalStoragePromptUserForSpaceAsync(uint64_t requestedBytes, XAsyncBlock* asyncBlock).</summary>
    [Shim(GdkModule.Name, SystemService.PromptForSpaceApi)]
    [UnmanagedCallersOnly]
    private static int XPersistentLocalStoragePromptUserForSpaceAsync(ulong requested, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Async.BeginManaged(async, SystemService.PromptForSpaceApi, new LocalStorageSpaceOperation(gdk.System, requested));
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPersistentLocalStoragePromptUserForSpaceAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XPersistentLocalStoragePromptUserForSpaceAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XPersistentLocalStoragePromptUserForSpaceResult")]
    [UnmanagedCallersOnly]
    private static int XPersistentLocalStoragePromptUserForSpaceResult(nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Async.ManagedResult(async, SystemService.PromptForSpaceApi, 0, 0);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPersistentLocalStoragePromptUserForSpaceResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XPersistentLocalStoragePromptUserForSpaceResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XPersistentLocalStorageGetSpaceInfo")]
    [UnmanagedCallersOnly]
    private static int XPersistentLocalStorageGetSpaceInfo(ulong* info)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.System.GetLocalStorageSpaceInfo(info);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XPersistentLocalStorageGetSpaceInfo), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XSystemShim.XPersistentLocalStorageGetSpaceInfo");

            return HResult.Fail;
        }
    }
}
