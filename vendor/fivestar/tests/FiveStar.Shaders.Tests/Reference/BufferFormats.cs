// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// BUF_DATA_FORMAT / BUF_NUM_FORMAT decoding of buffer_load_format (GCN numbering). AMD names list components from the
/// MSB, so 2_10_10_10 holds x in bits 9..0 (DXGI R10G10B10A2), 10_10_10_2 holds x in bits 1..0 and 10_11_11 holds x in
/// bits 10..0 (DXGI R11G11B10) (cf. Mesa si_translate_buffer_dataformat). Missing components read (0, 0, 0, 1). The
/// V# dword 3 of a format: DST_SEL_X..W in bits 0-11 (4..7 select x..w), the unified format in bits 12-18 and
/// RESOURCE_LEVEL (bit 24, 1 on gfx10) (RDNA 2 ISA, buffer resource).
/// </summary>
public static class BufferFormats
{
    public const int Unorm = 0;
    public const int Snorm = 1;
    public const int UnsignedInt = 4;
    public const int SignedInt = 5;
    public const int FloatingPoint = 7;

    private const int SelectX = 4;
    private const int SelectBits = 3;
    private const int FormatShift = 12;
    private const uint ResourceLevel = 1u << 24;
    private const int ExponentBits = 5;
    private const int ExponentBias = 15;

    private static readonly Dictionary<int, int[]> Layouts = new()
    {
        [1] = [8], [2] = [16], [3] = [8, 8], [4] = [32], [5] = [16, 16], [6] = [11, 11, 10], [7] = [10, 11, 11],
        [8] = [2, 10, 10, 10], [9] = [10, 10, 10, 2], [10] = [8, 8, 8, 8], [11] = [32, 32], [12] = [16, 16, 16, 16],
        [13] = [32, 32, 32], [14] = [32, 32, 32, 32],
    };

    /// <summary>The component each result reads: x, y, z, w in memory order.</summary>
    public static int[] Identity { get; } = [0, 1, 2, 3];

    /// <summary>A B8G8R8A8 view: x reads the third byte.</summary>
    public static int[] Bgra { get; } = [2, 1, 0, 3];

    /// <summary>V# dword 3 of unified format <paramref name="unified"/> whose results read <paramref name="components"/>.</summary>
    public static uint Word3(int unified, int[] components)
    {
        var select = 0u;

        for (var index = 0; index < components.Length; index++)
        {
            select |= (uint)(SelectX + components[index]) << (SelectBits * index);
        }

        return select | ((uint)unified << FormatShift) | ResourceLevel;
    }

    /// <summary>The four result dwords of one element whose bytes start at words[0].</summary>
    public static uint[] Decode(uint[] words, int layout, int numeric)
    {
        var joined = BigInteger.Zero;

        for (var index = 0; index < Math.Min(words.Length, 4); index++)
        {
            joined |= new BigInteger(words[index]) << (32 * index);
        }

        var result = new List<uint>();
        var shift = 0;

        foreach (var bits in Layouts[layout])
        {
            var raw = (uint)((joined >> shift) & ((BigInteger.One << bits) - 1));
            result.Add(Component(raw, bits, numeric));
            shift += bits;
        }

        var integer = numeric is UnsignedInt or SignedInt;

        while (result.Count < 3)
        {
            result.Add(0);
        }

        if (result.Count == 3)
        {
            result.Add(One(integer));
        }

        return [.. result];
    }

    private static uint Component(uint raw, int bits, int numeric)
    {
        long signed = raw;

        if (bits < 32 && (raw & (1u << (bits - 1))) != 0)
        {
            signed -= 1L << bits;
        }

        switch (numeric)
        {
            case Unorm:
                return FloatBits.Of(new Rational(raw, (BigInteger.One << bits) - 1));
            case Snorm:
                var snorm = new Rational(signed, (BigInteger.One << (bits - 1)) - 1);
                var clamped = snorm < Rational.From(-1) ? Rational.From(-1) : snorm;

                return FloatBits.Of(clamped);
            case UnsignedInt:
                return raw;
            case SignedInt:
                return unchecked((uint)signed);
            case FloatingPoint:
                return FloatValue(raw, bits);
            default:
                throw new ArgumentOutOfRangeException(nameof(numeric));
        }
    }

    private static uint FloatValue(uint raw, int bits)
    {
        if (bits == 32)
        {
            return raw;
        }

        if (bits == 16)
        {
            return FloatBits.Of(FloatBits.HalfValue(raw));
        }

        // 11- and 10-bit fields: unsigned, 5 exponent bits (bias 15), the rest mantissa.
        var mantissaBits = bits - ExponentBits;
        var exponent = (int)(raw >> mantissaBits);
        var mantissa = raw & ((1u << mantissaBits) - 1);
        var fraction = mantissa / (double)(1u << mantissaBits);

        if (exponent == (1 << ExponentBits) - 1 && mantissa == 0)
        {
            return FloatBits.Infinity;
        }

        if (exponent == (1 << ExponentBits) - 1)
        {
            return FloatBits.QuietNan;
        }

        if (exponent == 0)
        {
            return FloatBits.Of(Math.ScaleB(fraction, 1 - ExponentBias));
        }

        return FloatBits.Of(Math.ScaleB(1 + fraction, exponent - ExponentBias));
    }

    private static uint One(bool integer)
    {
        if (integer)
        {
            return 1;
        }

        return FloatBits.Of(1.0);
    }
}
