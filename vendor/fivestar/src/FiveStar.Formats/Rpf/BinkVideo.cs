// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Rpf;

/// <summary>A Bink 2 video found inside an archive.</summary>
/// <param name="Offset">Archive offset of the <c>KB2</c> magic.</param>
/// <param name="Size">File size: the header's size field plus 8.</param>
/// <param name="Revision">The revision letter after <c>KB2</c>.</param>
/// <param name="Frames">Frame count.</param>
/// <param name="Width">Width in pixels.</param>
/// <param name="Height">Height in pixels.</param>
/// <param name="FrameRateNumerator">Frame rate numerator.</param>
/// <param name="FrameRateDenominator">Frame rate denominator.</param>
public sealed record BinkVideo(
    long Offset,
    long Size,
    char Revision,
    uint Frames,
    uint Width,
    uint Height,
    uint FrameRateNumerator,
    uint FrameRateDenominator)
{
    public double FramesPerSecond => (double)FrameRateNumerator / FrameRateDenominator;
}
