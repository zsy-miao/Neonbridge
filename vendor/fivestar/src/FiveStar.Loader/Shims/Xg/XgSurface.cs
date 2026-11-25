// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>The computed layout of a resource: its planes back to back, each aligned to its block size.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgSurface
{
    public XgResourceDesc Desc;
    public int PlaneCount;
    public ulong Size;
    public ulong Alignment;
    public XgPlaneArray Planes;
}
