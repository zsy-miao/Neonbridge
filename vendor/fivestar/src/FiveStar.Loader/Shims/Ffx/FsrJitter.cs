// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Ffx;

/// <summary>
/// The FSR camera jitter as the FidelityFX documentation defines it: a phase count of 8 times the squared upscale
/// ratio, and offsets from the Halton (2, 3) sequence minus one half. The shim answers these queries itself when no
/// upscaler is loaded, because the game divides its frame index by the phase count without checking the query.
/// </summary>
internal static class FsrJitter
{
    private const float BasePhaseCount = 8.0f;
    private const int HorizontalBase = 2;
    private const int VerticalBase = 3;
    private const float Half = 0.5f;

    /// <summary>The jitter phase count for an upscale from <paramref name="renderWidth"/> to <paramref name="displayWidth"/>; at least 1.</summary>
    public static int PhaseCount(uint renderWidth, uint displayWidth)
    {
        if (renderWidth == 0 || displayWidth == 0)
        {
            return 1;
        }

        float ratio = (float)displayWidth / renderWidth;

        return Math.Max((int)(BasePhaseCount * ratio * ratio), 1);
    }

    /// <summary>The sub-pixel offset of frame <paramref name="index"/>, each axis in [-0.5, 0.5).</summary>
    public static (float X, float Y) Offset(int index, int phaseCount)
    {
        if (phaseCount <= 0)
        {
            return (0, 0);
        }

        int position = (((index % phaseCount) + phaseCount) % phaseCount) + 1;

        return (Halton(position, HorizontalBase) - Half, Halton(position, VerticalBase) - Half);
    }

    private static float Halton(int index, int radix)
    {
        float fraction = 1.0f;
        float result = 0.0f;

        while (index > 0)
        {
            fraction /= radix;
            result += fraction * (index % radix);
            index /= radix;
        }

        return result;
    }
}
