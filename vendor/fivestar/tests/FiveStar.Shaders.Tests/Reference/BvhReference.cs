// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// IMAGE_BVH_INTERSECT_RAY / IMAGE_BVH64_INTERSECT_RAY (RDNA2, RT IP 1.1), written from AMD GPURT
/// (IntersectCommon.hlsl: IntersectNodeBvh4, fast_intersect_bbox, fast_intersect_triangle, SwizzleBarycentrics), the
/// GPURT node layouts and Mesa radv's BVH descriptor, never from the translator's rdna_bvh.hlsli.
/// </summary>
public static class BvhReference
{
    public const uint Invalid = 0xFFFFFFFF;

    // Vertex triples of triangle node types 0..3 (one nibble per vertex slot).
    private static readonly int[] TriangleMap = [0x210, 0x231, 0x432, 0x042];

    // Sorting network of the four children, closest first.
    private static readonly (int A, int B)[] SortPairs = [(0, 2), (1, 3), (0, 1), (2, 3), (1, 2)];

    /// <summary>BVH T#: base byte address (256-aligned), sort flag, box grow ULPs, triangle return mode.</summary>
    public static uint[] Descriptor(int baseAddress, bool sort, int grow, bool returnBarycentrics)
    {
        const ulong SizeMinusOne = (1UL << 42) - 1;
        var sortBit = sort ? 0x80000000u : 0;
        var returnBit = returnBarycentrics ? 1u << 24 : 0;

        return
        [
            (uint)(baseAddress >> 8),
            ((uint)(grow & 0xFF) << 23) | sortBit,
            unchecked((uint)SizeMinusOne),
            (uint)(SizeMinusOne >> 32) | returnBit | (8u << 28),
        ];
    }

    /// <summary>A node pointer: 64-byte aligned offset >> 3, node type in the low 3 bits.</summary>
    public static uint NodePointer(int offset, int type)
    {
        return (uint)(offset >> 3) | (uint)type;
    }

    /// <summary>The four result dwords for one ray against one node.</summary>
    public static uint[] Intersect(BvhMemory memory, uint[] descriptor, uint node, BvhRay ray)
    {
        var type = (int)(node & 7);
        var address = (int)((descriptor[0] << 8) + ((node & ~7u) << 3));

        if (type >= 6)
        {
            return [Invalid, Invalid, Invalid, Invalid];
        }

        if (type >= 4)
        {
            return BoxNode(memory, address, type == 4, ray, (int)((descriptor[1] >> 23) & 0xFF), (descriptor[1] >> 31) != 0);
        }

        return TriangleNode(memory, address, type, ((descriptor[3] >> 24) & 1) != 0, ray);
    }

    /// <summary>GPURT fast_intersect_bbox: (entry, exit) distances of a ray and a box.</summary>
    private static (double Min, double Max) BoxInterval(BvhRay ray, double[] min, double[] max)
    {
        var low = new double[3];
        var high = new double[3];

        for (var axis = 0; axis < 3; axis++)
        {
            var near = BvhFloat.Multiply(BvhFloat.Subtract(min[axis], ray.Origin[axis]), ray.InverseDirection[axis]);
            var far = BvhFloat.Multiply(BvhFloat.Subtract(max[axis], ray.Origin[axis]), ray.InverseDirection[axis]);
            var positive = ray.InverseDirection[axis] >= 0.0;
            low[axis] = positive ? near : far;
            high[axis] = positive ? far : near;
        }

        var entry = BvhFloat.MaxNumber(BvhFloat.MaxNumber(low[0], low[1]), low[2]);
        var exit = BvhFloat.MinNumber(BvhFloat.MinNumber(high[0], high[1]), high[2]);

        if (double.IsNaN(entry) || double.IsNaN(exit))
        {
            return (double.PositiveInfinity, double.NegativeInfinity);
        }

        return (BvhFloat.MaxNumber(entry, 0.0), BvhFloat.MinNumber(exit, ray.Extent));
    }

    private static uint[] BoxNode(BvhMemory memory, int address, bool half, BvhRay ray, int grow, bool sort)
    {
        var children = new uint[4];
        var keys = new double[4];
        var growFactor = BvhFloat.Add(1.0, BvhFloat.Multiply(grow, Math.Pow(2, -24)));

        for (var child = 0; child < 4; child++)
        {
            var (min, max) = ReadBox(memory, address, child, half);
            var (entry, exit) = BoxInterval(ray, min, max);
            var hit = entry <= BvhFloat.Multiply(exit, growFactor);
            children[child] = hit ? memory.ReadWord(address + (4 * child)) : Invalid;
            keys[child] = entry;
        }

        if (sort)
        {
            foreach (var (a, b) in SortPairs)
            {
                SortPair(children, keys, a, b);
            }
        }

        return children;
    }

    private static (double[] Min, double[] Max) ReadBox(BvhMemory memory, int address, int child, bool half)
    {
        if (half)
        {
            var at = address + 16 + (12 * child);
            var (w0, w1, w2) = (memory.ReadWord(at), memory.ReadWord(at + 4), memory.ReadWord(at + 8));
            double[] min = [FloatBits.HalfValue(w0), FloatBits.HalfValue(w0 >> 16), FloatBits.HalfValue(w1)];
            double[] max = [FloatBits.HalfValue(w1 >> 16), FloatBits.HalfValue(w2), FloatBits.HalfValue(w2 >> 16)];

            return (min, max);
        }

        var minimum = new double[3];
        var maximum = new double[3];

        for (var axis = 0; axis < 3; axis++)
        {
            minimum[axis] = memory.ReadFloat(address + 16 + (24 * child) + (4 * axis));
            maximum[axis] = memory.ReadFloat(address + 28 + (24 * child) + (4 * axis));
        }

        return (minimum, maximum);
    }

    private static void SortPair(uint[] children, double[] keys, int a, int b)
    {
        if ((children[b] != Invalid && keys[b] < keys[a]) || children[a] == Invalid)
        {
            (children[a], children[b]) = (children[b], children[a]);
            (keys[a], keys[b]) = (keys[b], keys[a]);
        }
    }

    private static uint[] TriangleNode(BvhMemory memory, int address, int type, bool returnBarycentrics, BvhRay ray)
    {
        var map = TriangleMap[type];
        var v = new double[3][];

        for (var vertex = 0; vertex < 3; vertex++)
        {
            var slot = (map >> (4 * vertex)) & 0xF;
            v[vertex] = [.. Enumerable.Range(0, 3).Select(axis => memory.ReadFloat(address + (12 * slot) + (4 * axis)))];
        }

        var triangleId = memory.ReadWord(address + 60);
        var e1 = Difference(v[1], v[0]);
        var e2 = Difference(v[2], v[0]);
        var e3 = Difference(ray.Origin, v[0]);

        var s1 = Cross(ray.Direction, e2);
        var s2 = Cross(e3, e1);
        double[] r = [Dot(e2, s2), Dot(s1, e1), Dot(e3, s1), Dot(ray.Direction, s2)];

        return TriangleResult(r, triangleId, type, returnBarycentrics);
    }

    private static uint[] TriangleResult(double[] r, uint triangleId, int type, bool returnBarycentrics)
    {
        var t = BvhFloat.Divide(r[0], r[1]);
        var u = BvhFloat.Divide(r[2], r[1]);
        var w = BvhFloat.Divide(r[3], r[1]);
        var miss = u < 0.0 || u > 1.0 || w < 0.0 || BvhFloat.Add(u, w) > 1.0 || t < 0.0;

        if (miss)
        {
            r[0] = double.PositiveInfinity;
            r[1] = 1.0;
        }

        if (!returnBarycentrics)
        {
            var hit = miss ? 0u : 1u;

            return [BvhFloat.Bits(r[0]), BvhFloat.Bits(r[1]), triangleId, hit];
        }

        double[] barycentrics = [BvhFloat.Subtract(BvhFloat.Subtract(r[1], r[2]), r[3]), r[2], r[3], 0.0];
        var shift = 8 * type;
        var i = barycentrics[(triangleId >> shift) & 3];
        var j = barycentrics[(triangleId >> (shift + 2)) & 3];

        return [BvhFloat.Bits(r[0]), BvhFloat.Bits(r[1]), BvhFloat.Bits(i), BvhFloat.Bits(j)];
    }

    private static double[] Difference(double[] a, double[] b)
    {
        return [BvhFloat.Subtract(a[0], b[0]), BvhFloat.Subtract(a[1], b[1]), BvhFloat.Subtract(a[2], b[2])];
    }

    private static double[] Cross(double[] a, double[] b)
    {
        return
        [
            BvhFloat.Subtract(BvhFloat.Multiply(a[1], b[2]), BvhFloat.Multiply(a[2], b[1])),
            BvhFloat.Subtract(BvhFloat.Multiply(a[2], b[0]), BvhFloat.Multiply(a[0], b[2])),
            BvhFloat.Subtract(BvhFloat.Multiply(a[0], b[1]), BvhFloat.Multiply(a[1], b[0])),
        ];
    }

    private static double Dot(double[] a, double[] b)
    {
        return BvhFloat.Add(BvhFloat.Add(BvhFloat.Multiply(a[0], b[0]), BvhFloat.Multiply(a[1], b[1])), BvhFloat.Multiply(a[2], b[2]));
    }
}
