// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Numerics;

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// Reads the triangles of one BLAS geometry: indices (16-bit, 32-bit or none), positions in the float and normalized
/// formats DXR accepts, then the optional 3x4 transform. Formats outside <see cref="IsSupported"/> give no triangles.
/// </summary>
public static class TriangleDecoder
{
    // DXGI_FORMAT values (dxgiformat.h).
    private const uint R32G32B32Float = 6;
    private const uint R16G16B16A16Float = 10;
    private const uint R16G16B16A16Snorm = 13;
    private const uint R32G32Float = 16;
    private const uint R16G16Float = 34;
    private const uint R16G16Snorm = 37;
    private const uint R32Uint = 42;
    private const uint R16Uint = 57;
    private const uint NoIndices = 0;

    private const int TriangleVertices = 3;
    private const uint SecondCorner = 1;
    private const uint ThirdCorner = 2;
    private const int AxisX = 0;
    private const int AxisY = 1;
    private const int AxisZ = 2;
    private const int Component16 = 2;
    private const int Component32 = 4;
    private const int Float3Size = 12;
    private const int Float2Size = 8;
    private const int Short4Size = 8;
    private const int Short2Size = 4;
    private const float SnormScale = 32767f;
    private const float SnormMinimum = -1f;

    // Row-major 3x4: row r holds m[4r .. 4r + 3], the last column is the translation.
    private const int TransformFloats = 12;
    private const int Row1 = 4;
    private const int Row2 = 8;
    private const int Column1 = 1;
    private const int Column2 = 2;
    private const int Column3 = 3;

    public static bool IsSupported(uint vertexFormat, uint indexFormat)
    {
        bool index = indexFormat == NoIndices || indexFormat == R16Uint || indexFormat == R32Uint;

        return index && vertexFormat is R32G32B32Float or R16G16B16A16Float or R16G16B16A16Snorm
            or R32G32Float or R16G16Float or R16G16Snorm;
    }

    /// <summary>Appends the geometry's triangles; false (and nothing appended) for an unsupported format or a short buffer.</summary>
    public static bool Append(List<BvhTriangle> triangles, TriangleBuffers geometry)
    {
        if (!IsSupported(geometry.VertexFormat, geometry.IndexFormat))
        {
            return false;
        }

        bool indexed = geometry.IndexFormat != NoIndices;
        uint corners = geometry.VertexCount;

        if (indexed)
        {
            corners = geometry.IndexCount;
        }

        uint count = corners / TriangleVertices;
        int start = triangles.Count;

        for (uint primitive = 0; primitive < count; primitive++)
        {
            uint first = primitive * TriangleVertices;

            if (!TryVertex(geometry, first, out var v0)
                || !TryVertex(geometry, first + SecondCorner, out var v1)
                || !TryVertex(geometry, first + ThirdCorner, out var v2))
            {
                triangles.RemoveRange(start, triangles.Count - start);

                return false;
            }

            triangles.Add(new BvhTriangle(v0, v1, v2, geometry.GeometryIndex, primitive, geometry.Flags));
        }

        return true;
    }

    private static bool TryVertex(TriangleBuffers geometry, uint corner, out Vector3 position)
    {
        position = default;

        if (!TryIndex(geometry, corner, out uint vertex) || vertex >= geometry.VertexCount)
        {
            return false;
        }

        long offset = (long)vertex * geometry.VertexStride;

        if (offset + FormatSize(geometry.VertexFormat) > geometry.Vertices.Length)
        {
            return false;
        }

        position = Transform(Decode(geometry.Vertices[(int)offset..], geometry.VertexFormat), geometry.Transform);

        return true;
    }

    private static bool TryIndex(TriangleBuffers geometry, uint corner, out uint vertex)
    {
        vertex = corner;

        if (geometry.IndexFormat == NoIndices)
        {
            return true;
        }

        int size = geometry.IndexFormat == R16Uint ? Component16 : Component32;
        long offset = (long)corner * size;

        if (offset + size > geometry.Indices.Length)
        {
            return false;
        }

        var at = geometry.Indices[(int)offset..];

        if (size == Component16)
        {
            vertex = BinaryPrimitives.ReadUInt16LittleEndian(at);
        }
        else
        {
            vertex = BinaryPrimitives.ReadUInt32LittleEndian(at);
        }

        return true;
    }

    private static int FormatSize(uint format)
    {
        return format switch
        {
            R32G32B32Float => Float3Size,
            R32G32Float => Float2Size,
            R16G16Float or R16G16Snorm => Short2Size,
            _ => Short4Size,
        };
    }

    private static Vector3 Decode(ReadOnlySpan<byte> at, uint format)
    {
        return format switch
        {
            R32G32B32Float => new Vector3(Single(at, AxisX), Single(at, AxisY), Single(at, AxisZ)),
            R32G32Float => new Vector3(Single(at, AxisX), Single(at, AxisY), 0f),
            R16G16B16A16Float => new Vector3(Half(at, AxisX), Half(at, AxisY), Half(at, AxisZ)),
            R16G16Float => new Vector3(Half(at, AxisX), Half(at, AxisY), 0f),
            R16G16B16A16Snorm => new Vector3(Snorm(at, AxisX), Snorm(at, AxisY), Snorm(at, AxisZ)),
            _ => new Vector3(Snorm(at, AxisX), Snorm(at, AxisY), 0f),
        };
    }

    private static float Single(ReadOnlySpan<byte> at, int component)
    {
        return BinaryPrimitives.ReadSingleLittleEndian(at[(component * Component32)..]);
    }

    private static float Half(ReadOnlySpan<byte> at, int component)
    {
        return (float)BinaryPrimitives.ReadHalfLittleEndian(at[(component * Component16)..]);
    }

    private static float Snorm(ReadOnlySpan<byte> at, int component)
    {
        return Math.Max(BinaryPrimitives.ReadInt16LittleEndian(at[(component * Component16)..]) / SnormScale, SnormMinimum);
    }

    private static Vector3 Transform(Vector3 v, ReadOnlySpan<float> m)
    {
        if (m.Length < TransformFloats)
        {
            return v;
        }

        return new Vector3(
            (m[0] * v.X) + (m[Column1] * v.Y) + (m[Column2] * v.Z) + m[Column3],
            (m[Row1] * v.X) + (m[Row1 + Column1] * v.Y) + (m[Row1 + Column2] * v.Z) + m[Row1 + Column3],
            (m[Row2] * v.X) + (m[Row2 + Column1] * v.Y) + (m[Row2 + Column2] * v.Z) + m[Row2 + Column3]);
    }
}
