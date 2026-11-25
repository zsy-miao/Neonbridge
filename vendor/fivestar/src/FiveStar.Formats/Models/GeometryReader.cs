// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// Decodes one geometry object. Layout (virtual offsets from the object):
/// <list type="table">
/// <item><term>geometry +0x08</term><description>pointer to the vertex buffer object</description></item>
/// <item><term>geometry +0x40</term><description>pointer to the meshlet info: +0x00 pointer to the meshlet buffer object, +0x08 u16 meshlet count, +0x0A u16 triangle count</description></item>
/// <item><term>buffer object +0x08</term><description>u32 size in dwords, then u16 element size (4) and u16 kind (9 vertices, 8 meshlets)</description></item>
/// <item><term>buffer object +0x18</term><description>pointer into the physical segment</description></item>
/// <item><term>vertex buffer object +0x40</term><description>pointer to the declaration; its u32 at +0x138 holds the vertex count in bits 10-31</description></item>
/// </list>
/// </summary>
public static class GeometryReader
{
    public const ushort VertexKind = 9;
    public const ushort MeshletKind = 8;

    private const int VertexBufferPointer = 0x08;
    private const int MeshletInfoPointer = 0x40;
    private const int BufferDwords = 0x08;
    private const int BufferKind = 0x0E;
    private const int BufferFlags = 0x10;
    private const uint MeshletFlags = 0x1380;
    private const int BufferData = 0x18;
    private const int DeclarationPointer = 0x40;
    private const int DeclarationVertexCount = 0x138;
    private const int VertexCountShift = 10;
    private const int MeshletCountOffset = 0x08;

    public static MeshGeometry Read(ResourceView view, int offset, int lod, int shader)
    {
        var vertexObject = view.FollowRequired(offset + VertexBufferPointer, "vertex buffer");
        var info = view.FollowRequired(offset + MeshletInfoPointer, "meshlet info");
        var meshletObject = view.FollowRequired(info, "meshlet buffer");
        var vertices = Buffer(view, vertexObject, VertexKind);
        var meshlets = Buffer(view, meshletObject, MeshletKind);
        var declaration = view.FollowRequired(vertexObject + DeclarationPointer, "vertex declaration");

        if (view.U32(meshletObject + BufferFlags) != MeshletFlags)
        {
            throw new ModelFormatException($"index buffer flags 0x{view.U32(meshletObject + BufferFlags):X}, not those of a meshlet buffer");
        }

        var count = (int)(view.U32(declaration + DeclarationVertexCount) >> VertexCountShift);
        var streams = VertexChannelTable.Read(vertices);
        var positions = Positions(vertices, streams, count);
        var normals = Normals(vertices, streams, count);
        var texCoords = TexCoords(vertices, streams, VertexSlot.TexCoord0, count);
        var indices = Meshlets.Triangles(meshlets, count);

        if (Meshlets.Count(meshlets) != view.U16(info + MeshletCountOffset))
        {
            throw new ModelFormatException($"meshlet info says {view.U16(info + MeshletCountOffset)} meshlets, the buffer {Meshlets.Count(meshlets)}");
        }

        var extras = VertexExtrasReader.Read(vertices, streams, count);

        return new MeshGeometry(lod, offset, shader, count, Meshlets.Count(meshlets), positions, normals, texCoords, indices, streams, extras);
    }

    /// <summary>The texture coordinates of a slot as u, v pairs, or null when absent or of another format.</summary>
    internal static float[]? TexCoords(ReadOnlySpan<byte> vertices, IReadOnlyList<VertexChannel> streams, int slot, int count)
    {
        var stream = Find(streams, slot);

        if (!stream.HasValue || (stream.Value.Format != VertexFormat.Fixed16x2 && stream.Value.Format != VertexFormat.Eight))
        {
            return null;
        }

        return VertexAttributes.TexCoords(Slice(vertices, stream.Value, count), stream.Value.Format, count);
    }

    /// <summary>The stream's elements; throws when they run past the vertex buffer.</summary>
    internal static ReadOnlySpan<byte> Slice(ReadOnlySpan<byte> vertices, VertexChannel stream, int count)
    {
        return Slice(vertices, stream, stream.ElementSize, count);
    }

    internal static ReadOnlySpan<byte> Slice(ReadOnlySpan<byte> vertices, VertexChannel stream, int elementSize, int count)
    {
        var length = elementSize * count;

        if (stream.Offset + length > vertices.Length)
        {
            throw new ModelFormatException($"stream in slot {stream.Slot} runs past the vertex buffer ({count} vertices)");
        }

        return vertices.Slice(stream.Offset, length);
    }

    internal static VertexChannel? Find(IReadOnlyList<VertexChannel> streams, int slot)
    {
        foreach (var stream in streams)
        {
            if (stream.Slot == slot)
            {
                return stream;
            }
        }

        return null;
    }

    private static ReadOnlySpan<byte> Buffer(ResourceView view, int bufferObject, ushort kind)
    {
        if (view.U16(bufferObject + BufferKind) != kind)
        {
            throw new ModelFormatException($"buffer object at 0x{bufferObject:X} is of kind {view.U16(bufferObject + BufferKind)}, not {kind}");
        }

        var size = (int)view.U32(bufferObject + BufferDwords) * sizeof(uint);

        return view.PhysicalBytes(view.FollowPhysical(bufferObject + BufferData), size);
    }

    private static float[] Positions(ReadOnlySpan<byte> vertices, IReadOnlyList<VertexChannel> streams, int count)
    {
        var stream = Find(streams, VertexSlot.Position);

        if (stream.HasValue && stream.Value.Format == VertexFormat.Float3)
        {
            return VertexAttributes.Floats(Slice(vertices, stream.Value, count), count);
        }

        if (!stream.HasValue || stream.Value.Format != VertexFormat.Eight || stream.Value.Offset < PackedPositions.HeaderBytes)
        {
            throw new ModelFormatException($"position stream {Describe(stream)} is neither packed nor three floats");
        }

        var data = Slice(vertices, stream.Value, count);
        var header = vertices.Slice(stream.Value.Offset - PackedPositions.HeaderBytes, PackedPositions.HeaderBytes);

        return PackedPositions.Decode(header, data, count);
    }

    private static float[]? Normals(ReadOnlySpan<byte> vertices, IReadOnlyList<VertexChannel> streams, int count)
    {
        var stream = Find(streams, VertexSlot.Normal);

        if (stream.HasValue && stream.Value.Format == VertexFormat.Float3)
        {
            return VertexAttributes.Floats(Slice(vertices, stream.Value, count), count);
        }

        if (!stream.HasValue || stream.Value.Format != VertexFormat.PackedNormal)
        {
            return null;
        }

        return VertexAttributes.OctahedralNormals(Slice(vertices, stream.Value, count), count);
    }

    private static string Describe(VertexChannel? stream)
    {
        if (!stream.HasValue)
        {
            return "(missing)";
        }

        return $"0x{stream.Value.Format:X2}";
    }
}
