// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The implemented IDStorageStatusArrayX slots. The game polls IsComplete(0), waiting 1 ms between polls, after
/// EnqueueStatus and Submit. An entry index outside the array reads as complete with E_INVALIDARG.
/// </summary>
internal static unsafe class StatusArrayMethods
{
    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, uint> addRef = &AddRef;
        delegate* unmanaged<nint, uint> release = &Release;
        delegate* unmanaged<nint, uint, byte> isComplete = &IsComplete;
        delegate* unmanaged<nint, uint, int> result = &GetHResult;

        vtable[DStorageLayout.AddRefSlot] = (nint)addRef;
        vtable[DStorageLayout.ReleaseSlot] = (nint)release;
        vtable[DStorageLayout.StatusIsComplete] = (nint)isComplete;
        vtable[DStorageLayout.StatusGetHResult] = (nint)result;
    }

    /// <summary>Gives up one reference; the last frees the array.</summary>
    [HotPath]
    public static uint ReleaseArray(nint self)
    {
        if (!StorageObjects.Drop(self, out uint left))
        {
            return left;
        }

        StorageObjects.Free(self);

        return 0;
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
            UnmanagedGuard.Caught(exception, "StatusArrayMethods.AddRef");

            return default;
        }
    }

    [UnmanagedCallersOnly]
    private static uint Release(nint self)
    {
        try
        {
            return ReleaseArray(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StatusArrayMethods.Release");

            return default;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static byte IsComplete(nint self, uint index)
    {
        try
        {
            if (index >= (uint)((StorageObjectHeader*)self)->Count)
            {
                return 1;
            }

            return (byte)Volatile.Read(ref StorageObjects.Entries(self)[index].Complete);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StatusArrayMethods.IsComplete");

            return default;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static int GetHResult(nint self, uint index)
    {
        try
        {
            if (index >= (uint)((StorageObjectHeader*)self)->Count)
            {
                return HResult.InvalidArgument;
            }

            ref var entry = ref StorageObjects.Entries(self)[index];

            if (Volatile.Read(ref entry.Complete) == 0)
            {
                return HResult.Pending;
            }

            return entry.Result;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StatusArrayMethods.GetHResult");

            return HResult.Fail;
        }
    }
}
