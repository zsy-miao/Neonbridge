// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Native;

/// <summary>Raw COM objects: a pointer to a vtable whose first three slots are IUnknown.</summary>
internal static unsafe class ComObject
{
    public const int QueryInterfaceSlot = 0;
    public const int ReleaseSlot = 2;

    /// <summary>Method <paramref name="slot"/> of the object's vtable.</summary>
    public static nint Slot(nint unknown, int slot)
    {
        return (*(nint**)unknown)[slot];
    }

    /// <summary>IUnknown::Release, when the pointer is not null.</summary>
    public static void Release(nint unknown)
    {
        if (unknown != 0)
        {
            ((delegate* unmanaged<nint, uint>)Slot(unknown, ReleaseSlot))(unknown);
        }
    }
}
