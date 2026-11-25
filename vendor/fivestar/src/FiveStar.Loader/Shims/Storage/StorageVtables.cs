// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The four native vtables. Implemented slots point at the [UnmanagedCallersOnly] methods; QueryInterface answers
/// E_NOINTERFACE and every other slot is a stub named "xcrdapi.dll!Interface::slotN" that logs its first call and
/// returns E_NOTIMPL.
/// </summary>
internal sealed unsafe class StorageVtables : IDisposable
{
    public StorageVtables(UnresolvedStubs stubs, CodeHeap heap)
    {
        Factory = Build(stubs, DStorageLayout.FactoryName, DStorageLayout.FactorySlots, &DStorageFactoryMethods.Fill);
        Queue = Build(stubs, DStorageLayout.QueueName, DStorageLayout.QueueSlots, &StorageQueueMethods.Fill);
        File = Build(stubs, DStorageLayout.FileName, DStorageLayout.FileSlots, &StorageFileMethods.Fill);
        StatusArray = Build(stubs, DStorageLayout.StatusArrayName, DStorageLayout.StatusArraySlots, &StatusArrayMethods.Fill);

        // One read-write to read-execute flip for all stubs.
        heap.Seal();
    }

    public nint Factory { get; }

    public nint Queue { get; }

    public nint File { get; }

    public nint StatusArray { get; }

    public void Dispose()
    {
        NativeMemory.Free((void*)Factory);
        NativeMemory.Free((void*)Queue);
        NativeMemory.Free((void*)File);
        NativeMemory.Free((void*)StatusArray);
    }

    private static nint Build(UnresolvedStubs stubs, string name, int slots, delegate*<nint*, void> fill)
    {
        var vtable = (nint*)NativeMemory.AllocZeroed((nuint)(slots * sizeof(nint)));
        fill(vtable);
        vtable[DStorageLayout.QueryInterfaceSlot] = stubs.Create(DStorageShim.Module, $"{name}::QueryInterface", HResult.NoInterface, true, false);

        for (int slot = 0; slot < slots; slot++)
        {
            if (vtable[slot] == 0)
            {
                vtable[slot] = stubs.Create(DStorageShim.Module, $"{name}::slot{slot}", HResult.NotImplemented, true, false);
            }
        }

        return (nint)vtable;
    }
}
