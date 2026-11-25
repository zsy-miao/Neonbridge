// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Graphics.Pipelines;

/// <summary>
/// The shaders and output formats of a VS (+ GS) (+ PS) pipeline. The input layout reads vertex buffer slot 0;
/// <see cref="DepthFormat"/> is <c>Format.Unknown</c> for no depth target.
/// </summary>
public sealed record GraphicsProgram(
    byte[] VertexShader,
    byte[]? GeometryShader,
    byte[]? PixelShader,
    IReadOnlyList<InputElementDescription> InputLayout,
    IReadOnlyList<Format> Targets,
    Format DepthFormat,
    PrimitiveTopologyType Topology);
