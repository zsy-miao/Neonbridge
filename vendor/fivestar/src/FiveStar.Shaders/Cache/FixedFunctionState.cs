// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// The fixed-function part of a <c>D3D12_GRAPHICS_PIPELINE_STATE_DESC</c>, decoded from the state registers.
/// </summary>
/// <param name="Blend">Eight render-target blend descriptions.</param>
/// <param name="DualSource">Whether target 0 blends with the second colour output (SRC1 factors).</param>
/// <param name="RenderTargets">The render targets the pixel shader writes, with a guessed format each.</param>
/// <param name="DepthStencil">Depth and stencil state.</param>
/// <param name="Rasterizer">Rasterizer state.</param>
/// <param name="TopologyType">D3D12_PRIMITIVE_TOPOLOGY_TYPE (rasterised primitive; 0 when unknown).</param>
/// <param name="ForcedSampleCount">Rasterizer sample count from DB_EQAA when above 1 (UAV-only and conservative passes), else 0.</param>
public sealed record FixedFunctionState(
    IReadOnlyList<BlendTarget> Blend,
    bool DualSource,
    IReadOnlyList<RenderTargetSlot> RenderTargets,
    DepthStencilState DepthStencil,
    RasterizerState Rasterizer,
    int TopologyType,
    int ForcedSampleCount)
{
    /// <summary>Gets NumRenderTargets: one past the highest written slot (0 for depth-only pipelines).</summary>
    public int RenderTargetCount => RenderTargets.Select(target => target.Index + 1).DefaultIfEmpty(0).Max();

    /// <summary>SampleDesc.Count; no pipeline of the reference dump renders to multisampled targets.</summary>
    public const int SampleCount = 1;
}
