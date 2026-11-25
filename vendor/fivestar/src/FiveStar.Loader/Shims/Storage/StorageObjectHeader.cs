// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The start of every native DirectStorage object (our own layout; the game sees only the vtable pointer): the vtable,
/// a GCHandle of the managed owner (0 for status arrays), the reference count and, for a status array, its capacity.
/// A status array's <see cref="StatusEntry"/> values follow the header.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct StorageObjectHeader
{
    public nint Vtable;
    public nint Owner;
    public int References;
    public int Count;
}
