// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The kind word of a derive-slot override entry, as the game writes it (0x142479f40): the depth-stencil entry is
/// written with 0, the rasterizer entry with 1.
/// </summary>
internal enum OverrideKind
{
    /// <summary>A D3D12_DEPTH_STENCIL_DESC.</summary>
    DepthStencil = 0,

    /// <summary>A D3D12_RASTERIZER_DESC.</summary>
    Rasterizer = 1,

    /// <summary>A D3D12_BLEND_DESC.</summary>
    Blend = 2,

    /// <summary>
    /// Written after a depth-stencil override whose state asks for it: 16 zero bytes, the stencil read and write
    /// masks, then zero. Meaning unknown.
    /// </summary>
    StencilMasks = 5,
}
