// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Numerics;
using System.Text;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The ds_swizzle_b32 offset (AMD "RDNA 2 Instruction Set Architecture", DS_SWIZZLE_B32): bit 15 with bits [14:8]
/// clear selects quad permute (four 2-bit lane selects); bit 15 clear selects the bit-mask mode, AND [4:0], OR [9:5],
/// XOR [14:10], printed as the named special cases SWAP, REVERSE and BROADCAST where they apply.
/// </summary>
internal static class Swizzle
{
    private const int QuadPermute = 0x8000;
    private const int QuadPermuteMask = 0xFF00;
    private const int BitMaskModeMask = 0x8000;
    private const int MaskWidth = 5;
    private const int OrLow = 5;
    private const int XorLow = 10;
    private const int FullMask = 0x1F;
    private const int Lanes = 4;
    private const int LaneSelectWidth = 2;

    public static string Text(int offset)
    {
        var word = (uint)offset;

        if ((offset & QuadPermuteMask) == QuadPermute)
        {
            var lanes = Enumerable.Range(0, Lanes).Select(lane => Bits.Field(word, lane * LaneSelectWidth, LaneSelectWidth));

            return "swizzle(QUAD_PERM," + string.Join(',', lanes) + ")";
        }

        if ((offset & BitMaskModeMask) != 0)
        {
            return offset.ToString(CultureInfo.InvariantCulture);
        }

        var and = Bits.Field(word, 0, MaskWidth);
        var or = Bits.Field(word, OrLow, MaskWidth);
        var xor = Bits.Field(word, XorLow, MaskWidth);

        return BitMask(and, or, xor);
    }

    private static string BitMask(int and, int or, int xor)
    {
        if (and == FullMask && or == 0 && BitOperations.PopCount((uint)xor) == 1)
        {
            return string.Create(CultureInfo.InvariantCulture, $"swizzle(SWAP,{xor})");
        }

        if (and == FullMask && or == 0 && xor > 0 && BitOperations.IsPow2(xor + 1))
        {
            return string.Create(CultureInfo.InvariantCulture, $"swizzle(REVERSE,{xor + 1})");
        }

        var groupSize = FullMask - and + 1;

        if (groupSize > 1 && BitOperations.IsPow2(groupSize) && or < groupSize && xor == 0)
        {
            return string.Create(CultureInfo.InvariantCulture, $"swizzle(BROADCAST,{groupSize},{or})");
        }

        var probe0 = or ^ xor;
        var probe1 = ((FullMask & and) | or) ^ xor;
        var text = new StringBuilder("swizzle(BITMASK_PERM,\"");

        for (var mask = 1 << (MaskWidth - 1); mask > 0; mask >>= 1)
        {
            text.Append(Lane(probe0 & mask, probe1 & mask));
        }

        return text.Append("\")").ToString();
    }

    private static char Lane(int bit0, int bit1)
    {
        if (bit0 == bit1)
        {
            if (bit0 == 0)
            {
                return '0';
            }

            return '1';
        }

        if (bit0 == 0)
        {
            return 'p';
        }

        return 'i';
    }
}
