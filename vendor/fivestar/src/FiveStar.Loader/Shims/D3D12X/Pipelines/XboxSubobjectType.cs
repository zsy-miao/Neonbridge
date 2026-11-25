// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// D3D12_PIPELINE_STATE_SUBOBJECT_TYPE as the game's stream uses it: desktop values up to 22, then the amplification
/// and mesh shaders one lower than on the desktop (24 and 25).
/// </summary>
internal enum XboxSubobjectType
{
    RootSignature = 0,
    VertexShader = 1,
    PixelShader = 2,
    DomainShader = 3,
    HullShader = 4,
    GeometryShader = 5,
    ComputeShader = 6,
    StreamOutput = 7,
    Blend = 8,
    SampleMask = 9,
    Rasterizer = 10,
    DepthStencil = 11,
    InputLayout = 12,
    StripCutValue = 13,
    PrimitiveTopology = 14,
    RenderTargetFormats = 15,
    DepthStencilFormat = 16,
    SampleDescription = 17,
    NodeMask = 18,
    CachedPipelineState = 19,
    Flags = 20,
    DepthStencil1 = 21,
    ViewInstancing = 22,

    /// <summary>Written in the hull-shader slot when the vertex slot holds a mesh shader.</summary>
    AmplificationShader = 23,

    /// <summary>Written in the vertex-shader slot when the shader is a mesh shader.</summary>
    MeshShader = 24,
}
