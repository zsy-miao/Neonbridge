// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>RDNA integer VALU / SALU semantics (RDNA ISA pseudo-code, LLVM constant folding for bit-field extracts).</summary>
public static class IsaInteger
{
    /// <summary>The low 24 bits, sign-extended (V_MUL_I32_I24 operands).</summary>
    public static long SignExtend24(uint value)
    {
        var low = value & 0xFFFFFF;

        if ((low & 0x800000) != 0)
        {
            return low - (1L << 24);
        }

        return low;
    }

    public static int AsInt(uint value)
    {
        return unchecked((int)value);
    }

    public static uint BitFieldUnsigned(uint value, int offset, int width)
    {
        offset &= 31;
        width &= 31;

        return (uint)((value >> offset) & ((1UL << width) - 1));
    }

    /// <summary>
    /// V_BFE_I32 / S_BFE_I32: sign-extend the field; when offset + width reaches past bit 31 the field runs off the top
    /// and the result is S0.i >> offset (arithmetic).
    /// </summary>
    public static uint BitFieldSigned(uint value, int offset, int width)
    {
        if (width == 0)
        {
            return 0;
        }

        if (offset + width < 32)
        {
            long field = (value >> offset) & ((1L << width) - 1);

            if ((field & (1L << (width - 1))) != 0)
            {
                field -= 1L << width;
            }

            return unchecked((uint)field);
        }

        return unchecked((uint)(AsInt(value) >> offset));
    }

    /// <summary>S_BFE_U32: offset = S1[4:0], width = S1[22:16].</summary>
    public static uint ScalarBitFieldUnsigned(uint value, uint select)
    {
        var offset = (int)(select & 31);
        var width = (int)((select >> 16) & 0x7F);

        return (uint)((value >> offset) & ((1UL << Math.Min(width, 63)) - 1));
    }

    public static uint ScalarBitFieldSigned(uint value, uint select)
    {
        return BitFieldSigned(value, (int)(select & 31), (int)((select >> 16) & 0x7F));
    }

    public static uint AlignBit(uint high, uint low, int shift)
    {
        var joined = ((ulong)high << 32) | low;

        return (uint)(joined >> (shift & 31));
    }

    /// <summary>V_PERM_B32: byte permute of {S0, S1} by the selector bytes.</summary>
    public static uint Permute(uint source0, uint source1, uint select)
    {
        var data = ((ulong)source0 << 32) | source1;
        uint result = 0;

        for (var index = 0; index < 4; index++)
        {
            var selector = (int)((select >> (8 * index)) & 0xFF);
            result |= PermuteByte(data, selector) << (8 * index);
        }

        return result;
    }

    public static uint FirstBitHigh(uint value)
    {
        if (value == 0)
        {
            return uint.MaxValue;
        }

        return (uint)BitOperations.LeadingZeroCount(value);
    }

    public static uint FirstBitLow(uint value)
    {
        if (value == 0)
        {
            return uint.MaxValue;
        }

        return (uint)BitOperations.TrailingZeroCount(value);
    }

    public static uint Reverse(uint value)
    {
        uint result = 0;

        for (var bit = 0; bit < 32; bit++)
        {
            result |= ((value >> bit) & 1) << (31 - bit);
        }

        return result;
    }

    private static uint PermuteByte(ulong data, int selector)
    {
        switch (selector)
        {
            case >= 13:
                return 0xFF;
            case 12:
                return 0x00;
            case 11:
                return SignByte(data, 63);
            case 10:
                return SignByte(data, 47);
            case 9:
                return SignByte(data, 31);
            case 8:
                return SignByte(data, 15);
            default:
                return (uint)((data >> (8 * selector)) & 0xFF);
        }
    }

    private static uint SignByte(ulong data, int bit)
    {
        if (((data >> bit) & 1) != 0)
        {
            return 0xFF;
        }

        return 0;
    }
}
