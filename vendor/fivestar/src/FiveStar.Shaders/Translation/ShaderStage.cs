// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>The HLSL stage a program is translated to.</summary>
public enum ShaderStage
{
    /// <summary>Pixel shader (hardware PS).</summary>
    Pixel,

    /// <summary>Vertex shader (hardware VS).</summary>
    Vertex,

    /// <summary>Compute shader (hardware CS).</summary>
    Compute,

    /// <summary>Mesh shader (hardware NGG primitive shader).</summary>
    Mesh,

    /// <summary>Geometry shader (hardware legacy GS with its VS copy shader).</summary>
    Geometry,
}
