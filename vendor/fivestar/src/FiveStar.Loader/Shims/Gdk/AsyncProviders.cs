// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// The XAsyncProvider callbacks the shims pass to XAsyncBegin: one for XAsyncRun (context = the title's work
/// callback) and one for shim-side operations (context = a GCHandle of a <see cref="ManagedAsyncOperation"/>).
/// </summary>
internal static unsafe class AsyncProviders
{
    [HotPath]
    [UnmanagedCallersOnly]
    public static int RunProvider(uint operation, nint* data)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                nint block = data[AsyncLayout.DataAsyncSlot];

                switch ((AsyncOperation)operation)
                {
                    case AsyncOperation.Begin:
                        return gdk.Async.Schedule(block, 0);
                    case AsyncOperation.DoWork:
                        return RunWork(block, data[AsyncLayout.DataContextSlot]);
                    default:
                        return HResult.Ok;
                }
            }
            catch (Exception e)
            {
                ReportError(gdk.Boundary, "XAsyncRun provider", e);

                return HResult.Fail;
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AsyncProviders.RunProvider");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static int ManagedProvider(uint operation, nint* data)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;
            var handle = GCHandle.FromIntPtr(data[AsyncLayout.DataContextSlot]);
            var managed = (ManagedAsyncOperation)handle.Target!;
            nint block = data[AsyncLayout.DataAsyncSlot];

            try
            {
                switch ((AsyncOperation)operation)
                {
                    case AsyncOperation.Begin:
                        return gdk.Async.Schedule(block, 0);
                    case AsyncOperation.DoWork:
                        return DoManagedWork(gdk.Async, block, managed);
                    case AsyncOperation.GetResult:
                        return managed.Write((byte*)data[AsyncLayout.DataBufferSlot], (nuint)data[AsyncLayout.DataBufferSizeSlot]);
                    case AsyncOperation.Cleanup:
                        return FreeOperation(handle);
                    default:
                        // Cancel: the managed work is short and not cancelable.
                        return HResult.NotImplemented;
                }
            }
            catch (Exception e)
            {
                ReportError(gdk.Boundary, "managed async op", e);

                return HResult.Fail;
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "AsyncProviders.ManagedProvider");

            return HResult.Fail;
        }
    }

    /// <summary>XAsyncWork: <c>HRESULT CALLBACK XAsyncWork(XAsyncBlock* asyncBlock)</c>.</summary>
    private static int RunWork(nint block, nint work)
    {
        return ((delegate* unmanaged<nint, int>)work)(block);
    }

    private static int DoManagedWork(AsyncService async, nint block, ManagedAsyncOperation managed)
    {
        var result = managed.Work();
        nuint size = HResult.Failed(result.HResult) ? 0 : result.Size;
        async.Complete(block, result.HResult, size);

        return HResult.Ok;
    }

    private static void ReportError(NativeBoundary boundary, string what, Exception exception)
    {
        boundary.Error($"{what}: {exception.Message}");
    }

    private static int FreeOperation(GCHandle handle)
    {
        handle.Free();

        return HResult.Ok;
    }
}
