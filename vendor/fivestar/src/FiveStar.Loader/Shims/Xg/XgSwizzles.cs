// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// The block swizzles of the console's 4 KB and 64 KB swizzle modes per element size, as element-index bits from the
/// least significant (<c>x</c> = next x bit, <c>y</c> = next y bit). They are the patterns found on the shipped
/// textures (docs/textures.md); the block shapes are the D3D standard tile shapes. A 256-byte block uses the first
/// bits of the 4 KB pattern.
/// </summary>
internal static class XgSwizzles
{
    public const int LargeBlockBytes = 0x10000;
    public const int SmallBlockBytes = 0x1000;
    public const int MicroBlockBytes = 0x100;

    /// <summary>Element sizes with a pattern: 1, 2, 4, 8 and 16 bytes (index = log2 of the size).</summary>
    public const int ElementSizes = 5;

    /// <summary>log2 of a 1-byte element: its 64 KB block puts y bit 1 below y bit 0 (docs/textures.md).</summary>
    private const int OneByteSize = 0;

    private static readonly string[] SmallPatterns = ["xxxxyyyyyxyx", "xxxyyyxyxyx", "xxyyyxyxyx", "xyyxxyxyx", "yyxxyxyx"];
    private static readonly string[] LargePatterns = ["xxxyyyxyyxyxyxyx", "xxxyyyxyxyxyxyx", "xxyyxyyxyxyxyx", "xyxxyyxyxyxyx", "xyxyyxyxyxyx"];

    /// <summary>The axes in the order extra brick bits are taken.</summary>
    private const string AxisLetters = "xzy";

    /// <summary>log2 of the 4 KB brick per element size: 16x16x16, 8x16x16, 8x16x8, 8x8x8, 4x8x8 (x, y, z).</summary>
    private static readonly int[] BrickXLog2 = [4, 3, 3, 3, 2];
    private static readonly int[] BrickYLog2 = [4, 4, 4, 3, 3];
    private static readonly int[] BrickZLog2 = [4, 4, 3, 3, 3];

    /// <summary>log2 growth from a 4 KB to a 64 KB brick: the RDNA 1 KB brick times (4, 4, 4) against (1, 2, 2).</summary>
    private const int LargeGrowthX = 2;
    private const int LargeGrowthYZ = 1;

    private static readonly XgSwizzle[] Small = Parse(SmallPatterns);
    private static readonly XgSwizzle[] Large = WithLowYSwapped(Parse(LargePatterns), OneByteSize);
    private static readonly XgSwizzle[] SmallVolumes = Volumes(false);
    private static readonly XgSwizzle[] LargeVolumes = Volumes(true);

    /// <summary>The swizzle of a block size (bytes) and element size (log2 of bytes); the 256-byte block is a prefix.</summary>
    [HotPath]
    public static XgSwizzle Find(int blockBytes, int elementLog2)
    {
        if (blockBytes == LargeBlockBytes)
        {
            return Large[elementLog2];
        }

        var small = Small[elementLog2];

        if (blockBytes == SmallBlockBytes)
        {
            return small;
        }

        int microBits = small.Bits - (System.Numerics.BitOperations.Log2(SmallBlockBytes) - System.Numerics.BitOperations.Log2(MicroBlockBytes));

        return new XgSwizzle(small.XMask & ((1u << microBits) - 1), microBits);
    }

    /// <summary>
    /// The brick of a thick (3D) block size and element size: the 4 KB bricks found on the shipped volumes (2- and
    /// 8-byte elements, docs/textures.md), other sizes and 64 KB bricks extended to the RDNA brick shape by taking x, z
    /// and y bits in turn (not checked on data).
    /// </summary>
    [HotPath]
    public static XgSwizzle FindVolume(int blockBytes, int elementLog2)
    {
        if (blockBytes == LargeBlockBytes)
        {
            return LargeVolumes[elementLog2];
        }

        return SmallVolumes[elementLog2];
    }

    private static XgSwizzle[] Volumes(bool large)
    {
        var swizzles = new XgSwizzle[ElementSizes];

        for (int size = 0; size < ElementSizes; size++)
        {
            swizzles[size] = Volume(large, size);
        }

        return swizzles;
    }

    private static XgSwizzle Volume(bool large, int elementLog2)
    {
        int x = BrickXLog2[elementLog2];
        int y = BrickYLog2[elementLog2];
        int z = BrickZLog2[elementLog2];

        if (large)
        {
            x += LargeGrowthX;
            y += LargeGrowthYZ;
            z += LargeGrowthYZ;
        }

        string known = FiveStar.Formats.Ttpf.SwizzlePatterns.FindVolume(1 << elementLog2) ?? string.Empty;
        string pattern = Extend(known, [x, z, y]);
        uint xMask = 0;
        uint zMask = 0;

        for (int bit = 0; bit < pattern.Length; bit++)
        {
            if (pattern[bit] == 'x')
            {
                xMask |= 1u << bit;
            }
            else if (pattern[bit] == 'z')
            {
                zMask |= 1u << bit;
            }
        }

        return new XgSwizzle(xMask, pattern.Length, false, zMask);
    }

    /// <summary>
    /// Appends x, z and y bits in turn (the order of <see cref="AxisLetters"/>), each while its axis has fewer bits
    /// than <paramref name="wanted"/> gives for it.
    /// </summary>
    private static string Extend(string pattern, int[] wanted)
    {
        var text = new System.Text.StringBuilder(pattern);
        int[] have = [.. AxisLetters.Select(letter => pattern.Count(c => c == letter))];
        bool added = true;

        while (added)
        {
            added = false;

            for (int axis = 0; axis < wanted.Length; axis++)
            {
                if (have[axis] < wanted[axis])
                {
                    text.Append(AxisLetters[axis]);
                    have[axis]++;
                    added = true;
                }
            }
        }

        return text.ToString();
    }

    private static XgSwizzle[] WithLowYSwapped(XgSwizzle[] swizzles, int size)
    {
        swizzles[size] = new XgSwizzle(swizzles[size].XMask, swizzles[size].Bits, true);

        return swizzles;
    }

    private static XgSwizzle[] Parse(string[] patterns)
    {
        var swizzles = new XgSwizzle[patterns.Length];

        for (int size = 0; size < patterns.Length; size++)
        {
            string bits = patterns[size];
            uint xMask = 0;

            for (int bit = 0; bit < bits.Length; bit++)
            {
                if (bits[bit] == 'x')
                {
                    xMask |= 1u << bit;
                }
            }

            swizzles[size] = new XgSwizzle(xMask, bits.Length);
        }

        return swizzles;
    }
}
