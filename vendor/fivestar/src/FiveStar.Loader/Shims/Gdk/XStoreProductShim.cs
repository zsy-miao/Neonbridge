// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XStore.h product queries: the offline user owns no add-ons and the catalog is unreachable, so every query returns
/// an empty, single-page XStoreProductQueryHandle.
/// </summary>
internal static unsafe partial class XStoreProductShim
{
    /// <summary>HRESULT XStoreQueryAssociatedProductsAsync(XStoreContextHandle, XStoreProductKind, uint32_t maxItemsToRetrievePerPage, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreQueryAssociatedProductsAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreQueryAssociatedProductsAsync(nint context, uint kinds, uint pageSize, nint async)
    {
        try
        {
            return BeginQuery(context, async, nameof(XStoreQueryAssociatedProductsAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreQueryAssociatedProductsAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryAssociatedProductsResult(XAsyncBlock*, XStoreProductQueryHandle*).</summary>
    [Shim(GdkModule.Name, "XStoreQueryAssociatedProductsResult")]
    [UnmanagedCallersOnly]
    private static int XStoreQueryAssociatedProductsResult(nint async, nint* query)
    {
        try
        {
            return AsyncCalls.HandleResult(async, nameof(XStoreQueryAssociatedProductsAsync), query);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreQueryAssociatedProductsResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryEntitledProductsAsync(XStoreContextHandle, XStoreProductKind, uint32_t maxItemsToRetrievePerPage, XAsyncBlock*).</summary>
    [Shim(GdkModule.Name, nameof(XStoreQueryEntitledProductsAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreQueryEntitledProductsAsync(nint context, uint kinds, uint pageSize, nint async)
    {
        try
        {
            return BeginQuery(context, async, nameof(XStoreQueryEntitledProductsAsync));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreQueryEntitledProductsAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreQueryEntitledProductsResult(XAsyncBlock*, XStoreProductQueryHandle*).</summary>
    [Shim(GdkModule.Name, "XStoreQueryEntitledProductsResult")]
    [UnmanagedCallersOnly]
    private static int XStoreQueryEntitledProductsResult(nint async, nint* query)
    {
        try
        {
            return AsyncCalls.HandleResult(async, nameof(XStoreQueryEntitledProductsAsync), query);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreQueryEntitledProductsResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreEnumerateProductsQuery(XStoreProductQueryHandle, void* context, XStoreProductQueryCallback*): no products, so no callback.</summary>
    [Shim(GdkModule.Name, "XStoreEnumerateProductsQuery")]
    [UnmanagedCallersOnly]
    private static int XStoreEnumerateProductsQuery(nint query, nint context, nint callback)
    {
        try
        {
            if (callback == 0 || !ShimContext.Runtime.Gdk.Store.IsProductQuery(query))
            {
                return HResult.InvalidArgument;
            }

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreEnumerateProductsQuery");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XStoreProductsQueryHasMorePages")]
    [UnmanagedCallersOnly]
    private static byte XStoreProductsQueryHasMorePages(nint query)
    {
        return GdkModule.False;
    }

    /// <summary>HRESULT XStoreProductsQueryNextPageAsync(XStoreProductQueryHandle, XAsyncBlock*): there is no next page to fetch.</summary>
    [Shim(GdkModule.Name, nameof(XStoreProductsQueryNextPageAsync))]
    [UnmanagedCallersOnly]
    private static int XStoreProductsQueryNextPageAsync(nint query, nint async)
    {
        try
        {
            if (!ShimContext.Runtime.Gdk.Store.IsProductQuery(query))
            {
                return HResult.InvalidArgument;
            }

            return AsyncCalls.Begin(async, nameof(XStoreProductsQueryNextPageAsync), new OfflineOperation(StoreService.NetworkError));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreProductsQueryNextPageAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XStoreProductsQueryNextPageResult(XAsyncBlock*, XStoreProductQueryHandle*).</summary>
    [Shim(GdkModule.Name, "XStoreProductsQueryNextPageResult")]
    [UnmanagedCallersOnly]
    private static int XStoreProductsQueryNextPageResult(nint async, nint* query)
    {
        try
        {
            return AsyncCalls.HandleResult(async, nameof(XStoreProductsQueryNextPageAsync), query);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreProductsQueryNextPageResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XStoreCloseProductsQueryHandle")]
    [UnmanagedCallersOnly]
    private static void XStoreCloseProductsQueryHandle(nint query)
    {
        try
        {
            ShimContext.Runtime.Gdk.Store.CloseProductQuery(query);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XStoreProductShim.XStoreCloseProductsQueryHandle");
        }
    }

    private static int BeginQuery(nint context, nint async, string api)
    {
        return AsyncCalls.BeginStore(context, async, api, new ProductQueryOperation(ShimContext.Runtime.Gdk.Store));
    }
}
