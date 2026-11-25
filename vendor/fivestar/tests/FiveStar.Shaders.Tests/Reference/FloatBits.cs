// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>Conversions between numbers and the f32 / f16 bit patterns the GPU registers hold.</summary>
public static class FloatBits
{
    public const uint Infinity = 0x7F800000;
    public const uint NegativeInfinity = 0xFF800000;
    public const uint QuietNan = 0x7FC00000;
    public const uint SignalingNan = 0x7F800001;

    /// <summary>f32 bits of a number, rounded to nearest even (overflow gives infinity, NaN the quiet NaN).</summary>
    public static uint Of(double value)
    {
        if (double.IsNaN(value))
        {
            return QuietNan;
        }

        return BitConverter.SingleToUInt32Bits((float)value);
    }

    /// <summary>f32 bits of an exact value, rounded once.</summary>
    public static uint Of(Rational value)
    {
        return Of(FloatRounding.ToSingle(value));
    }

    public static double Value(uint bits)
    {
        return BitConverter.UInt32BitsToSingle(bits);
    }

    /// <summary>The value of f16 bits.</summary>
    public static double HalfValue(uint bits)
    {
        return (double)BitConverter.UInt16BitsToHalf((ushort)(bits & 0xFFFF));
    }

    /// <summary>f16 bits of a number: round to nearest even, or toward zero as v_cvt_pkrtz does.</summary>
    public static uint Half(double value, bool towardZero = false)
    {
        if (double.IsNaN(value))
        {
            return 0x7E00;
        }

        if (double.IsInfinity(value))
        {
            return WithSign(value, 0x7C00);
        }

        var bits = Half(Rational.From(value), towardZero);

        if (bits == 0 && double.IsNegative(value))
        {
            return 0x8000;
        }

        return bits;
    }

    /// <summary>f16 bits of an exact value.</summary>
    public static uint Half(Rational value, bool towardZero = false)
    {
        var rounded = FloatRounding.ToHalf(value, towardZero);

        if (double.IsInfinity(rounded))
        {
            return WithSign(rounded, 0x7C00);
        }

        if (rounded == 0)
        {
            return 0;
        }

        return BitConverter.HalfToUInt16Bits((Half)rounded);
    }

    /// <summary>Two f16 halves in one dword: low in bits 0..15, high in 16..31.</summary>
    public static uint Pack(uint low, uint high)
    {
        return (low & 0xFFFF) | ((high & 0xFFFF) << 16);
    }

    public static Rational Exact(uint singleBits)
    {
        return Rational.From(Value(singleBits));
    }

    public static Rational ExactHalf(uint halfBits)
    {
        return Rational.From(HalfValue(halfBits));
    }

    private static uint WithSign(double value, uint magnitude)
    {
        if (value < 0)
        {
            return magnitude | 0x8000;
        }

        return magnitude;
    }
}
