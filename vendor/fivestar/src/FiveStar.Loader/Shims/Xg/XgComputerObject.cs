// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// The native XGTextureAddressComputer the shim hands out (our own layout; the game sees only the vtable pointer):
/// the vtable, the reference count and the computed layout. It lives in native memory until its last Release.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgComputerObject
{
    public nint Vtable;
    public int References;
    public XgSurface Surface;
}
