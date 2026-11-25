// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// The shared halves of the shim-side *Async / *Result entry points (XStore, XUser tokens, XNetworking URL queries):
/// starting a <see cref="ManagedAsyncOperation"/> and reading its result, with exceptions turned into HRESULTs.
/// </summary>
internal static unsafe class AsyncCalls
{
    /// <summary>Starts <paramref name="operation"/> for <paramref name="api"/>.</summary>
    public static int Begin(nint async, string api, ManagedAsyncOperation operation)
    {
        var gdk = ShimContext.Runtime.Gdk;

        try
        {
            return gdk.Async.BeginManaged(async, api, operation);
        }
        catch (Exception e)
        {
            return gdk.Boundary.Failed(api, e);
        }
    }

    /// <summary>Starts <paramref name="operation"/> for <paramref name="api"/> on a valid store context.</summary>
    public static int BeginStore(nint context, nint async, string api, ManagedAsyncOperation operation)
    {
        if (!ShimContext.Runtime.Gdk.Store.IsContext(context))
        {
            return HResult.InvalidArgument;
        }

        return Begin(async, api, operation);
    }

    /// <summary>Starts a store operation that ends with E_GAMESTORE_NETWORK_ERROR.</summary>
    public static int BeginStoreOffline(nint context, nint async, string api)
    {
        return BeginStore(context, async, api, new OfflineOperation(StoreService.NetworkError));
    }

    /// <summary>A *Result that returns a handle: zeroed first, so a failed call leaves no stale handle behind.</summary>
    public static int HandleResult(nint async, string api, nint* handle)
    {
        if (handle == null)
        {
            return HResult.Pointer;
        }

        *handle = 0;

        return Result(async, api, (nuint)sizeof(nint), (nint)handle);
    }

    /// <summary>A *Result of <paramref name="size"/> bytes (0 when the call has no result).</summary>
    public static int Result(nint async, string api, nuint size, nint buffer)
    {
        var gdk = ShimContext.Runtime.Gdk;

        try
        {
            return gdk.Async.ManagedResult(async, api, size, buffer);
        }
        catch (Exception e)
        {
            return gdk.Boundary.Failed(api, e);
        }
    }

    /// <summary>A *ResultCount or *ResultSize of a call with an empty result: the call's status, and 0.</summary>
    public static int EmptyCount<T>(nint async, T* count)
        where T : unmanaged
    {
        if (count == null)
        {
            return HResult.Pointer;
        }

        *count = default;
        int status = ShimContext.Runtime.Gdk.Async.GetStatus(async, false);

        if (status == HResult.Pending || HResult.Failed(status))
        {
            return status;
        }

        return HResult.Ok;
    }
}
