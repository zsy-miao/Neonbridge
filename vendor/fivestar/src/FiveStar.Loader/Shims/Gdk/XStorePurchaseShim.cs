// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XStore.h purchase UI, consumables and user ids. There is no store UI offline, so purchase, product page and redeem
/// end as canceled (E_ABORT); consumable balances, fulfillment and the user collections/purchase ids need the store
/// service and fail with E_GAMESTORE_NETWORK_ERROR.
/// </summary>
internal static unsafe partial class XStorePurchaseShim
{
    /// <summary>HRESULT XStoreShowPurchaseUIAsync(XStoreContextHandle, const char* storeId, const char* name, const char* extendedJsonData, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreShowPurchaseUIAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreShowPurchaseUIAsync(nint context, byte* storeId, byte* name, byte* json, nint async)
    {
        try
        {
            return BeginCanceled(context, async, nameof(XStoreShowPurchaseUIAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreShowPurchaseUIAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XStoreShowPurchaseUIResult")]
    [UnmanagedCallersOnly]
    private static int XStoreShowPurchaseUIResult(nint async)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreShowPurchaseUIAsync), 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreShowPurchaseUIResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreShowProductPageUIAsync(XStoreContextHandle, const char* storeId, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreShowProductPageUIAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreShowProductPageUIAsync(nint context, byte* storeId, nint async)
    {
        try
        {
            return BeginCanceled(context, async, nameof(XStoreShowProductPageUIAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreShowProductPageUIAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XStoreShowProductPageUIResult")]
    [UnmanagedCallersOnly]
    private static int XStoreShowProductPageUIResult(nint async)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreShowProductPageUIAsync), 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreShowProductPageUIResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreShowRedeemTokenUIAsync(XStoreContextHandle, const char* token, const char** allowedStoreIds, size_t count, bool disallowCsvRedemption, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreShowRedeemTokenUIAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreShowRedeemTokenUIAsync(nint context, byte* token, nint allowedStoreIds, nuint count, byte disallowCsv, nint async)
    {
        try
        {
            return BeginCanceled(context, async, nameof(XStoreShowRedeemTokenUIAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreShowRedeemTokenUIAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XStoreShowRedeemTokenUIResult")]
    [UnmanagedCallersOnly]
    private static int XStoreShowRedeemTokenUIResult(nint async)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreShowRedeemTokenUIAsync), 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreShowRedeemTokenUIResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryConsumableBalanceRemainingAsync(XStoreContextHandle, const char* storeProductId, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreQueryConsumableBalanceRemainingAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreQueryConsumableBalanceRemainingAsync(nint context, byte* storeProductId, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreQueryConsumableBalanceRemainingAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreQueryConsumableBalanceRemainingAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryConsumableBalanceRemainingResult(XAsyncBlock*, XStoreConsumableResult*).</summary>
    [Shim(GdkModule.Name, "XStoreQueryConsumableBalanceRemainingResult")]
    [UnmanagedCallersOnly]
    private static int XStoreQueryConsumableBalanceRemainingResult(nint async, uint* consumable)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreQueryConsumableBalanceRemainingAsync), sizeof(uint), (nint)consumable);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreQueryConsumableBalanceRemainingResult");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// HRESULT XStoreReportConsumableFulfillmentAsync(XStoreContextHandle, const char* storeProductId, uint32_t quantity,
    /// GUID trackingId, XAsyncBlock*); the 16-byte GUID travels by reference (x64 ABI).
    /// </summary>
    [Shim(GdkModule.Name, nameof(XStoreReportConsumableFulfillmentAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreReportConsumableFulfillmentAsync(nint context, byte* storeProductId, uint quantity, nint trackingId, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreReportConsumableFulfillmentAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreReportConsumableFulfillmentAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreReportConsumableFulfillmentResult(XAsyncBlock*, XStoreConsumableResult*).</summary>
    [Shim(GdkModule.Name, "XStoreReportConsumableFulfillmentResult")]
    [UnmanagedCallersOnly]
    private static int XStoreReportConsumableFulfillmentResult(nint async, uint* consumable)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreReportConsumableFulfillmentAsync), sizeof(uint), (nint)consumable);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreReportConsumableFulfillmentResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreGetUserCollectionsIdAsync(XStoreContextHandle, const char* serviceTicket, const char* publisherUserId, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreGetUserCollectionsIdAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreGetUserCollectionsIdAsync(nint context, byte* serviceTicket, byte* publisherUserId, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreGetUserCollectionsIdAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreGetUserCollectionsIdAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreGetUserPurchaseIdAsync(XStoreContextHandle, const char* serviceTicket, const char* publisherUserId, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreGetUserPurchaseIdAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreGetUserPurchaseIdAsync(nint context, byte* serviceTicket, byte* publisherUserId, nint async)
    {
        try
        {
            return AsyncCalls.BeginStoreOffline(context, async, nameof(XStoreGetUserPurchaseIdAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreGetUserPurchaseIdAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreGetUser{Collections,Purchase}IdResultSize(XAsyncBlock*, size_t* size).</summary>
    [Shim(GdkModule.Name, "XStoreGetUserCollectionsIdResultSize")]
    [Shim(GdkModule.Name, "XStoreGetUserPurchaseIdResultSize")]
    [UnmanagedCallersOnly]
    private static int XStoreGetUserIdResultSize(nint async, nuint* size)
    {
        try
        {
            return AsyncCalls.EmptyCount(async, size);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreGetUserIdResultSize");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreGetUserCollectionsIdResult(XAsyncBlock*, size_t size, char* result).</summary>
    [Shim(GdkModule.Name, "XStoreGetUserCollectionsIdResult")]
    [UnmanagedCallersOnly]
    private static int XStoreGetUserCollectionsIdResult(nint async, nuint size, byte* result)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreGetUserCollectionsIdAsync), size, (nint)result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreGetUserCollectionsIdResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreGetUserPurchaseIdResult(XAsyncBlock*, size_t size, char* result).</summary>
    [Shim(GdkModule.Name, "XStoreGetUserPurchaseIdResult")]
    [UnmanagedCallersOnly]
    private static int XStoreGetUserPurchaseIdResult(nint async, nuint size, byte* result)
    {
        try
        {
            return AsyncCalls.Result(async, nameof(XStoreGetUserPurchaseIdAsync), size, (nint)result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStorePurchaseShim.XStoreGetUserPurchaseIdResult");

            return HResult.Fail;
        }
    }

    private static int BeginCanceled(nint context, nint async, string api)
    {
        ShimContext.Runtime.Gdk.Boundary.Info($"{api}: no store UI offline; ends as canceled");

        return AsyncCalls.BeginStore(context, async, api, new OfflineOperation(HResult.Abort));
    }
}
