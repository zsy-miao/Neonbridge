// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Models;

/// <summary>Decoders of the 4- and 8-byte attribute streams (normals, tangents, texture coordinates).</summary>
public static class VertexAttributes
{
    /// <summary>One unit of a <see cref="VertexFormat.Fixed16x2"/> value (checked on posters: 0..1 across the texture).</summary>
    public const float FixedUnit = 1f / 8192;

    private const int NormalBits = 10;
    private const uint NormalMask = (1u << NormalBits) - 1;
    private const float NormalHalfRange = NormalMask / 2f;
    private const int Components = 3;
    private const int Pair = 2;
    private const int TangentComponents = 4;
    private const int TangentSignShift = 30;
    private const uint FlippedSign = 3;

    /// <summary>
    /// Octahedral normals: x = bits 0-9, y = bits 10-19, each unorm mapped to -1..1; z = 1 - |x| - |y|, folded when
    /// negative. Returns x, y, z triples of unit length.
    /// </summary>
    public static float[] OctahedralNormals(ReadOnlySpan<byte> stream, int count)
    {
        var normals = new float[count * Components];

        for (var vertex = 0; vertex < count; vertex++)
        {
            var word = BinaryPrimitives.ReadUInt32LittleEndian(stream[(vertex * sizeof(uint))..]);
            var x = ((word & NormalMask) / NormalHalfRange) - 1;
            var y = (((word >> NormalBits) & NormalMask) / NormalHalfRange) - 1;
            var z = 1 - MathF.Abs(x) - MathF.Abs(y);

            if (z < 0)
            {
                var foldedX = (1 - MathF.Abs(y)) * MathF.CopySign(1, x);
                y = (1 - MathF.Abs(x)) * MathF.CopySign(1, y);
                x = foldedX;
            }

            var length = MathF.Sqrt((x * x) + (y * y) + (z * z));
            normals[vertex * Components] = x / length;
            normals[(vertex * Components) + 1] = y / length;
            normals[(vertex * Components) + Pair] = z / length;
        }

        return normals;
    }

    /// <summary>
    /// Tangents: x = bits 0-9, y = bits 10-19, z = bits 20-29, each unorm mapped to -1..1, in model space. Bits 30-31
    /// are 0 or 3; 3 gives w = -1. The bitangent w * cross(normal, tangent) points the way v grows (v = 0 at the top).
    /// Returns x, y, z, w with x, y, z of unit length.
    /// </summary>
    public static float[] Tangents(ReadOnlySpan<byte> stream, int count)
    {
        var tangents = new float[count * TangentComponents];

        for (var vertex = 0; vertex < count; vertex++)
        {
            var word = BinaryPrimitives.ReadUInt32LittleEndian(stream[(vertex * sizeof(uint))..]);
            var x = ((word & NormalMask) / NormalHalfRange) - 1;
            var y = (((word >> NormalBits) & NormalMask) / NormalHalfRange) - 1;
            var z = (((word >> (NormalBits * Pair)) & NormalMask) / NormalHalfRange) - 1;
            var length = MathF.Sqrt((x * x) + (y * y) + (z * z));
            var start = vertex * TangentComponents;

            tangents[start] = x / length;
            tangents[start + 1] = y / length;
            tangents[start + Pair] = z / length;
            tangents[start + Components] = 1;

            if (word >> TangentSignShift == FlippedSign)
            {
                tangents[start + Components] = -1;
            }
        }

        return tangents;
    }

    /// <summary>A <see cref="VertexFormat.Float3"/> stream as x, y, z triples.</summary>
    public static float[] Floats(ReadOnlySpan<byte> stream, int count)
    {
        var values = new float[count * Components];

        for (var index = 0; index < values.Length; index++)
        {
            values[index] = BinaryPrimitives.ReadSingleLittleEndian(stream[(index * sizeof(float))..]);
        }

        return values;
    }

    /// <summary>Texture coordinates as u, v pairs (v = 0 at the top of the texture) from a 4- or 8-byte stream.</summary>
    public static float[] TexCoords(ReadOnlySpan<byte> stream, byte format, int count)
    {
        var coordinates = new float[count * Pair];

        for (var index = 0; index < coordinates.Length; index++)
        {
            if (format == VertexFormat.Eight)
            {
                coordinates[index] = BinaryPrimitives.ReadSingleLittleEndian(stream[(index * sizeof(float))..]);
            }
            else
            {
                coordinates[index] = BinaryPrimitives.ReadInt16LittleEndian(stream[(index * sizeof(short))..]) * FixedUnit;
            }
        }

        return coordinates;
    }
}
