// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// Exact rounding of a rational to a binary float format (IEEE 754): round to nearest even, or toward zero (which
/// never overflows to infinity). Results are doubles that hold the rounded value exactly, or an infinity.
/// </summary>
public static class FloatRounding
{
    /// <summary>One f32 rounding (24-bit significand, exponents -126..127).</summary>
    public static double ToSingle(Rational value)
    {
        return Round(value, 24, -126, 127, towardZero: false);
    }

    /// <summary>One f16 rounding (11-bit significand, exponents -14..15).</summary>
    public static double ToHalf(Rational value, bool towardZero)
    {
        return Round(value, 11, -14, 15, towardZero);
    }

    private static double Round(Rational value, int significandBits, int minExponent, int maxExponent, bool towardZero)
    {
        if (value.Sign == 0)
        {
            return 0.0;
        }

        var sign = value.Sign;
        var magnitude = value.Abs();
        var exponent = Math.Max(Exponent(magnitude), minExponent);
        var scale = Rational.Power(exponent - (significandBits - 1));

        var scaled = magnitude / scale;
        var whole = scaled.Floor();
        var rest = scaled - new Rational(whole, 1);

        if (!towardZero)
        {
            var half = new Rational(1, 2);

            if (rest > half || (rest.CompareTo(half) == 0 && !whole.IsEven))
            {
                whole += 1;
            }
        }

        var rounded = whole * scale;
        var limit = Rational.Power(maxExponent + 1);

        if (rounded >= limit)
        {
            if (!towardZero)
            {
                return sign * double.PositiveInfinity;
            }

            rounded = limit - Rational.Power(maxExponent + 1 - significandBits);
        }

        return sign * rounded.ToDouble();
    }

    /// <summary>floor(log2(x)) of a positive rational.</summary>
    private static int Exponent(Rational magnitude)
    {
        var exponent = (int)(magnitude.Numerator.GetBitLength() - magnitude.Denominator.GetBitLength());

        while (Rational.Power(exponent) > magnitude)
        {
            exponent--;
        }

        while (Rational.Power(exponent + 1) <= magnitude)
        {
            exponent++;
        }

        return exponent;
    }
}
