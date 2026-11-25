// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Models;

/// <summary>
/// The meshlet buffer that replaces an index buffer:
/// <list type="table">
/// <item><term>+0x00</term><description>u32: 0x80000000 | flags | meshlet count (bits 0-15)</description></item>
/// <item><term>+0x04</term><description>u32 triangle count</description></item>
/// <item><term>+0x08</term><description>u32 meshlet vertex count (the length of the remap table)</description></item>
/// <item><term>+0x0C</term><description>16 bytes per meshlet; dword 1 = first triangle &lt;&lt; 16 | first remap entry</description></item>
/// <item><term>then</term><description>one u32 per triangle: three 7-bit meshlet-local indices (bits 0-20), the meshlet number in bits 21-31</description></item>
/// <item><term>then</term><description>u16 per meshlet vertex: the vertex it stands for</description></item>
/// </list>
/// A meshlet runs to the next meshlet's first triangle and first remap entry. Both starts are u16 and wrap past 65,535.
/// </summary>
public static class Meshlets
{
    public const uint Flag = 0x8000_0000;

    // Bits 16-30 are flags: vehicles set 29 and 30.
    private const uint CountMask = 0xFFFF;

    private const int HeaderBytes = 12;
    private const int DescriptorBytes = 16;
    private const int TriangleCountOffset = 4;
    private const int RemapCountOffset = 8;
    private const int StartWord = 4;
    private const int StartShift = 16;
    private const uint LowHalf = 0xFFFF;
    private const int IndexBits = 7;
    private const uint IndexMask = (1u << IndexBits) - 1;
    private const int MeshletShift = IndexBits * Corners;
    private const int Corners = 3;

    /// <summary>The number of meshlets the buffer declares.</summary>
    public static int Count(ReadOnlySpan<byte> data)
    {
        return (int)(BinaryPrimitives.ReadUInt32LittleEndian(data) & CountMask);
    }

    /// <summary>A triangle list over the vertex buffer; throws when an index leaves its meshlet or the buffer.</summary>
    public static int[] Triangles(ReadOnlySpan<byte> data, int vertexCount)
    {
        var head = BinaryPrimitives.ReadUInt32LittleEndian(data);

        if ((head & Flag) == 0)
        {
            throw new ModelFormatException($"meshlet header 0x{head:X8} lacks the meshlet flag");
        }

        long meshlets = Count(data);
        long triangles = BinaryPrimitives.ReadUInt32LittleEndian(data[TriangleCountOffset..]);
        long remapCount = BinaryPrimitives.ReadUInt32LittleEndian(data[RemapCountOffset..]);
        var triangleStart = HeaderBytes + (meshlets * DescriptorBytes);
        var remapStart = triangleStart + (triangles * sizeof(uint));

        if (remapStart + (remapCount * sizeof(ushort)) > data.Length)
        {
            throw new ModelFormatException($"meshlet buffer of 0x{data.Length:X} bytes is too short for {meshlets} meshlets and {triangles} triangles");
        }

        return ExpandAll(data, (int)meshlets, (int)triangles, (int)remapCount, vertexCount);
    }

    private static int[] ExpandAll(ReadOnlySpan<byte> data, int meshlets, int triangles, int remapCount, int vertexCount)
    {
        var triangleStart = HeaderBytes + (meshlets * DescriptorBytes);
        var remapStart = triangleStart + (triangles * sizeof(uint));
        var indices = new int[triangles * Corners];

        if (meshlets == 0)
        {
            return indices;
        }

        var (rawTriangle, rawVertex) = Start(data, 0);
        var (firstTriangle, firstVertex) = ((int)rawTriangle, (int)rawVertex);

        for (var meshlet = 0; meshlet < meshlets; meshlet++)
        {
            var (endTriangle, endVertex) = (triangles, remapCount);

            if (meshlet + 1 < meshlets)
            {
                var (nextTriangle, nextVertex) = Start(data, meshlet + 1);
                endTriangle = firstTriangle + Advance(rawTriangle, nextTriangle);
                endVertex = firstVertex + Advance(rawVertex, nextVertex);
                (rawTriangle, rawVertex) = (nextTriangle, nextVertex);
            }

            if (endTriangle > triangles || endVertex > remapCount)
            {
                throw new ModelFormatException($"meshlet {meshlet} spans triangles {firstTriangle}-{endTriangle} and vertices {firstVertex}-{endVertex}");
            }

            var range = new MeshletRange(meshlet, firstTriangle, endTriangle, firstVertex, endVertex);
            Expand(data[triangleStart..], data[remapStart..], range, vertexCount, indices);
            (firstTriangle, firstVertex) = (endTriangle, endVertex);
        }

        return indices;
    }

    /// <summary>
    /// The starts are u16 and wrap on geometries of more than 65,535 triangles or remap entries; a meshlet holds far
    /// fewer, so the step from one start to the next, modulo 2^16, is the meshlet's true size.
    /// </summary>
    private static int Advance(uint start, uint next)
    {
        return (int)((next - start) & LowHalf);
    }

    private static (uint Triangle, uint Vertex) Start(ReadOnlySpan<byte> data, int meshlet)
    {
        var word = BinaryPrimitives.ReadUInt32LittleEndian(data[(HeaderBytes + (meshlet * DescriptorBytes) + StartWord)..]);

        return (word >> StartShift, word & LowHalf);
    }

    private static void Expand(ReadOnlySpan<byte> triangles, ReadOnlySpan<byte> remap, MeshletRange range, int vertexCount, int[] indices)
    {
        var localCount = range.EndVertex - range.FirstVertex;

        for (var triangle = range.FirstTriangle; triangle < range.EndTriangle; triangle++)
        {
            var word = BinaryPrimitives.ReadUInt32LittleEndian(triangles[(triangle * sizeof(uint))..]);

            if (word >> MeshletShift != range.Meshlet)
            {
                throw new ModelFormatException($"triangle {triangle} names meshlet {word >> MeshletShift}, not {range.Meshlet}");
            }

            for (var corner = 0; corner < Corners; corner++)
            {
                var local = (int)((word >> (corner * IndexBits)) & IndexMask);

                if (local >= localCount)
                {
                    throw new ModelFormatException($"triangle {triangle} uses local vertex {local} of {localCount}");
                }

                var vertex = BinaryPrimitives.ReadUInt16LittleEndian(remap[((range.FirstVertex + local) * sizeof(ushort))..]);

                if (vertex >= vertexCount)
                {
                    throw new ModelFormatException($"meshlet {range.Meshlet} maps to vertex {vertex} of {vertexCount}");
                }

                indices[(triangle * Corners) + corner] = vertex;
            }
        }
    }

    private readonly record struct MeshletRange(int Meshlet, int FirstTriangle, int EndTriangle, int FirstVertex, int EndVertex);
}
