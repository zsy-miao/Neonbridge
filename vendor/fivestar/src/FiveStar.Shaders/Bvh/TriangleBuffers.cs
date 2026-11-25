// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// One triangle geometry of a BLAS build (D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC) over CPU-readable bytes.
/// </summary>
/// <param name="Vertices">The vertex buffer from its start address; <paramref name="VertexStride"/> bytes per vertex.</param>
/// <param name="VertexStride">Bytes from one vertex to the next.</param>
/// <param name="VertexFormat">DXGI format of the position.</param>
/// <param name="VertexCount">Number of vertices.</param>
/// <param name="Indices">The index buffer; empty when the geometry is not indexed.</param>
/// <param name="IndexFormat">DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R32_UINT or DXGI_FORMAT_UNKNOWN (no indices).</param>
/// <param name="IndexCount">Number of indices.</param>
/// <param name="Transform">A row-major 3x4 matrix applied to every vertex; empty for none.</param>
/// <param name="GeometryIndex">The index of the geometry desc, reported by hits.</param>
/// <param name="Flags">The low byte of the geometry flags.</param>
public readonly ref struct TriangleBuffers(
    ReadOnlySpan<byte> Vertices,
    int VertexStride,
    uint VertexFormat,
    uint VertexCount,
    ReadOnlySpan<byte> Indices,
    uint IndexFormat,
    uint IndexCount,
    ReadOnlySpan<float> Transform,
    uint GeometryIndex,
    byte Flags)
{
    public ReadOnlySpan<byte> Vertices { get; } = Vertices;

    public int VertexStride { get; } = VertexStride;

    public uint VertexFormat { get; } = VertexFormat;

    public uint VertexCount { get; } = VertexCount;

    public ReadOnlySpan<byte> Indices { get; } = Indices;

    public uint IndexFormat { get; } = IndexFormat;

    public uint IndexCount { get; } = IndexCount;

    public ReadOnlySpan<float> Transform { get; } = Transform;

    public uint GeometryIndex { get; } = GeometryIndex;

    public byte Flags { get; } = Flags;
}
