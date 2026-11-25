// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// A <c>D3D12_DEPTH_STENCIL_DESC</c> from DB_DEPTH_CONTROL, DB_STENCIL_CONTROL and DB_STENCILREFMASK. The depth
/// format is not in the state; <see cref="UsesDepthTarget"/> says whether the pipeline needs one bound.
/// </summary>
public sealed record DepthStencilState(
    bool DepthEnable,
    bool DepthWrite,
    int DepthFunc,
    bool DepthBoundsTest,
    bool StencilEnable,
    int StencilReadMask,
    int StencilWriteMask,
    StencilFace Front,
    StencilFace Back)
{
    /// <summary>Gets whether depth or stencil is tested.</summary>
    public bool UsesDepthTarget => DepthEnable || StencilEnable || DepthBoundsTest;
}
