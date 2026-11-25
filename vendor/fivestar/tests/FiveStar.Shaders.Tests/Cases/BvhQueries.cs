// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>64 intersect queries per case: edge cases first (written out), then seeded random rays.</summary>
public static class BvhQueries
{
    private const int Count = 64;
    private const double Extent = 100.0;

    /// <summary>Box queries: zero / negative-zero directions, slab planes, extents, origins inside, corners, ties, NaN slots.</summary>
    public static List<BvhQuery> Boxes(int seed)
    {
        var random = new Random(seed);
        var queries = new List<BvhQuery>
        {
            Make(BvhScene.Root, Extent, [-1, 0.5, 0.5], [1, 0, 0]), Make(BvhScene.Root, Extent, [-1, 0.0, 0.5], [1, 0, 0]),
            Make(BvhScene.Root, Extent, [5, 0.5, 0.5], [-1, -0.0, 0]), Make(BvhScene.Root, 0.5, [-1, 0.5, 0.5], [1, 0, 0]),
            Make(BvhScene.Root, 1.0, [-1, 0.5, 0.5], [1, 0, 0]), Make(BvhScene.Root, Extent, [0.5, 0.5, 0.5], [0.3, 0.2, -0.1]),
            Make(BvhScene.Root, Extent, [0.5, 0.5, 10], [0, 0, 1]), Make(BvhScene.Root, Extent, [-1, 1.0, 0.5], [1, 0, 0]),
            Make(BvhScene.Root, Extent, [-2, -2, -2], [1, 1, 1]), Make(BvhScene.Box16, Extent, [-3, -3, -3], [1, 1, 1]),
            Make(BvhScene.Box16, Extent, [1, 0.5, -1], [0, 0, 1]), Make(BvhScene.Box16, 0.0, [1, 1, 1], [1, 0, 0]),
            Make(BvhScene.Duplicates, Extent, [-1, 0.5, 0.5], [1, 0, 0]), Make(BvhScene.Duplicates, Extent, [2, 0.5, 0.5], [-1, 0, 0]),
            Make(BvhScene.Duplicates, Extent, [0.5, 0.5, -1], [0, 0, 1]), Make(BvhScene.Duplicates, Extent, [-1, 0.25, 0.25], [1, 0.125, 0.0625]),
        };

        while (queries.Count < Count)
        {
            var node = BvhScene.BoxNodes[queries.Count % 3];
            var extent = random.NextDouble() < 0.5 ? Extent : Uniform(random, 0.2, 3.0);

            if (random.NextDouble() < 0.35)
            {
                // Aimed at a box corner: entry equals exit up to rounding (the grow / equality cases).
                var corner = Enumerable.Range(0, 3).Select(_ => (double)random.Next(4)).ToArray();
                var origin = Enumerable.Range(0, 3).Select(_ => BvhFloat.Round(Uniform(random, -3, 6))).ToArray();
                queries.Add(Make(node, extent, origin, [.. corner.Zip(origin, (c, o) => c - o)]));
            }
            else
            {
                var (origin, direction) = Ray(random, [-1, -1, -1], [3.5, 3.5, 3.5], -3, 6);
                queries.Add(Make(node, extent, origin, direction));
            }
        }

        return queries;
    }

    /// <summary>Triangle queries: hits from both sides, edges, vertices, misses, t &lt; 0, t = 0, parallel rays, degenerate, node types 6 / 7.</summary>
    public static List<BvhQuery> Triangles(int seed)
    {
        var random = new Random(seed);
        var queries = new List<BvhQuery>
        {
            Make(BvhScene.TriangleA0, Extent, [0.5, 0.5, 0], [0, 0, 1]), Make(BvhScene.TriangleA0, Extent, [0.5, 0.5, 4], [0, 0, -1]),
            Make(BvhScene.TriangleA0, Extent, [1.0, 0.0, 0], [0, 0, 1]), Make(BvhScene.TriangleA0, Extent, [0.0, 0.0, 0], [0, 0, 1]),
            Make(BvhScene.TriangleA0, Extent, [1.0, 1.0, 0], [0, 0, 1]), Make(BvhScene.TriangleA0, Extent, [1.5, 1.5, 0], [0, 0, 1]),
            Make(BvhScene.TriangleA1, Extent, [1.5, 1.5, 0], [0, 0, 1]), Make(BvhScene.TriangleA0, Extent, [0.5, 0.5, 3], [0, 0, 1]),
            Make(BvhScene.TriangleA0, Extent, [0.5, 0.5, 2], [0, 0, 1]), Make(BvhScene.TriangleA0, Extent, [0.5, 0.5, 0], [1, 0, 0]),
            Make(BvhScene.TriangleA0, Extent, [-1, 0.5, 2], [1, 0, 0]), Make(BvhScene.TriangleC, Extent, [0.5, 0.5, 0], [0, 0, 1]),
            Make(BvhScene.TriangleA2, Extent, [1.0, 1.4, 0], [0.1, 0.2, 1]), Make(BvhScene.TriangleA3, Extent, [0.8, 0.6, 0], [0.1, 0.1, 1]),
            Make(BvhScene.TriangleB, Extent, [0.2, 0.3, 0], [0.05, -0.02, 1]), Make(BvhScene.Instance, Extent, [0, 0, 0], [0, 0, 1]),
            Make(BvhScene.User, Extent, [0, 0, 0], [0, 0, 1]),
        };

        while (queries.Count < Count)
        {
            var node = BvhScene.TriangleNodes[queries.Count % BvhScene.TriangleNodes.Count];
            double[] low = [-1.5, -1.5, 3.5];
            double[] high = [3.5, 3.5, 6.5];

            if ((node & ~7u) == (BvhScene.TriangleA0 & ~7u))
            {
                // The quad's triangles lie in z = 2.
                (low, high) = ([-0.5, -0.5, 2], [2.5, 2.5, 2]);
            }

            var (origin, direction) = Ray(random, low, high, -4, 9);
            queries.Add(Make(node, Extent, origin, direction));
        }

        return queries;
    }

    /// <summary>Box and triangle queries interleaved.</summary>
    public static List<BvhQuery> Mixed(int seed)
    {
        var boxes = Boxes(seed);
        var triangles = Triangles(seed + 1);

        return [.. boxes.Take(Count / 2).Zip(triangles).SelectMany(pair => new[] { pair.First, pair.Second })];
    }

    /// <summary>Mixed queries with the direction and inverse direction rounded to f16 (a16 inputs; overflow gives infinity).</summary>
    public static List<BvhQuery> Half(int seed)
    {
        return [.. Mixed(seed).Select(query =>
        {
            var direction = query.Ray.Direction.Select(value => ToHalf(Math.Clamp(value, -60000.0, 60000.0))).ToArray();
            var inverse = direction.Select(value => ToHalf(BvhFloat.Divide(1.0, value))).ToArray();

            return query with { Ray = query.Ray with { Direction = direction, InverseDirection = inverse } };
        })];
    }

    /// <summary>The value of a number rounded to f16 (values past the f16 range become infinities).</summary>
    public static double ToHalf(double value)
    {
        return FloatBits.HalfValue(FloatBits.Half(value));
    }

    private static BvhQuery Make(uint node, double extent, double[] origin, double[] direction)
    {
        var rounded = direction.Select(BvhFloat.Round).ToArray();
        var inverse = rounded.Select(value => BvhFloat.Divide(1.0, value)).ToArray();

        return new BvhQuery(node, new BvhRay(BvhFloat.Round(extent), [.. origin.Select(BvhFloat.Round)], rounded, inverse));
    }

    private static (double[] Origin, double[] Direction) Ray(Random random, double[] low, double[] high, double originLow, double originHigh)
    {
        var origin = Enumerable.Range(0, 3).Select(_ => BvhFloat.Round(Uniform(random, originLow, originHigh))).ToArray();
        var target = Enumerable.Range(0, 3).Select(axis => Uniform(random, low[axis], high[axis])).ToArray();

        return (origin, [.. target.Zip(origin, (t, o) => t - o)]);
    }

    private static double Uniform(Random random, double low, double high)
    {
        return low + ((high - low) * random.NextDouble());
    }
}
