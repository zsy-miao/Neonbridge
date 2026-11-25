// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>D3D12_TILE_SHAPE: a 64 KB tile's extent in pixels.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgTileShape
{
    public uint WidthInTexels;
    public uint HeightInTexels;
    public uint DepthInTexels;
}
