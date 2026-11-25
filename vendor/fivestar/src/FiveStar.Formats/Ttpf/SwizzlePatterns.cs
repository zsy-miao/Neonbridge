// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// The tile swizzles of the console's texture layouts, per tile size and element size. The 4 KB patterns are a
/// 256-byte micro tile followed by <c>yxyx</c>, the 64 KB patterns a different micro order followed by
/// <c>yxyxyxyx</c>; tile shapes match the D3D standard tile shapes. Found by block-continuity search on real textures.
/// </summary>
public static class SwizzlePatterns
{
    /// <summary>Bytes of a 4 KB tile (layout bases 1, 2 and 3).</summary>
    public const int SmallTile = 0x1000;

    /// <summary>Bytes of a 64 KB tile (layout base 0).</summary>
    public const int LargeTile = 0x10000;

    private const int OneByte = 1;
    private const int TwoBytes = 2;
    private const int FourBytes = 4;
    private const int EightBytes = 8;
    private const int SixteenBytes = 16;

    /// <summary>The pattern bits for a tile size and element size, or null when there is none.</summary>
    public static string? Find(int tileBytes, int bytesPerElement)
    {
        if (tileBytes == SmallTile)
        {
            return SmallTilePattern(bytesPerElement);
        }

        if (tileBytes == LargeTile)
        {
            return LargeTilePattern(bytesPerElement);
        }

        return null;
    }

    /// <summary>The 4 KB brick of a volume (bits of x, y and z), or null when none is known for the element size.</summary>
    public static string? FindVolume(int bytesPerElement)
    {
        switch (bytesPerElement)
        {
            case TwoBytes:
                return "xzyzyxzyxzy";
            case EightBytes:
                return "xzyxzyxzy";
            default:
                return null;
        }
    }

    private static string? SmallTilePattern(int bytesPerElement)
    {
        switch (bytesPerElement)
        {
            case OneByte:
                return "xxxxyyyyyxyx";
            case TwoBytes:
                return "xxxyyyxyxyx";
            case FourBytes:
                return "xxyyyxyxyx";
            case EightBytes:
                return "xyyxxyxyx";
            case SixteenBytes:
                return "yyxxyxyx";
            default:
                return null;
        }
    }

    private static string? LargeTilePattern(int bytesPerElement)
    {
        switch (bytesPerElement)
        {
            case OneByte:
                return "xxxy1y0yxyyxyxyxyx";
            case TwoBytes:
                return "xxxyyyxyxyxyxyx";
            case FourBytes:
                return "xxyyxyyxyxyxyx";
            case EightBytes:
                return "xyxxyyxyxyxyx";
            case SixteenBytes:
                return "xyxyyxyxyxyx";
            default:
                return null;
        }
    }
}
