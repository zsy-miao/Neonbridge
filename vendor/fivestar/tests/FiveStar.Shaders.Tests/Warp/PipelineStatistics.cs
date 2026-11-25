// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>D3D12_QUERY_DATA_PIPELINE_STATISTICS of one run.</summary>
public sealed record PipelineStatistics(
    ulong InputVertices,
    ulong InputPrimitives,
    ulong VertexInvocations,
    ulong GeometryInvocations,
    ulong GeometryPrimitives,
    ulong ClipperInvocations,
    ulong ClipperPrimitives,
    ulong PixelInvocations,
    ulong ComputeInvocations);
