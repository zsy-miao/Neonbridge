// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Graphics;

/// <summary>A rendered sample: the scene, its RGBA8 pixels (rows top first) and the debug layer's errors, if it ran.</summary>
public sealed record SampleFrame(UnlitScene Scene, byte[] Pixels, IReadOnlyList<string> Errors)
{
    private const int BytesPerPixel = 4;

    /// <summary>The four bytes of pixel (x, y).</summary>
    public ReadOnlySpan<byte> Pixel(int x, int y)
    {
        return Pixels.AsSpan(((y * Scene.Width) + x) * BytesPerPixel, BytesPerPixel);
    }
}
