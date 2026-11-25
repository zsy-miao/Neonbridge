// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>RECT / D3D12_RECT (windef.h): left, top, right, bottom.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct PlaneRect
{
    public int Left;
    public int Top;
    public int Right;
    public int Bottom;
}
