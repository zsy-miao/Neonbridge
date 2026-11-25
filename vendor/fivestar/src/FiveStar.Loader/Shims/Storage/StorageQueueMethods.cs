// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>The implemented IDStorageQueueX slots. Every other slot is a not-implemented stub.</summary>
internal static unsafe class StorageQueueMethods
{
    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, uint> addRef = &AddRef;
        delegate* unmanaged<nint, uint> release = &Release;

        delegate* unmanaged<nint, byte*, void> enqueueRequest = &EnqueueRequest;
        delegate* unmanaged<nint, nint, uint, void> enqueueStatus = &EnqueueStatus;
        delegate* unmanaged<nint, nint, ulong, void> enqueueSignal = &EnqueueSignal;
        delegate* unmanaged<nint, void> submit = &Submit;
        delegate* unmanaged<nint, void> close = &Close;

        vtable[DStorageLayout.AddRefSlot] = (nint)addRef;
        vtable[DStorageLayout.ReleaseSlot] = (nint)release;

        vtable[DStorageLayout.QueueEnqueueRequest] = (nint)enqueueRequest;
        vtable[DStorageLayout.QueueEnqueueStatus] = (nint)enqueueStatus;
        vtable[DStorageLayout.QueueEnqueueSignal] = (nint)enqueueSignal;
        vtable[DStorageLayout.QueueSubmit] = (nint)submit;
        vtable[DStorageLayout.QueueClose] = (nint)close;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static uint AddRef(nint self)
    {
        try
        {
            return StorageObjects.AddRef(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageQueueMethods.AddRef");

            return default;
        }
    }

    /// <summary>The last reference closes the queue (waiting for the request in progress) and frees it.</summary>
    [UnmanagedCallersOnly]
    private static uint Release(nint self)
    {
        try
        {
            if (!StorageObjects.Drop(self, out uint left))
            {
                return left;
            }

            StorageObjects.Owner<StorageQueue>(self).Dispose();
            StorageObjects.Free(self);

            return 0;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageQueueMethods.Release");

            return default;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static void EnqueueRequest(nint self, byte* request)
    {
        try
        {
            if (request != null)
            {
                StorageObjects.Owner<StorageQueue>(self).EnqueueRequest(request);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageQueueMethods.EnqueueRequest");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static void EnqueueStatus(nint self, nint statusArray, uint index)
    {
        try
        {
            StorageObjects.Owner<StorageQueue>(self).EnqueueStatus(statusArray, index);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageQueueMethods.EnqueueStatus");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static void EnqueueSignal(nint self, nint fence, ulong value)
    {
        try
        {
            StorageObjects.Owner<StorageQueue>(self).EnqueueSignal(fence, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageQueueMethods.EnqueueSignal");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static void Submit(nint self)
    {
        try
        {
            StorageObjects.Owner<StorageQueue>(self).Submit();
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageQueueMethods.Submit");
        }
    }

    /// <summary>Close: stops the worker; the object stays valid until its last Release.</summary>
    [UnmanagedCallersOnly]
    private static void Close(nint self)
    {
        try
        {
            StorageObjects.Owner<StorageQueue>(self).Dispose();
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageQueueMethods.Close");
        }
    }
}
