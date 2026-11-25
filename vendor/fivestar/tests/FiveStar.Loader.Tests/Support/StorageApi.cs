// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.Storage;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Calls the DirectStorage objects through their vtables with the argument shapes the executable uses: the factory
/// from the xcrdapi.dll!DStorageGetFactory entry point, requests built at the console offsets.
/// </summary>
internal sealed unsafe class StorageApi
{
    private static readonly TimeSpan CompletionTimeout = TimeSpan.FromSeconds(30);

    private StorageApi(nint factory)
    {
        Factory = factory;
    }

    public nint Factory { get; }

    public static StorageApi Create(TestRuntime test)
    {
        var getFactory = (delegate* unmanaged<Guid*, nint*, int>)test.Shim("xcrdapi.dll", "DStorageGetFactory");
        Guid iid = Guid.Empty;
        nint factory;

        Check(getFactory(&iid, &factory));
        var staging = (delegate* unmanaged<nint, uint, int>)Slot(factory, DStorageLayout.FactorySetStagingBufferSize);
        Check(staging(factory, DStorageLayout.DefaultStagingBytes));

        return new StorageApi(factory);
    }

    public nint OpenFile(string path)
    {
        Guid iid = Guid.Empty;
        nint file;

        fixed (char* name = path)
        {
            var open = (delegate* unmanaged<nint, char*, Guid*, nint*, int>)Slot(Factory, DStorageLayout.FactoryOpenFile);
            Check(open(Factory, name, &iid, &file));
        }

        return file;
    }

    public nint CreateStatusArray(uint capacity)
    {
        Guid iid = Guid.Empty;
        nint array;
        var create = (delegate* unmanaged<nint, uint, byte*, Guid*, nint*, int>)Slot(Factory, DStorageLayout.FactoryCreateStatusArray);
        Check(create(Factory, capacity, null, &iid, &array));

        return array;
    }

    /// <summary>A file-source queue, filled in like the game's DSTORAGE_QUEUE_DESC.</summary>
    public nint CreateQueue()
    {
        byte* desc = stackalloc byte[0x20];
        new Span<byte>(desc, 0x20).Clear();
        *(ushort*)(desc + 0x08) = 0x7F;
        nint name = Marshal.StringToHGlobalAnsi("test");
        *(nint*)(desc + 0x10) = name;

        Guid iid = Guid.Empty;
        nint queue;
        var create = (delegate* unmanaged<nint, byte*, Guid*, nint*, int>)Slot(Factory, DStorageLayout.FactoryCreateQueue);
        Check(create(Factory, desc, &iid, &queue));
        Marshal.FreeHGlobal(name);

        return queue;
    }

    /// <summary>EnqueueRequest with a request laid out as the game builds it.</summary>
    public static void EnqueueRead(nint queue, nint file, ulong offset, uint size, byte* destination, uint destinationSize, byte compression)
    {
        byte* request = stackalloc byte[DStorageLayout.RequestSize];
        new Span<byte>(request, DStorageLayout.RequestSize).Clear();

        // Options, destination and its size, then the source file, offset and size.
        *(ulong*)request = compression;
        *(nint*)(request + 0x08) = (nint)destination;
        *(uint*)(request + 0x18) = destinationSize;

        *(nint*)(request + 0x20) = file;
        *(ulong*)(request + 0x28) = offset;
        *(uint*)(request + 0x30) = size;

        ((delegate* unmanaged<nint, byte*, void>)Slot(queue, DStorageLayout.QueueEnqueueRequest))(queue, request);
    }

    public static void EnqueueStatus(nint queue, nint array, uint index)
    {
        ((delegate* unmanaged<nint, nint, uint, void>)Slot(queue, DStorageLayout.QueueEnqueueStatus))(queue, array, index);
    }

    public static void Submit(nint queue)
    {
        ((delegate* unmanaged<nint, void>)Slot(queue, DStorageLayout.QueueSubmit))(queue);
    }

    /// <summary>Polls IsComplete like the game, then returns the entry's HRESULT.</summary>
    public static int WaitStatus(nint array, uint index)
    {
        var isComplete = (delegate* unmanaged<nint, uint, byte>)Slot(array, DStorageLayout.StatusIsComplete);
        var clock = Stopwatch.StartNew();
        var spin = default(SpinWait);

        while (isComplete(array, index) == 0)
        {
            if (clock.Elapsed > CompletionTimeout)
            {
                throw new TimeoutException($"status entry {index} never completed");
            }

            spin.SpinOnce();
        }

        return ((delegate* unmanaged<nint, uint, int>)Slot(array, DStorageLayout.StatusGetHResult))(array, index);
    }

    public static void Release(nint unknown)
    {
        ((delegate* unmanaged<nint, uint>)Slot(unknown, DStorageLayout.ReleaseSlot))(unknown);
    }

    private static nint Slot(nint self, int slot)
    {
        return (*(nint**)self)[slot];
    }

    private static void Check(int hr)
    {
        if (hr < 0)
        {
            throw new InvalidOperationException($"HRESULT 0x{hr:x8}");
        }
    }
}
