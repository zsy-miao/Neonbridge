// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>The system value numbers of signature parts (D3D_NAME, d3dcommon.h).</summary>
public enum SystemValueName : uint
{
    Undefined = 0,
    Position = 1,
    ClipDistance = 2,
    CullDistance = 3,
    RenderTargetArrayIndex = 4,
    ViewportArrayIndex = 5,
    VertexId = 6,
    PrimitiveId = 7,
    InstanceId = 8,
    IsFrontFace = 9,
    SampleIndex = 10,
    Barycentrics = 23,
    ShadingRate = 24,
    CullPrimitive = 25,
    Target = 64,
    Depth = 65,
    Coverage = 66,
    DepthGreaterEqual = 67,
    DepthLessEqual = 68,
    StencilRef = 69,
    InnerCoverage = 70,
}
