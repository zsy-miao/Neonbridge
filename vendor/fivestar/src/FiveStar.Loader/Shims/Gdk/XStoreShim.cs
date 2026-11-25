// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XStore.h contexts, licenses and license events; the work is in <see cref="StoreService"/>. Acquiring an add-on or
/// package license needs the store, so it fails with E_GAMESTORE_NETWORK_ERROR and no license handle is issued.
/// </summary>
internal static unsafe partial class XStoreShim
{
    /// <summary>HRESULT XStoreCreateContext(XUserHandle user, XStoreContextHandle* storeContextHandle).</summary>
    [Shim(GdkModule.Name, "XStoreCreateContext")]
    [UnmanagedCallersOnly]
    private static int XStoreCreateContext(nint user, nint* context)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Store.CreateContext(context);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreCreateContext");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XStoreCloseContextHandle")]
    [UnmanagedCallersOnly]
    private static void XStoreCloseContextHandle(nint context)
    {
        try
        {
            ShimContext.Runtime.Gdk.Store.CloseContext(context);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreCloseContextHandle");
        }
    }

    /// <summary>HRESULT XStoreQueryGameLicenseAsync(const XStoreContextHandle storeContextHandle, XAsyncBlock* async).</summary>
    [Shim(GdkModule.Name, StoreService.GameLicenseApi)]
    [UnmanagedCallersOnly]
    private static int XStoreQueryGameLicenseAsync(nint context, nint async)
    {
        try
        {
            return AsyncCalls.BeginStore(context, async, StoreService.GameLicenseApi, new GameLicenseOperation(ShimContext.Runtime.Gdk.Store));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreQueryGameLicenseAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryGameLicenseResult(XAsyncBlock* async, XStoreGameLicense* license).</summary>
    [Shim(GdkModule.Name, "XStoreQueryGameLicenseResult")]
    [UnmanagedCallersOnly]
    private static int XStoreQueryGameLicenseResult(nint async, byte* license)
    {
        try
        {
            if (license == null)
            {
                return HResult.Pointer;
            }

            return AsyncCalls.Result(async, StoreService.GameLicenseApi, StoreService.GameLicenseSize, (nint)license);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreQueryGameLicenseResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreAcquireLicenseForPackageAsync(XStoreContextHandle, const char* packageIdentifier, XAsyncBlock* async).</summary>
    [Shim(GdkModule.Name, nameof(XStoreAcquireLicenseForPackageAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreAcquireLicenseForPackageAsync(nint context, byte* packageIdentifier, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreAcquireLicenseForPackageAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreAcquireLicenseForPackageAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreAcquireLicenseForPackageResult(XAsyncBlock* async, XStoreLicenseHandle* storeLicenseHandle).</summary>
    [Shim(GdkModule.Name, "XStoreAcquireLicenseForPackageResult")]
    [UnmanagedCallersOnly]
    private static int XStoreAcquireLicenseForPackageResult(nint async, nint* license)
    {
        try
        {
            return AsyncCalls.HandleResult(async, nameof(XStoreAcquireLicenseForPackageAsync), license);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreAcquireLicenseForPackageResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreAcquireLicenseForDurablesAsync(XStoreContextHandle, const char* storeId, XAsyncBlock* async).</summary>
    [Shim(GdkModule.Name, nameof(XStoreAcquireLicenseForDurablesAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreAcquireLicenseForDurablesAsync(nint context, byte* storeId, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreAcquireLicenseForDurablesAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreAcquireLicenseForDurablesAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreAcquireLicenseForDurablesResult(XAsyncBlock* async, XStoreLicenseHandle* storeLicenseHandle).</summary>
    [Shim(GdkModule.Name, "XStoreAcquireLicenseForDurablesResult")]
    [UnmanagedCallersOnly]
    private static int XStoreAcquireLicenseForDurablesResult(nint async, nint* license)
    {
        try
        {
            return AsyncCalls.HandleResult(async, nameof(XStoreAcquireLicenseForDurablesAsync), license);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreAcquireLicenseForDurablesResult");

            return HResult.Fail;
        }
    }

    /// <summary>bool XStoreIsLicenseValid(XStoreLicenseHandle): no license handle is ever issued, so none is valid.</summary>
    [Shim(GdkModule.Name, "XStoreIsLicenseValid")]
    [UnmanagedCallersOnly]
    private static byte XStoreIsLicenseValid(nint license)
    {
        return GdkModule.False;
    }

    [Shim(GdkModule.Name, "XStoreCloseLicenseHandle")]
    [UnmanagedCallersOnly]
    private static void XStoreCloseLicenseHandle(nint license)
    {
    }

    /// <summary>HRESULT XStoreRegisterGameLicenseChanged(XStoreContextHandle, XTaskQueueHandle, void* context, callback, XTaskQueueRegistrationToken*).</summary>
    [Shim(GdkModule.Name, "XStoreRegisterGameLicenseChanged")]
    [UnmanagedCallersOnly]
    private static int XStoreRegisterGameLicenseChanged(nint store, nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (!gdk.Store.IsContext(store))
            {
                return HResult.InvalidArgument;
            }

            return gdk.Registrations.Register("XStoreRegisterGameLicenseChanged", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreRegisterGameLicenseChanged");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreRegisterPackageLicenseLost(XStoreLicenseHandle, XTaskQueueHandle, void* context, callback, XTaskQueueRegistrationToken*).</summary>
    [Shim(GdkModule.Name, "XStoreRegisterPackageLicenseLost")]
    [UnmanagedCallersOnly]
    private static int XStoreRegisterPackageLicenseLost(nint license, nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XStoreRegisterPackageLicenseLost", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreRegisterPackageLicenseLost");

            return HResult.Fail;
        }
    }

    /// <summary>bool XStoreUnregister*(handle, XTaskQueueRegistrationToken token, bool wait).</summary>
    [Shim(GdkModule.Name, "XStoreUnregisterGameLicenseChanged")]
    [Shim(GdkModule.Name, "XStoreUnregisterPackageLicenseLost")]
    [UnmanagedCallersOnly]
    private static byte XStoreUnregister(nint handle, ulong token, byte wait)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Unregister(token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreUnregister");

            return default;
        }
    }

    /// <summary>HRESULT XStoreQueryPackageIdentifier(const char* storeId, size_t size, char* packageIdentifier).</summary>
    [Shim(GdkModule.Name, "XStoreQueryPackageIdentifier")]
    [UnmanagedCallersOnly]
    private static int XStoreQueryPackageIdentifier(byte* storeId, nuint size, byte* packageIdentifier)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Store.QueryPackageIdentifier(Utf8Text.Read((nint)storeId), size, packageIdentifier);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XStoreQueryPackageIdentifier), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreShim.XStoreQueryPackageIdentifier");

            return HResult.Fail;
        }
    }
}
