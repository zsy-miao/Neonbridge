// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// A presented frame shrunk to <see cref="Columns"/> x <see cref="Rows"/> cells of mean luminance, one byte each: what
/// an input script's <c>screen</c> blocks recognise a screen by. Two signatures differ by the number of cells whose
/// luminance differs by more than <see cref="CellTolerance"/>, so animation in a corner or a moved focus changes a few
/// cells while another screen changes many.
/// </summary>
internal sealed class ScreenSignature
{
    public const int Columns = 48;
    public const int Rows = 27;
    public const int Cells = Columns * Rows;

    /// <summary>Luminance levels (of 255) two cells may differ by and still count as the same.</summary>
    public const int CellTolerance = 16;

    /// <summary>Pixels between two read in a cell, along each axis: a cell of a 2560x1440 frame is about 53 pixels wide.</summary>
    private const int Stride = 4;

    private const int Channels = 4;
    private const int Green = 1;
    private const int Blue = 2;

    /// <summary>Rec. 709 luma weights in 1/256ths: 54 R + 183 G + 19 B.</summary>
    private const int RedWeight = 54;
    private const int GreenWeight = 183;
    private const int BlueWeight = 19;
    private const int WeightShift = 8;

    private readonly byte[] _cells;

    private ScreenSignature(byte[] cells)
    {
        _cells = cells;
    }

    /// <summary>The signature of an RGBA8 image.</summary>
    public static ScreenSignature Of(ReadOnlySpan<byte> rgba, int width, int height)
    {
        var sums = new long[Cells];
        var counts = new int[Cells];

        for (int y = 0; y < height; y += Stride)
        {
            int row = y * Rows / height * Columns;

            for (int x = 0; x < width; x += Stride)
            {
                int at = ((y * width) + x) * Channels;
                int luma = ((rgba[at] * RedWeight) + (rgba[at + Green] * GreenWeight) + (rgba[at + Blue] * BlueWeight)) >> WeightShift;
                int cell = row + (x * Columns / width);
                sums[cell] += luma;
                counts[cell]++;
            }
        }

        var cells = new byte[Cells];

        for (int i = 0; i < Cells; i++)
        {
            cells[i] = (byte)(sums[i] / Math.Max(1, counts[i]));
        }

        return new ScreenSignature(cells);
    }

    /// <summary>Reads the text form (<see cref="ToString"/>); null when it is not one.</summary>
    public static ScreenSignature? Parse(string text)
    {
        var cells = new byte[Cells];

        if (!Convert.TryFromBase64String(text, cells, out int written) || written != Cells)
        {
            return null;
        }

        return new ScreenSignature(cells);
    }

    /// <summary>How many cells differ by more than <see cref="CellTolerance"/>.</summary>
    public int Distance(ScreenSignature other)
    {
        int changed = 0;

        for (int i = 0; i < Cells; i++)
        {
            if (Math.Abs(_cells[i] - other._cells[i]) > CellTolerance)
            {
                changed++;
            }
        }

        return changed;
    }

    /// <summary>The cells in base64.</summary>
    public override string ToString()
    {
        return Convert.ToBase64String(_cells);
    }
}
