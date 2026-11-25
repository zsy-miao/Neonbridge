// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using System.Runtime.InteropServices;
using FiveStar.Shaders.Bvh;
using FiveStar.Shaders.Tests.Reference;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The CPU BVH4 builder: a ray walking the built nodes with the RT IP 1.1 reference model of image_bvh_intersect_ray
/// finds the same closest triangle, at the same distance, as testing every triangle with that model.
/// </summary>
public sealed class BvhBuilderTests
{
    private const int TriangleCount = 500;
    private const int RayCount = 400;
    private const float Extent = 1000f;

    [Fact]
    public void Traversal_of_a_built_bvh_finds_the_closest_triangle()
    {
        var random = new Random(1234);
        var triangles = Enumerable.Range(0, TriangleCount).Select(i => RandomTriangle(random, (uint)i)).ToList();
        var image = Bvh4Builder.Build(triangles, 0);
        var memory = Load(image.Nodes);
        var descriptor = BvhReference.Descriptor(0, sort: false, grow: 0, returnBarycentrics: false);
        int hits = 0;

        for (int i = 0; i < RayCount; i++)
        {
            var target = triangles[random.Next(TriangleCount)];
            var ray = RayTowards(random, (target.V0 + target.V1 + target.V2) / 3f);
            var walked = Walk(memory, descriptor, image.Root, ray);
            var brute = BruteForce(triangles, ray);

            Assert.Equal(brute, walked);

            if (brute.Primitive != uint.MaxValue)
            {
                hits++;
            }
        }

        Assert.True(hits > RayCount / 2, $"only {hits} rays hit");
        Assert.Equal(TriangleCount, image.TriangleNodes);
    }

    /// <summary>
    /// A BLAS built from an indexed mesh (R16 indices, f32 positions, a 3x4 transform) in the console layout: the root
    /// comes from the header at 0x30 and a ray down onto each transformed triangle finds that triangle.
    /// </summary>
    [Fact]
    public void Runtime_blas_from_buffers_is_traversed_from_its_header_root()
    {
        const int Grid = 4;
        var vertices = new List<float>();
        var indices = new List<ushort>();

        for (int y = 0; y <= Grid; y++)
        {
            for (int x = 0; x <= Grid; x++)
            {
                vertices.AddRange([x, y, 0f]);
            }
        }

        for (int y = 0; y < Grid; y++)
        {
            for (int x = 0; x < Grid; x++)
            {
                ushort corner = (ushort)((y * (Grid + 1)) + x);
                indices.AddRange([corner, (ushort)(corner + 1), (ushort)(corner + Grid + 1)]);
                indices.AddRange([(ushort)(corner + 1), (ushort)(corner + Grid + 2), (ushort)(corner + Grid + 1)]);
            }
        }

        float[] transform = [2, 0, 0, 10, 0, 2, 0, -3, 0, 0, 1, 7];
        var triangles = new List<BvhTriangle>();
        var buffers = new TriangleBuffers(
            MemoryMarshal.AsBytes(vertices.ToArray().AsSpan()), 12, 6, (uint)(vertices.Count / 3),
            MemoryMarshal.AsBytes(indices.ToArray().AsSpan()), 57, (uint)indices.Count, transform, 0, 1);
        Assert.True(TriangleDecoder.Append(triangles, buffers));

        var structure = XdxrBlas.Build(triangles);
        var memory = Load(structure);
        uint root = memory.ReadWord(XdxrBlas.RootOffset);
        var descriptor = BvhReference.Descriptor(0, sort: false, grow: 0, returnBarycentrics: false);

        Assert.Equal(AmdBvhLayout.NodePointer(XdxrBlas.HeaderSize, AmdBvhLayout.Box32Type), root);
        Assert.Equal((ulong)structure.Length, XdxrBlas.Size(structure));
        Assert.Equal(2 * Grid * Grid, triangles.Count);

        foreach (var triangle in triangles)
        {
            var target = (triangle.V0 + triangle.V1 + triangle.V2) / 3f;
            var ray = new BvhRay(Extent, Vertex(target + new Vector3(0, 0, 5)), [0, 0, -1], [float.PositiveInfinity, float.PositiveInfinity, -1]);
            var (primitive, distance) = Walk(memory, descriptor, root, ray);

            Assert.Equal(triangle.PrimitiveIndex, primitive);
            Assert.Equal(5f, BitConverter.UInt32BitsToSingle(distance), 3);
        }
    }

    private static (uint Primitive, uint Distance) Walk(BvhMemory memory, uint[] descriptor, uint root, BvhRay ray)
    {
        var best = (Primitive: uint.MaxValue, Distance: float.PositiveInfinity);
        var stack = new Stack<uint>([root]);

        while (stack.Count > 0)
        {
            uint node = stack.Pop();
            var result = BvhReference.Intersect(memory, descriptor, node, ray);

            if ((node & 7) == AmdBvhLayout.Box32Type)
            {
                foreach (uint child in result.Where(child => child != BvhReference.Invalid))
                {
                    stack.Push(child);
                }

                continue;
            }

            uint primitive = memory.ReadWord((int)((node & ~7u) << 3) + 52);
            best = Closer(best, result, primitive);
        }

        return (best.Primitive, BitConverter.SingleToUInt32Bits(best.Distance));
    }

    private static (uint Primitive, uint Distance) BruteForce(List<BvhTriangle> triangles, BvhRay ray)
    {
        var best = (Primitive: uint.MaxValue, Distance: float.PositiveInfinity);
        var descriptor = BvhReference.Descriptor(0, sort: false, grow: 0, returnBarycentrics: false);

        foreach (var triangle in triangles)
        {
            var single = new BvhMemory(64);
            single.WriteTriangle(0, [Vertex(triangle.V0), Vertex(triangle.V1), Vertex(triangle.V2)], 9);
            best = Closer(best, BvhReference.Intersect(single, descriptor, 0, ray), triangle.PrimitiveIndex);
        }

        return (best.Primitive, BitConverter.SingleToUInt32Bits(best.Distance));
    }

    /// <summary>Return mode 0: t_num, t_denom, triangle_id, hit. Ties keep the lower primitive index.</summary>
    private static (uint Primitive, float Distance) Closer((uint Primitive, float Distance) best, uint[] result, uint primitive)
    {
        if (result[3] == 0)
        {
            return best;
        }

        float t = BitConverter.UInt32BitsToSingle(result[0]) / BitConverter.UInt32BitsToSingle(result[1]);
        bool closer = t < best.Distance || (t == best.Distance && primitive < best.Primitive);

        if (t > Extent || !closer)
        {
            return best;
        }

        return (primitive, t);
    }

    private static BvhMemory Load(byte[] nodes)
    {
        var memory = new BvhMemory(nodes.Length);

        for (int at = 0; at < nodes.Length; at += 4)
        {
            memory.WriteWord(at, BitConverter.ToUInt32(nodes, at));
        }

        return memory;
    }

    private static BvhTriangle RandomTriangle(Random random, uint index)
    {
        var center = new Vector3(Next(random, -50, 50), Next(random, -50, 50), Next(random, -50, 50));

        return new BvhTriangle(Corner(random, center), Corner(random, center), Corner(random, center), 0, index, 0);
    }

    private static Vector3 Corner(Random random, Vector3 center)
    {
        return center + new Vector3(Next(random, -4, 4), Next(random, -4, 4), Next(random, -4, 4));
    }

    private static BvhRay RayTowards(Random random, Vector3 target)
    {
        var origin = new Vector3(Next(random, -80, 80), Next(random, -80, 80), Next(random, -80, 80));
        var direction = Vector3.Normalize(target - origin);
        var inverse = new Vector3(1f / direction.X, 1f / direction.Y, 1f / direction.Z);

        return new BvhRay(Extent, Vertex(origin), Vertex(direction), Vertex(inverse));
    }

    private static double[] Vertex(Vector3 value)
    {
        return [value.X, value.Y, value.Z];
    }

    private static float Next(Random random, float low, float high)
    {
        return low + ((high - low) * random.NextSingle());
    }
}
