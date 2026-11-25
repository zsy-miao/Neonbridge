// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Numerics;

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// A BLAS built on the CPU in the console's structure layout (docs/raytracing.md): the 64-byte header a rehydrated BLAS
/// has, then the <see cref="Bvh4Builder"/> nodes. The shaders and a TLAS take the root node pointer from dword 12;
/// node pointers count from the start of the structure.
/// </summary>
public static class XdxrBlas
{
    /// <summary>Header bytes; the nodes follow.</summary>
    public const int HeaderSize = 0x40;

    /// <summary>The structure is a whole number of 64-byte blocks; dword 1 holds the count minus one.</summary>
    public const int BlockSize = 64;

    /// <summary>The header dword holding the root node pointer.</summary>
    public const int RootOffset = 0x30;

    // Header dwords (docs/raytracing.md, serialized BLAS): flags, size, triangle count, bounds, node offsets, copy
    // size, vertex transform. Dword 11 (offsetToPrimRemap) stays 0, as in every serialized BLAS: no remap table.
    private const int FlagsOffset = 0x00;
    private const int BlocksOffset = 0x04;
    private const int TrianglesOffset = 0x0C;
    private const int BoundsOffset = 0x10;
    private const int BoxesOffset = 0x28;

    private const int TriangleNodesOffset = 0x34;
    private const int CopySizeOffset = 0x38;
    private const int VertexTransformOffset = 0x3C;
    private const int FloatSize = 4;
    private const int Axes = 3;

    /// <summary>
    /// Dword 0 of a BLAS: bit 31 bottom level, low 16 bits format version 6 (set in every serialized BLAS; the serialized
    /// ones also carry bit 24, not yet rehydrated, and bit 17).
    /// </summary>
    private const uint BottomLevelFlags = 0x80000006;

    /// <summary>Dword 15 without a vertex transform: three scale exponents of 127 (2^0).</summary>
    private const uint NoVertexTransform = 0x7F7F7F;

    /// <summary>Builds the structure over <paramref name="triangles"/> (transformed vertices).</summary>
    public static byte[] Build(IReadOnlyList<BvhTriangle> triangles)
    {
        var image = Bvh4Builder.Build(triangles, HeaderSize);
        int size = HeaderSize + image.Nodes.Length;
        var bytes = new byte[size];
        image.Nodes.CopyTo(bytes.AsSpan(HeaderSize));
        WriteHeader(bytes, image, Bounds(triangles));

        return bytes;
    }

    /// <summary>The structure size from a header (dword 1).</summary>
    public static ulong Size(ReadOnlySpan<byte> header)
    {
        return ((ulong)BinaryPrimitives.ReadUInt32LittleEndian(header[BlocksOffset..]) + 1) * BlockSize;
    }

    /// <summary>The root node pointer of a structure from its header (dword 12).</summary>
    public static uint Root(ReadOnlySpan<byte> header)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(header[RootOffset..]);
    }

    /// <summary>The bounds of everything in a structure from its header (dwords 4 to 9).</summary>
    public static BvhBounds Bounds(ReadOnlySpan<byte> header)
    {
        var min = new float[Axes];
        var max = new float[Axes];

        for (int axis = 0; axis < Axes; axis++)
        {
            min[axis] = BinaryPrimitives.ReadSingleLittleEndian(header[(BoundsOffset + (FloatSize * axis))..]);
            max[axis] = BinaryPrimitives.ReadSingleLittleEndian(header[(BoundsOffset + (FloatSize * (Axes + axis)))..]);
        }

        return new BvhBounds(new Vector3(min), new Vector3(max));
    }

    /// <summary>
    /// Writes the header fields a top- and a bottom-level structure share: flags, size, element count, bounds, the box
    /// offset, the root and the offset of the leaf nodes.
    /// </summary>
    internal static void WriteCommon(Span<byte> bytes, uint flags, int elements, BvhBounds bounds, uint root, int leaves)
    {
        Write(bytes, FlagsOffset, flags);
        Write(bytes, BlocksOffset, (uint)((bytes.Length / BlockSize) - 1));
        Write(bytes, TrianglesOffset, (uint)elements);

        for (int axis = 0; axis < Axes; axis++)
        {
            BinaryPrimitives.WriteSingleLittleEndian(bytes[(BoundsOffset + (FloatSize * axis))..], bounds.Min[axis]);
            BinaryPrimitives.WriteSingleLittleEndian(bytes[(BoundsOffset + (FloatSize * (Axes + axis)))..], bounds.Max[axis]);
        }

        Write(bytes, BoxesOffset, HeaderSize);
        Write(bytes, RootOffset, root);
        Write(bytes, TriangleNodesOffset, (uint)leaves);
    }

    private static void WriteHeader(Span<byte> bytes, Bvh4Image image, BvhBounds bounds)
    {
        int triangleNodes = HeaderSize + (image.BoxNodes * AmdBvhLayout.BoxNodeSize);
        WriteCommon(bytes, BottomLevelFlags, image.TriangleNodes, bounds, image.Root, triangleNodes);
        Write(bytes, CopySizeOffset, (uint)bytes.Length);
        Write(bytes, VertexTransformOffset, NoVertexTransform);
    }

    private static BvhBounds Bounds(IReadOnlyList<BvhTriangle> triangles)
    {
        var bounds = BvhBounds.Empty;

        foreach (var triangle in triangles)
        {
            bounds = bounds.Union(BvhBounds.Of(triangle));
        }

        return bounds;
    }

    private static void Write(Span<byte> bytes, int offset, uint value)
    {
        BinaryPrimitives.WriteUInt32LittleEndian(bytes[offset..], value);
    }
}
