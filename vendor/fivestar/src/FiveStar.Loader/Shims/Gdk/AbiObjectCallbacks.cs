// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// IUnknown of the objects QueryApiImpl hands out. The objects live for the process, so AddRef and Release are
/// no-ops, and QueryInterface returns the object itself.
/// </summary>
internal static unsafe class AbiObjectCallbacks
{
    /// <summary>The reference count AddRef and Release report.</summary>
    private const uint StaticReferenceCount = 1;

    [UnmanagedCallersOnly]
    public static int QueryInterface(nint self, Guid* iid, nint* result)
    {
        if (result != null)
        {
            *result = self;
        }

        return HResult.Ok;
    }

    [UnmanagedCallersOnly]
    public static uint AddRef(nint self)
    {
        return StaticReferenceCount;
    }
}
