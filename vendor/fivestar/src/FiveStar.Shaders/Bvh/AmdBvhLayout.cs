// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Numerics;

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// The BVH node layouts the translated <c>image_bvh_intersect_ray</c> reads (RT IP 1.1, docs/ray-intersection.md),
/// with the triangle-node fields the console's rehydration writes (docs/raytracing.md). A node pointer is the node's
/// byte offset from the BVH base shifted right by 3, with the node type in bits 2..0.
/// </summary>
public static class AmdBvhLayout
{
    /// <summary>A child slot or root that points nowhere.</summary>
    public const uint InvalidNode = 0xFFFFFFFF;

    public const int BoxNodeSize = 128;
    public const int TriangleNodeSize = 64;

    /// <summary>Node pointers address 64-byte units: offset &gt;&gt; 3 keeps 3 bits for the type.</summary>
    public const int NodeAlignment = 64;

    public const int Children = 4;

    /// <summary>Node types: triangle node holding V0, V1, V2 (type 0) and box32 (type 5).</summary>
    public const uint TriangleType = 0;
    public const uint Box32Type = 5;

    // Box32: four child pointers, then per child min xyz and max xyz as f32.
    private const int BoxBoundsOffset = 16;
    private const int BoxBoundsStride = 24;
    private const int BoxMaxOffset = 12;

    // A vector: x, y, z as consecutive f32.
    private const int YOffset = 4;
    private const int ZOffset = 8;

    // Triangle node: V0..V3 (12 bytes each), geometry index << 8 | flags, two primitive indices, triangle_id.
    private const int V1Offset = 12;
    private const int V2Offset = 24;
    private const int V3Offset = 36;
    private const int GeometryOffset = 48;
    private const int Primitive0Offset = 52;
    private const int Primitive1Offset = 56;
    private const int TriangleIdOffset = 60;
    private const int GeometryShift = 8;
    private const int PointerShift = 3;

    /// <summary>
    /// triangle_id of a lone type-0 triangle, as the console's rehydration writes it (with V3 = NaN): barycentric I
    /// from slot 1 (bits 1..0), J from slot 2 (bits 3..2), so a hit reports the triangle's own barycentrics.
    /// </summary>
    private const uint IdentityTriangleId = 0x9;

    public static uint NodePointer(int offset, uint type)
    {
        return ((uint)offset >> PointerShift) | type;
    }

    public static void WriteBox32(Span<byte> node, ReadOnlySpan<uint> children, ReadOnlySpan<BvhBounds> bounds)
    {
        for (int child = 0; child < Children; child++)
        {
            var box = child < children.Length ? bounds[child] : BvhBounds.Empty;
            uint pointer = child < children.Length ? children[child] : InvalidNode;
            BinaryPrimitives.WriteUInt32LittleEndian(node[(sizeof(uint) * child)..], pointer);
            WriteVector(node[(BoxBoundsOffset + (BoxBoundsStride * child))..], box.Min);
            WriteVector(node[(BoxBoundsOffset + (BoxBoundsStride * child) + BoxMaxOffset)..], box.Max);
        }
    }

    public static void WriteTriangle(Span<byte> node, BvhTriangle triangle)
    {
        node[..TriangleNodeSize].Clear();
        WriteVector(node, triangle.V0);
        WriteVector(node[V1Offset..], triangle.V1);
        WriteVector(node[V2Offset..], triangle.V2);
        WriteVector(node[V3Offset..], new Vector3(float.NaN));

        uint geometry = (triangle.GeometryIndex << GeometryShift) | triangle.GeometryFlags;
        BinaryPrimitives.WriteUInt32LittleEndian(node[GeometryOffset..], geometry);
        BinaryPrimitives.WriteUInt32LittleEndian(node[Primitive0Offset..], triangle.PrimitiveIndex);
        BinaryPrimitives.WriteUInt32LittleEndian(node[Primitive1Offset..], InvalidNode);
        BinaryPrimitives.WriteUInt32LittleEndian(node[TriangleIdOffset..], IdentityTriangleId);
    }

    /// <summary>The primitive index a triangle node reports.</summary>
    public static uint PrimitiveIndex(ReadOnlySpan<byte> node)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(node[Primitive0Offset..]);
    }

    private static void WriteVector(Span<byte> at, Vector3 value)
    {
        BinaryPrimitives.WriteSingleLittleEndian(at, value.X);
        BinaryPrimitives.WriteSingleLittleEndian(at[YOffset..], value.Y);
        BinaryPrimitives.WriteSingleLittleEndian(at[ZOffset..], value.Z);
    }
}
