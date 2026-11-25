// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Xg;

/// <summary>Fixed sizes of the XG layouts.</summary>
internal static class XgLimits
{
    /// <summary>D3D12_REQ_MIP_LEVELS; also the mip entries per plane of XG_RESOURCE_LAYOUT.</summary>
    public const int MaxMips = 15;

    /// <summary>Planes this shim produces: colour or depth, and stencil.</summary>
    public const int MaxPlanes = 2;

    /// <summary>Plane entries of XG_RESOURCE_LAYOUT (0x18 + 4 x 0x6c8 + 8 = 0x1b40 bytes, the game's stack buffer).</summary>
    public const int LayoutPlanes = 4;
}
