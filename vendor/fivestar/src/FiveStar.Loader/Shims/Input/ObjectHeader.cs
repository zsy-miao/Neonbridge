// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The start of every native object the GameInput shim hands out (our own layout; the game sees only the vtable
/// pointer): the vtable, a GCHandle of the owning <see cref="GameInputService"/>, the reference count and an index
/// (the device index for devices and readings).
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ObjectHeader
{
    public nint Vtable;
    public nint Owner;
    public int References;
    public int Index;
}
