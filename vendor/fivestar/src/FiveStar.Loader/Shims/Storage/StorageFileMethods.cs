// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>The implemented IDStorageFileX slots. Queued requests hold a reference, so a file outlives its reads.</summary>
internal static unsafe class StorageFileMethods
{
    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, uint> addRef = &AddRef;
        delegate* unmanaged<nint, uint> release = &Release;
        delegate* unmanaged<nint, int> close = &Close;
        delegate* unmanaged<nint, void*, int> information = &GetFileInformation;

        vtable[DStorageLayout.AddRefSlot] = (nint)addRef;
        vtable[DStorageLayout.ReleaseSlot] = (nint)release;
        vtable[DStorageLayout.FileClose] = (nint)close;
        vtable[DStorageLayout.FileGetInformation] = (nint)information;
    }

    /// <summary>Gives up one reference; the last closes the file and frees the object.</summary>
    [HotPath]
    public static uint ReleaseFile(nint self)
    {
        if (!StorageObjects.Drop(self, out uint left))
        {
            return left;
        }

        StorageObjects.Owner<StorageFile>(self).Dispose();
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
            UnmanagedGuard.Caught(exception, "StorageFileMethods.AddRef");

            return default;
        }
    }

    [UnmanagedCallersOnly]
    private static uint Release(nint self)
    {
        try
        {
            return ReleaseFile(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageFileMethods.Release");

            return default;
        }
    }

    [UnmanagedCallersOnly]
    private static int Close(nint self)
    {
        try
        {
            StorageObjects.Owner<StorageFile>(self).Dispose();

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageFileMethods.Close");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int GetFileInformation(nint self, void* information)
    {
        try
        {
            return StorageObjects.Owner<StorageFile>(self).GetInformation(information);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StorageFileMethods.GetFileInformation");

            return HResult.Fail;
        }
    }
}
