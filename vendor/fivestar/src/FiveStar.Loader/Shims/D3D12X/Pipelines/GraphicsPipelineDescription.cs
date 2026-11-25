// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// A graphics pipeline as the game described it, in desktop terms: the shaders as the game handed them over and the
/// fixed-function descs as d3d12.h bytes. Fields the game leaves out keep the D3D12 stream defaults.
/// </summary>
/// <param name="GameRootSignature">The game's root signature (kept for binding translation; the pipeline uses ours).</param>
/// <param name="Shaders">The shaders of every stage, in the order given.</param>
/// <param name="Blend">D3D12_BLEND_DESC.</param>
/// <param name="SampleMask">The sample mask.</param>
/// <param name="Rasterizer">D3D12_RASTERIZER_DESC.</param>
/// <param name="DepthStencil">D3D12_DEPTH_STENCIL_DESC1.</param>
/// <param name="InputLayout">The vertex input elements as the game named them.</param>
/// <param name="StripCut">D3D12_INDEX_BUFFER_STRIP_CUT_VALUE.</param>
/// <param name="TopologyType">D3D12_PRIMITIVE_TOPOLOGY_TYPE.</param>
/// <param name="RenderTargetFormats">DXGI formats of the bound targets.</param>
/// <param name="DepthStencilFormat">DXGI format of the depth target (0 for none).</param>
/// <param name="SampleCount">DXGI_SAMPLE_DESC.Count.</param>
/// <param name="SampleQuality">DXGI_SAMPLE_DESC.Quality.</param>
internal sealed record GraphicsPipelineDescription(
    nint GameRootSignature,
    IReadOnlyList<ShaderSource> Shaders,
    byte[] Blend,
    uint SampleMask,
    byte[] Rasterizer,
    byte[] DepthStencil,
    IReadOnlyList<VertexElement> InputLayout,
    int StripCut,
    int TopologyType,
    IReadOnlyList<int> RenderTargetFormats,
    int DepthStencilFormat,
    int SampleCount,
    int SampleQuality)
{
    /// <summary>The D3D12 defaults of a stream that names no subobject but the root signature.</summary>
    public static GraphicsPipelineDescription Defaults(nint gameRootSignature)
    {
        return new GraphicsPipelineDescription(
            gameRootSignature,
            [],
            DesktopStateBytes.DefaultBlend(),
            uint.MaxValue,
            DesktopStateBytes.DefaultRasterizer(),
            DesktopStateBytes.DefaultDepthStencil(),
            [],
            0,
            PipelineLayout.TriangleTopology,
            [],
            0,
            1,
            0);
    }
}
