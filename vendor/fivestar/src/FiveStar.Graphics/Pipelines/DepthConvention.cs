// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;

namespace FiveStar.Graphics.Pipelines;

/// <summary>
/// Which way depth runs. The game's main passes use reverse-Z (near = 1, far = 0, test GREATER_EQUAL, clear to 0);
/// shadow and linear-depth passes use the forward convention (near = 0, test LESS_EQUAL, clear to 1).
/// </summary>
public sealed record DepthConvention(bool Reversed)
{
    public static DepthConvention Reverse { get; } = new(true);

    public static DepthConvention Forward { get; } = new(false);

    /// <summary>The value the depth target is cleared to: the far plane.</summary>
    public float Far
    {
        get
        {
            if (Reversed)
            {
                return 0;
            }

            return 1;
        }
    }

    /// <summary>The test that keeps the nearer fragment.</summary>
    public ComparisonFunction Test
    {
        get
        {
            if (Reversed)
            {
                return ComparisonFunction.GreaterEqual;
            }

            return ComparisonFunction.LessEqual;
        }
    }

    /// <summary>The convention a depth test implies: GREATER or GREATER_EQUAL is reverse-Z, anything else forward.</summary>
    public static DepthConvention Of(ComparisonFunction test)
    {
        if (test == ComparisonFunction.Greater || test == ComparisonFunction.GreaterEqual)
        {
            return Reverse;
        }

        return Forward;
    }

    /// <summary>The stored depth of a point at <paramref name="distance"/> between the near (0) and far (1) planes.</summary>
    public float Depth(float distance)
    {
        if (Reversed)
        {
            return 1 - distance;
        }

        return distance;
    }
}
