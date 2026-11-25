// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XStore.h package updates and downloads. The local package is the only version there is, so the update query finds
/// none; downloading anything needs the store and fails with E_GAMESTORE_NETWORK_ERROR.
/// </summary>
internal static unsafe partial class XStorePackageShim
{
    /// <summary>HRESULT XStoreQueryGameAndDlcPackageUpdatesAsync(XStoreContextHandle, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreQueryGameAndDlcPackageUpdatesAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreQueryGameAndDlcPackageUpdatesAsync(nint context, nint async)
    {
        try
        {
            return AsyncCalls.BeginStore(context, async, nameof(XStoreQueryGameAndDlcPackageUpdatesAsync), new OfflineOperation(HResult.Ok));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePackageShim.XStoreQueryGameAndDlcPackageUpdatesAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryGameAndDlcPackageUpdatesResultCount(XAsyncBlock*, uint32_t* count).</summary>
    [Shim(GdkModule.Name, "XStoreQueryGameAndDlcPackageUpdatesResultCount")]
    [Shim(GdkModule.Name, "XStoreDownloadAndInstallPackagesResultCount")]
    [UnmanagedCallersOnly]
    private static int XStoreResultCount(nint async, uint* count)
    {
        try
        {
            return AsyncCalls.EmptyCount(async, count);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePackageShim.XStoreResultCount");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryGameAndDlcPackageUpdatesResult(XAsyncBlock*, uint32_t count, XStorePackageUpdate* packageUpdates).</summary>
    [Shim(GdkModule.Name, "XStoreQueryGameAndDlcPackageUpdatesResult")]
    [UnmanagedCallersOnly]
    private static int XStoreQueryGameAndDlcPackageUpdatesResult(nint async, uint count, nint updates)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreQueryGameAndDlcPackageUpdatesAsync), 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePackageShim.XStoreQueryGameAndDlcPackageUpdatesResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreDownloadAndInstallPackageUpdatesAsync(XStoreContextHandle, const char** packageIdentifiers, size_t count, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreDownloadAndInstallPackageUpdatesAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreDownloadAndInstallPackageUpdatesAsync(nint context, nint packageIdentifiers, nuint count, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreDownloadAndInstallPackageUpdatesAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePackageShim.XStoreDownloadAndInstallPackageUpdatesAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XStoreDownloadAndInstallPackageUpdatesResult")]
    [UnmanagedCallersOnly]
    private static int XStoreDownloadAndInstallPackageUpdatesResult(nint async)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreDownloadAndInstallPackageUpdatesAsync), 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePackageShim.XStoreDownloadAndInstallPackageUpdatesResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreDownloadAndInstallPackagesAsync(XStoreContextHandle, const char** storeIds, size_t count, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreDownloadAndInstallPackagesAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreDownloadAndInstallPackagesAsync(nint context, nint storeIds, nuint count, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreDownloadAndInstallPackagesAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePackageShim.XStoreDownloadAndInstallPackagesAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreDownloadAndInstallPackagesResult(XAsyncBlock*, uint32_t count, char packageIdentifiers[][XPACKAGE_IDENTIFIER_MAX_LENGTH]).</summary>
    [Shim(GdkModule.Name, "XStoreDownloadAndInstallPackagesResult")]
    [UnmanagedCallersOnly]
    private static int XStoreDownloadAndInstallPackagesResult(nint async, uint count, nint packageIdentifiers)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreDownloadAndInstallPackagesAsync), 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePackageShim.XStoreDownloadAndInstallPackagesResult");

            return HResult.Fail;
        }
    }
}
