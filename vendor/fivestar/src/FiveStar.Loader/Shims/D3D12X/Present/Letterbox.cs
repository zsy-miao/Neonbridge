// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The largest rectangle of a source's aspect ratio that fits a target, centered on it, in whole pixels: bars above and
/// below when the target is wider in proportion, at the sides when it is narrower.
/// </summary>
internal readonly record struct Letterbox(int X, int Y, int Width, int Height)
{
    /// <summary>Halves: the bars on both sides, and rounding to the nearest pixel.</summary>
    public const int Halves = 2;

    [HotPath]
    public static Letterbox Fit(uint sourceWidth, uint sourceHeight, uint targetWidth, uint targetHeight)
    {
        if (sourceWidth == 0 || sourceHeight == 0)
        {
            return new Letterbox(0, 0, (int)targetWidth, (int)targetHeight);
        }

        // Compare targetWidth / targetHeight with sourceWidth / sourceHeight without rounding.
        ulong widthBySource = (ulong)targetWidth * sourceHeight;
        ulong heightBySource = (ulong)targetHeight * sourceWidth;
        uint width = targetWidth;
        uint height = targetHeight;

        if (widthBySource > heightBySource)
        {
            width = AtLeastOne((heightBySource + (sourceHeight / Halves)) / sourceHeight);
        }
        else if (widthBySource < heightBySource)
        {
            height = AtLeastOne((widthBySource + (sourceWidth / Halves)) / sourceWidth);
        }

        return new Letterbox((int)(targetWidth - width) / Halves, (int)(targetHeight - height) / Halves, (int)width, (int)height);
    }

    [HotPath]
    private static uint AtLeastOne(ulong value)
    {
        if (value == 0)
        {
            return 1;
        }

        return (uint)value;
    }
}
