// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>D3D12_SUBRESOURCE_TILING (12 bytes).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgSubresourceTiling
{
    public uint WidthInTiles;
    public ushort HeightInTiles;
    public ushort DepthInTiles;
    public uint StartTileIndexInOverallResource;
}
