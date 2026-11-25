// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>D3D12_PACKED_MIP_INFO (12 bytes); the game reads both counts as bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XgPackedMipInfo
{
    public byte NumStandardMips;
    public byte NumPackedMips;
    public uint NumTilesForPackedMips;
    public uint StartTileIndexInOverallResource;
}
