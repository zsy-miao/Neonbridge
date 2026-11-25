// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// One pixel of a pixel-shader case and its place in the PC wave (from the probe shader, drawn as the case is). WARP runs
/// 4-lane waves, one 2x2 quad per wave, so there lane equals quad slot (0 top-left, 1 top-right, 2 bottom-left, 3
/// bottom-right); a hardware wave of 32 holds several quads. <see cref="Wave"/> names the live pixel in each lane of this
/// pixel's wave (null for a lane without one: unused or a helper).
/// </summary>
public readonly record struct PixelContext(int X, int Y, int Lane, int Quad, int WaveSize)
{
    public IReadOnlyList<(int X, int Y)?> Wave { get; init; } = [];

    /// <summary>The pixel at quad slot <paramref name="slot"/> of this pixel's quad.</summary>
    public (int X, int Y) QuadPixel(int slot)
    {
        return ((X & ~1) | (slot & 1), (Y & ~1) | (slot >> 1));
    }

    /// <summary>The live pixel in lane <paramref name="lane"/> of this pixel's PC wave, or null.</summary>
    public (int X, int Y)? WavePixel(int lane)
    {
        if (lane >= Wave.Count)
        {
            return null;
        }

        return Wave[lane];
    }
}
