// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>The signatures a graphics stage has (DxilSigPoint: VSIn, VSOut, PSIn, PSOut, MSOut, MSPOut, GSVIn, GSIn, GSOut).</summary>
public enum SignaturePoint
{
    VertexInput,
    VertexOutput,
    PixelInput,
    PixelOutput,
    MeshVertexOutput,
    MeshPrimitiveOutput,
    GeometryVertexInput,

    /// <summary>A geometry shader's inputs that are not per vertex (the primitive id).</summary>
    GeometryInput,
    GeometryOutput,
}
