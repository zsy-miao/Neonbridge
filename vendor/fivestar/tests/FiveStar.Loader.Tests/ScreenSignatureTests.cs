// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Input;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Screen signatures: a small change moves few cells, another layout many, and the text form round-trips.</summary>
public sealed class ScreenSignatureTests
{
    private const int Width = 480;
    private const int Height = 270;

    [Fact]
    public void Signatures_count_the_cells_a_change_covers()
    {
        var blank = Image((x, y) => 20);
        var focused = Image((x, y) => Lit(x < 20 && y < 20));
        var other = Image((x, y) => Lit(y < Height / 2));

        var signature = ScreenSignature.Of(blank, Width, Height);

        Assert.Equal(0, signature.Distance(ScreenSignature.Parse(signature.ToString())!));
        Assert.Equal(4, signature.Distance(ScreenSignature.Of(focused, Width, Height)));
        Assert.True(signature.Distance(ScreenSignature.Of(other, Width, Height)) > ScreenStore.DefaultMaxChanged);
    }

    private static int Lit(bool lit)
    {
        if (lit)
        {
            return 200;
        }

        return 20;
    }

    private static byte[] Image(Func<int, int, int> grey)
    {
        var rgba = new byte[Width * Height * 4];

        for (int y = 0; y < Height; y++)
        {
            for (int x = 0; x < Width; x++)
            {
                int at = ((y * Width) + x) * 4;
                rgba[at] = rgba[at + 1] = rgba[at + 2] = (byte)grey(x, y);
                rgba[at + 3] = 255;
            }
        }

        return rgba;
    }
}
