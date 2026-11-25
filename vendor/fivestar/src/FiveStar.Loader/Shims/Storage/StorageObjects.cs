// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// Creation, reference counting and the way back to the managed owner of the native DirectStorage objects. An object
/// starts with one reference; the last Release frees it.
/// </summary>
internal static unsafe class StorageObjects
{
    /// <summary>A native object with <paramref name="vtable"/>, owned by <paramref name="owner"/>, with <paramref name="extra"/> bytes after the header.</summary>
    public static nint Create(nint vtable, object? owner, int extra)
    {
        var header = (StorageObjectHeader*)NativeMemory.AllocZeroed((nuint)(sizeof(StorageObjectHeader) + extra));
        header->Vtable = vtable;
        header->References = 1;

        if (owner is not null)
        {
            header->Owner = GCHandle.ToIntPtr(GCHandle.Alloc(owner));
        }

        return (nint)header;
    }

    /// <summary>The managed object behind a native one.</summary>
    [HotPath]
    public static T Owner<T>(nint self)
        where T : class
    {
        return (T)GCHandle.FromIntPtr(((StorageObjectHeader*)self)->Owner).Target!;
    }

    [HotPath]
    public static uint AddRef(nint self)
    {
        return (uint)Interlocked.Increment(ref ((StorageObjectHeader*)self)->References);
    }

    /// <summary>
    /// Gives up one reference; true when it was the last, so the caller frees the object. A count that would go below 0
    /// stays at 0 and frees nothing.
    /// </summary>
    [HotPath]
    public static bool Drop(nint self, out uint left)
    {
        ref int references = ref ((StorageObjectHeader*)self)->References;
        int count = Interlocked.Decrement(ref references);

        if (count < 0)
        {
            Interlocked.Increment(ref references);
            left = 0;

            return false;
        }

        left = (uint)count;

        return count == 0;
    }

    /// <summary>Frees the native object and its owner handle; called once the count reaches 0.</summary>
    public static void Free(nint self)
    {
        var header = (StorageObjectHeader*)self;

        if (header->Owner != 0)
        {
            GCHandle.FromIntPtr(header->Owner).Free();
        }

        NativeMemory.Free(header);
    }

    /// <summary>The status entries after a status array's header.</summary>
    [HotPath]
    public static StatusEntry* Entries(nint statusArray)
    {
        return (StatusEntry*)(statusArray + sizeof(StorageObjectHeader));
    }
}
