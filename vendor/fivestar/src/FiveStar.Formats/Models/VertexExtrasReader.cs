// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Models;

/// <summary>
/// Reads the tangent (slot 11), texture coordinates 1-3 (slots 4, 14, 15) and skin streams of a vertex buffer. Skin:
/// slot 8 joints and slot 6 weights (4 bytes summing to 255); peds add a second four in slots 9 and 7. A slot 8 of
/// format 0 is one joint per vertex, a u8 or, on skeletons of more than 256 bones, a u16; streams are 4-byte aligned,
/// so the stream's size tells the two apart.
/// </summary>
public static class VertexExtrasReader
{
    /// <summary>The weight of a vertex's only joint, and the sum of a vertex's weights.</summary>
    public const byte FullWeight = 255;

    private const int SetSize = 4;
    private const int Single = 1;
    private const int Wide = 2;

    // Below 4 vertices a u8 stream padded to 4 bytes is as long as a u16 one; such streams are read as u8.
    private const int UnambiguousCount = 4;

    public static VertexExtras Read(ReadOnlySpan<byte> vertices, IReadOnlyList<VertexChannel> streams, int count)
    {
        float[]? tangents = null;
        var tangent = GeometryReader.Find(streams, VertexSlot.Tangent);

        if (tangent.HasValue && tangent.Value.Format == VertexFormat.Tangent)
        {
            tangents = VertexAttributes.Tangents(GeometryReader.Slice(vertices, tangent.Value, count), count);
        }

        return new VertexExtras(
            tangents,
            GeometryReader.TexCoords(vertices, streams, VertexSlot.TexCoord1, count),
            GeometryReader.TexCoords(vertices, streams, VertexSlot.TexCoord2, count),
            GeometryReader.TexCoords(vertices, streams, VertexSlot.TexCoord3, count),
            Skin(vertices, streams, count));
    }

    private static VertexSkin? Skin(ReadOnlySpan<byte> vertices, IReadOnlyList<VertexChannel> streams, int count)
    {
        var joints = GeometryReader.Find(streams, VertexSlot.Joints);

        if (!joints.HasValue)
        {
            return null;
        }

        if (joints.Value.Format == VertexFormat.SingleJoint)
        {
            return SingleJoint(vertices, streams, joints.Value, count);
        }

        var weights = GeometryReader.Find(streams, VertexSlot.Weights);

        if (!IsJointSet(joints) || !weights.HasValue || weights.Value.Format != VertexFormat.Bytes4)
        {
            return null;
        }

        var second = GeometryReader.Find(streams, VertexSlot.SecondJoints);
        var secondWeights = GeometryReader.Find(streams, VertexSlot.SecondWeights);

        if (!IsJointSet(second) || !secondWeights.HasValue || secondWeights.Value.Format != VertexFormat.Bytes4)
        {
            var skin = new VertexSkin(SetSize, new ushort[count * SetSize], new byte[count * SetSize]);
            CopySet(vertices, joints.Value, weights.Value, skin, 0, count);

            return skin;
        }

        var both = new VertexSkin(Wide * SetSize, new ushort[count * Wide * SetSize], new byte[count * Wide * SetSize]);
        CopySet(vertices, joints.Value, weights.Value, both, 0, count);
        CopySet(vertices, second.GetValueOrDefault(), secondWeights.Value, both, SetSize, count);

        return both;
    }

    private static VertexSkin SingleJoint(ReadOnlySpan<byte> vertices, IReadOnlyList<VertexChannel> streams, VertexChannel joints, int count)
    {
        var width = Single;

        if (count >= UnambiguousCount && StreamBytes(vertices, streams, joints) >= Wide * count)
        {
            width = Wide;
        }

        var data = GeometryReader.Slice(vertices, joints, width, count);
        var skin = new VertexSkin(Single, new ushort[count], new byte[count]);
        Array.Fill(skin.Weights, FullWeight);

        for (var vertex = 0; vertex < count; vertex++)
        {
            if (width == Wide)
            {
                skin.Joints[vertex] = BinaryPrimitives.ReadUInt16LittleEndian(data[(vertex * sizeof(ushort))..]);
            }
            else
            {
                skin.Joints[vertex] = data[vertex];
            }
        }

        return skin;
    }

    private static void CopySet(ReadOnlySpan<byte> vertices, VertexChannel joints, VertexChannel weights, VertexSkin skin, int first, int count)
    {
        var jointData = GeometryReader.Slice(vertices, joints, count);
        var weightData = GeometryReader.Slice(vertices, weights, count);

        for (var vertex = 0; vertex < count; vertex++)
        {
            for (var index = 0; index < SetSize; index++)
            {
                var source = (vertex * SetSize) + index;
                var target = (vertex * skin.Influences) + first + index;
                skin.Weights[target] = weightData[source];

                if (joints.Format == VertexFormat.Joints4x16)
                {
                    skin.Joints[target] = BinaryPrimitives.ReadUInt16LittleEndian(jointData[(source * sizeof(ushort))..]);
                }
                else
                {
                    skin.Joints[target] = jointData[source];
                }
            }
        }
    }

    private static bool IsJointSet(VertexChannel? stream)
    {
        return stream.HasValue && (stream.Value.Format == VertexFormat.Joints4x8 || stream.Value.Format == VertexFormat.Joints4x16);
    }

    /// <summary>The bytes from the stream to the next stream or the end of the buffer.</summary>
    private static int StreamBytes(ReadOnlySpan<byte> vertices, IReadOnlyList<VertexChannel> streams, VertexChannel stream)
    {
        var end = vertices.Length;

        foreach (var other in streams)
        {
            if (other.Offset > stream.Offset && other.Offset < end)
            {
                end = other.Offset;
            }
        }

        return end - stream.Offset;
    }
}
