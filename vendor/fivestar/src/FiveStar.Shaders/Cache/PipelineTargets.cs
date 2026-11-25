// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// The output part of a graphics pipeline's <c>D3D12_GRAPHICS_PIPELINE_STATE_DESC</c>, stored ahead of the vertex
/// input layout in the record's 0x0C metadata entry.
/// </summary>
/// <param name="RenderTargetFormats">RTVFormats, NumRenderTargets of them (DXGI_FORMAT values).</param>
/// <param name="DepthStencilFormat">DSVFormat (DXGI_FORMAT; 0 when no depth target is bound).</param>
/// <param name="TopologyType">PrimitiveTopologyType (D3D12_PRIMITIVE_TOPOLOGY_TYPE).</param>
/// <param name="SampleCount">SampleDesc.Count.</param>
/// <param name="SampleMask">SampleMask.</param>
public sealed record PipelineTargets(
    IReadOnlyList<int> RenderTargetFormats, int DepthStencilFormat, int TopologyType, int SampleCount, uint SampleMask);
