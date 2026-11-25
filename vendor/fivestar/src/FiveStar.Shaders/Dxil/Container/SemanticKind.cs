// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>DXIL semantic kinds (DxilConstants.h SemanticKind).</summary>
public enum SemanticKind : byte
{
    Arbitrary = 0,
    VertexId = 1,
    InstanceId = 2,
    Position = 3,
    RenderTargetArrayIndex = 4,
    ViewportArrayIndex = 5,
    ClipDistance = 6,
    CullDistance = 7,
    PrimitiveId = 10,
    GsInstanceId = 11,
    SampleIndex = 12,
    IsFrontFace = 13,
    Coverage = 14,
    InnerCoverage = 15,
    Target = 16,
    Depth = 17,
    DepthLessEqual = 18,
    DepthGreaterEqual = 19,
    StencilRef = 20,
    DispatchThreadId = 21,
    GroupId = 22,
    GroupIndex = 23,
    GroupThreadId = 24,
    ViewId = 27,
    Barycentrics = 28,
    ShadingRate = 29,
    CullPrimitive = 30,
}
