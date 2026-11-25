// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using FiveStar.Shaders.Bvh;
using FiveStar.Shaders.Tests.Reference;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// A TLAS built on the CPU, walked the way the game's traversal reads it (CS_TraceShadowRays_Half): box nodes with the
/// RT IP 1.1 reference model; at an instance node the mask byte at 0x30, the BLAS root at 0x38 and base at 0x3C, and
/// the ray taken to object space by the translation at 0x40 and the rows at 0x4C; then the BLAS with the reference.
/// </summary>
public sealed class XdxrTlasTests
{
    private const int BlasAddress = 0x10000;
    private const int MemorySize = 0x20000;
    private const float Extent = 1000f;
    private const float Height = 5f;
    private const uint RayMask = 0xD5000000;

    [Fact]
    public void Two_instances_of_one_blas_are_hit_at_both_transforms()
    {
        BvhTriangle[] quad =
        [
            new(new Vector3(0, 0, 0), new Vector3(1, 0, 0), new Vector3(0, 1, 0), 0, 0, 1),
            new(new Vector3(1, 0, 0), new Vector3(1, 1, 0), new Vector3(0, 1, 0), 0, 1, 1),
        ];
        var blas = XdxrBlas.Build(quad);
        uint root = XdxrBlas.Root(blas);
        var bounds = XdxrBlas.Bounds(blas);

        // Instance 0 moved by (10, 0, 0); instance 1 scaled by 2, turned 90 degrees about z and moved by (0, 20, 3).
        float[] moved = [1, 0, 0, 10, 0, 1, 0, 0, 0, 0, 1, 0];
        float[] turned = [0, -2, 0, 0, 2, 0, 0, 20, 0, 0, 2, 3];
        BvhInstance[] instances =
        [
            new(moved, 7, 0xFF, 0, 0, BlasAddress, root, bounds),
            new(turned, 8, 0xFF, 0, 0, BlasAddress, root, bounds),
        ];
        var tlas = XdxrTlas.Build(instances);
        var memory = Load(tlas, blas);

        foreach (var (instance, transform) in new[] { (0u, moved), (1u, turned) })
        {
            foreach (var triangle in quad)
            {
                var target = Apply(transform, (triangle.V0 + triangle.V1 + triangle.V2) / 3f);
                var hit = Trace(memory, target + new Vector3(0, 0, Height));

                Assert.Equal((instance, triangle.PrimitiveIndex), (hit.Instance, hit.Primitive));
                Assert.Equal(Height, hit.Distance, 3);
            }
        }

        Assert.Equal(uint.MaxValue, Trace(memory, new Vector3(50, 50, Height)).Instance);
    }

    private static (uint Instance, uint Primitive, float Distance) Trace(BvhMemory memory, Vector3 origin)
    {
        var best = (Instance: uint.MaxValue, Primitive: uint.MaxValue, Distance: float.PositiveInfinity);
        var tlas = BvhReference.Descriptor(0, sort: false, grow: 0, returnBarycentrics: false);
        var stack = new Stack<uint>([memory.ReadWord(XdxrBlas.RootOffset)]);
        var down = new Vector3(0, 0, -1);

        while (stack.Count > 0)
        {
            uint node = stack.Pop();
            int address = (int)((node & ~7u) << 3);

            if ((node & 7) == AmdBvhLayout.Box32Type)
            {
                foreach (uint child in BvhReference.Intersect(memory, tlas, node, Ray(origin, down)).Where(child => child != BvhReference.Invalid))
                {
                    stack.Push(child);
                }

                continue;
            }

            if ((memory.ReadWord(address + 0x30) & RayMask) == 0)
            {
                continue;
            }

            var (primitive, distance) = WalkInstance(memory, address, origin, down);

            if (distance < best.Distance)
            {
                best = (memory.ReadWord(address + 0x70), primitive, distance);
            }
        }

        return best;
    }

    private static (uint Primitive, float Distance) WalkInstance(BvhMemory memory, int node, Vector3 origin, Vector3 direction)
    {
        var translation = new Vector3(Float(memory, node + 0x40), Float(memory, node + 0x44), Float(memory, node + 0x48));
        float[] rows = Enumerable.Range(0, 9).Select(i => Float(memory, node + 0x4C + (4 * i))).ToArray();
        var ray = Ray(Rotate(rows, origin) + translation, Rotate(rows, direction));

        uint root = memory.ReadWord(node + 0x38) & 0x3FFFFFFF;
        var blas = BvhReference.Descriptor((int)(memory.ReadWord(node + 0x3C) << 8), sort: false, grow: 0, returnBarycentrics: false);
        var best = (Primitive: uint.MaxValue, Distance: float.PositiveInfinity);
        var stack = new Stack<uint>([root]);

        while (stack.Count > 0)
        {
            uint child = stack.Pop();
            var result = BvhReference.Intersect(memory, blas, child, ray);

            if ((child & 7) == AmdBvhLayout.Box32Type)
            {
                foreach (uint next in result.Where(next => next != BvhReference.Invalid))
                {
                    stack.Push(next);
                }

                continue;
            }

            float t = BitConverter.UInt32BitsToSingle(result[0]) / BitConverter.UInt32BitsToSingle(result[1]);

            if (result[3] != 0 && t < best.Distance)
            {
                best = (memory.ReadWord(BlasAddress + (int)((child & ~7u) << 3) + 52), t);
            }
        }

        return best;
    }

    private static BvhRay Ray(Vector3 origin, Vector3 direction)
    {
        return new BvhRay(Extent, [origin.X, origin.Y, origin.Z], [direction.X, direction.Y, direction.Z], [1f / direction.X, 1f / direction.Y, 1f / direction.Z]);
    }

    private static Vector3 Rotate(float[] rows, Vector3 v)
    {
        return new Vector3(
            (rows[0] * v.X) + (rows[1] * v.Y) + (rows[2] * v.Z),
            (rows[3] * v.X) + (rows[4] * v.Y) + (rows[5] * v.Z),
            (rows[6] * v.X) + (rows[7] * v.Y) + (rows[8] * v.Z));
    }

    private static Vector3 Apply(float[] m, Vector3 v)
    {
        return new Vector3(
            (m[0] * v.X) + (m[1] * v.Y) + (m[2] * v.Z) + m[3],
            (m[4] * v.X) + (m[5] * v.Y) + (m[6] * v.Z) + m[7],
            (m[8] * v.X) + (m[9] * v.Y) + (m[10] * v.Z) + m[11]);
    }

    private static float Float(BvhMemory memory, int address)
    {
        return BitConverter.UInt32BitsToSingle(memory.ReadWord(address));
    }

    private static BvhMemory Load(byte[] tlas, byte[] blas)
    {
        var memory = new BvhMemory(MemorySize);

        for (int at = 0; at < tlas.Length; at += 4)
        {
            memory.WriteWord(at, BitConverter.ToUInt32(tlas, at));
        }

        for (int at = 0; at < blas.Length; at += 4)
        {
            memory.WriteWord(BlasAddress + at, BitConverter.ToUInt32(blas, at));
        }

        return memory;
    }
}
