// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Shaders.Bvh;

/// <summary>An axis-aligned box; <see cref="Empty"/> contains nothing and is the identity of <see cref="Union"/>.</summary>
public readonly record struct BvhBounds(Vector3 Min, Vector3 Max)
{
    public static BvhBounds Empty { get; } = new(new Vector3(float.PositiveInfinity), new Vector3(float.NegativeInfinity));

    public static BvhBounds Of(BvhTriangle triangle)
    {
        return new BvhBounds(
            Vector3.Min(Vector3.Min(triangle.V0, triangle.V1), triangle.V2),
            Vector3.Max(Vector3.Max(triangle.V0, triangle.V1), triangle.V2));
    }

    public BvhBounds Union(BvhBounds other)
    {
        return new BvhBounds(Vector3.Min(Min, other.Min), Vector3.Max(Max, other.Max));
    }
}
