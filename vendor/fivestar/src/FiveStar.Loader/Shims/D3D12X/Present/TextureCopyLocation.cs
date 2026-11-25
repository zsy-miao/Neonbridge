// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>D3D12_TEXTURE_COPY_LOCATION (d3d12.h): a subresource index, or a placed footprint (offset, then the footprint).</summary>
[StructLayout(LayoutKind.Explicit, Size = 48)]
internal struct TextureCopyLocation
{
    /// <summary>D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT.</summary>
    public const int FootprintType = 1;

    [FieldOffset(0)]
    public nint Resource;

    [FieldOffset(8)]
    public int Type;

    [FieldOffset(16)]
    public uint Subresource;

    [FieldOffset(16)]
    public ulong FootprintOffset;
}
