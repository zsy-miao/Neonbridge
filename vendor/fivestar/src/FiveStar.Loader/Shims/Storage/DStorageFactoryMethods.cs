// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>The implemented IDStorageFactoryX slots. Every other slot is a not-implemented stub.</summary>
internal static unsafe class DStorageFactoryMethods
{
    /// <summary>The factory lives as long as the process, so the reference count is not kept.</summary>
    private const uint LiveReferences = 1;

    public static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, uint> addRef = &AddRef;
        delegate* unmanaged<nint, uint> release = &Release;
        delegate* unmanaged<nint, char*, Guid*, nint*, int> openFile = &OpenFile;
        delegate* unmanaged<nint, uint, byte*, Guid*, nint*, int> createStatusArray = &CreateStatusArray;
        delegate* unmanaged<nint, uint, int> stagingBufferSize = &SetStagingBufferSize;
        delegate* unmanaged<nint, byte*, Guid*, nint*, int> createQueue = &CreateQueue;

        vtable[DStorageLayout.AddRefSlot] = (nint)addRef;
        vtable[DStorageLayout.ReleaseSlot] = (nint)release;
        vtable[DStorageLayout.FactoryOpenFile] = (nint)openFile;
        vtable[DStorageLayout.FactoryCreateStatusArray] = (nint)createStatusArray;
        vtable[DStorageLayout.FactorySetStagingBufferSize] = (nint)stagingBufferSize;
        vtable[DStorageLayout.FactoryCreateQueue] = (nint)createQueue;
    }

    [UnmanagedCallersOnly]
    private static uint AddRef(nint self)
    {
        return LiveReferences;
    }

    [UnmanagedCallersOnly]
    private static uint Release(nint self)
    {
        return LiveReferences;
    }

    [UnmanagedCallersOnly]
    private static int OpenFile(nint self, char* path, Guid* iid, nint* result)
    {
        try
        {
            return StorageObjects.Owner<DStorageFactory>(self).OpenFile(path, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DStorageFactoryMethods.OpenFile");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int CreateStatusArray(nint self, uint capacity, byte* name, Guid* iid, nint* result)
    {
        try
        {
            return StorageObjects.Owner<DStorageFactory>(self).CreateStatusArray(capacity, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DStorageFactoryMethods.CreateStatusArray");

            return HResult.Fail;
        }
    }

    /// <summary>Takes (this, uint32 bytes): the size of each queue's read staging, for queues created afterwards.</summary>
    [UnmanagedCallersOnly]
    private static int SetStagingBufferSize(nint self, uint bytes)
    {
        try
        {
            return StorageObjects.Owner<DStorageFactory>(self).SetStagingBufferSize(bytes);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DStorageFactoryMethods.SetStagingBufferSize");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int CreateQueue(nint self, byte* desc, Guid* iid, nint* result)
    {
        try
        {
            return StorageObjects.Owner<DStorageFactory>(self).CreateQueue(desc, result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DStorageFactoryMethods.CreateQueue");

            return HResult.Fail;
        }
    }
}
