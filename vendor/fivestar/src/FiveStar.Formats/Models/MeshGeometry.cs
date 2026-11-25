// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// One decoded geometry: positions and normals as x, y, z triples (z up), texture coordinates as u, v pairs (v = 0 at
/// the top of the texture) and a triangle list, counter-clockwise when seen from the side the normals face.
/// </summary>
/// <param name="Lod">The level of detail, 0 the most detailed.</param>
/// <param name="Offset">Virtual offset of the geometry object.</param>
/// <param name="Shader">Index of its shader in the drawable's shader group.</param>
/// <param name="VertexCount">Vertices in the vertex buffer.</param>
/// <param name="MeshletCount">Meshlets in the meshlet buffer.</param>
/// <param name="Positions">x, y, z per vertex.</param>
/// <param name="Normals">Null when the normal stream is missing or of another format.</param>
/// <param name="TexCoords">Null when slot 3 is missing or of another format.</param>
/// <param name="Indices">Three vertex indices per triangle.</param>
/// <param name="Streams">The vertex buffer's stream table.</param>
/// <param name="Extras">Tangents, further texture coordinates and skin influences.</param>
public sealed record MeshGeometry(
    int Lod,
    int Offset,
    int Shader,
    int VertexCount,
    int MeshletCount,
    float[] Positions,
    float[]? Normals,
    float[]? TexCoords,
    int[] Indices,
    IReadOnlyList<VertexChannel> Streams,
    VertexExtras Extras)
{
    private const int Corners = 3;

    public int TriangleCount => Indices.Length / Corners;
}
