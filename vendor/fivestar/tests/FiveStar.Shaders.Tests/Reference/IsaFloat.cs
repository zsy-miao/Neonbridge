// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// RDNA float VALU semantics, written from the AMD RDNA ISA pseudo-code (and LLVM's constant folders where the manual
/// is terse), never from the translator. Results are f32 bits or integers.
/// </summary>
public static class IsaFloat
{
    /// <summary>V_MUL_LEGACY_F32: DX9 rules, 0 times anything (infinity and NaN included) is 0.</summary>
    public static uint MulLegacy(double a, double b)
    {
        if (a == 0.0 || b == 0.0)
        {
            return FloatBits.Of(0.0);
        }

        return FloatBits.Of(a * b);
    }

    /// <summary>V_FMA_F32: one rounding of the exact a * b + c.</summary>
    public static uint Fma(double a, double b, double c)
    {
        return FloatBits.Of((Rational.From(a) * Rational.From(b)) + Rational.From(c));
    }

    /// <summary>V_MAD_F32 / V_MAC_F32: not fused, the product is rounded to f32 first.</summary>
    public static uint Mad(double a, double b, double c)
    {
        var product = FloatRounding.ToSingle(Rational.From(a) * Rational.From(b));

        return FloatBits.Of(Rational.From(product) + Rational.From(c));
    }

    /// <summary>V_CVT_U32_F32: truncate; NaN gives 0; out of range saturates (negatives give 0).</summary>
    public static uint ConvertToUInt(double value)
    {
        if (double.IsNaN(value) || value <= 0)
        {
            return 0;
        }

        if (value >= 4294967296.0)
        {
            return uint.MaxValue;
        }

        return (uint)Math.Truncate(value);
    }

    /// <summary>V_CVT_I32_F32: truncate toward zero; NaN gives 0; out of range saturates.</summary>
    public static uint ConvertToInt(double value)
    {
        if (double.IsNaN(value))
        {
            return 0;
        }

        if (value >= 2147483648.0)
        {
            return 0x7FFFFFFF;
        }

        if (value <= -2147483648.0)
        {
            return 0x80000000;
        }

        return (uint)(int)Math.Truncate(value);
    }

    /// <summary>V_CVT_FLR_I32_F32: floor, then as V_CVT_I32_F32.</summary>
    public static uint ConvertFloorToInt(double value)
    {
        var floor = double.IsFinite(value) ? Math.Floor(value) : value;

        return ConvertToInt(floor);
    }

    /// <summary>V_CVT_RPI_I32_F32: floor(x + 0.5), then as V_CVT_I32_F32.</summary>
    public static uint ConvertRoundToInt(double value)
    {
        var rounded = double.IsFinite(value) ? Math.Floor(value + 0.5) : value;

        return ConvertToInt(rounded);
    }

    /// <summary>V_FRACT_F32: x - floor(x), clamped to the largest float below 1.0 (0x3F7FFFFF).</summary>
    public static uint Fract(double value)
    {
        var exact = Rational.From(value) - new Rational(Rational.From(value).Floor(), 1);
        var bits = FloatBits.Of(exact);

        if (bits < 0x80000000)
        {
            return Math.Min(bits, 0x3F7FFFFFu);
        }

        return bits;
    }

    /// <summary>V_RNDNE_F32: round half to even; a result of zero keeps the sign of the input.</summary>
    public static uint RoundNearestEven(double value)
    {
        var rounded = Math.Round(value, MidpointRounding.ToEven);

        if (rounded == 0)
        {
            return FloatBits.Of(Math.CopySign(0.0, value));
        }

        return FloatBits.Of(rounded);
    }

    /// <summary>V_LDEXP_F32: x * 2^e, rounded once (exact for finite results).</summary>
    public static uint Ldexp(double value, int exponent)
    {
        return FloatBits.Of(Rational.From(value) * Rational.Power(exponent));
    }

    public static double Median(double a, double b, double c)
    {
        return Math.Max(Math.Min(a, b), Math.Min(Math.Max(a, b), c));
    }

    public static long Median(long a, long b, long c)
    {
        return Math.Max(Math.Min(a, b), Math.Min(Math.Max(a, b), c));
    }

    /// <summary>V_CMP_CLASS_F32: bit 0 sNaN, 1 qNaN, 2 -inf, 3 -normal, 4 -denormal, 5 -0, 6 +0, 7 +denormal, 8 +normal, 9 +inf.</summary>
    public static uint CompareClass(uint bits, uint mask)
    {
        var negative = (bits >> 31) != 0;
        var exponent = (bits >> 23) & 0xFF;
        var fraction = bits & 0x7FFFFF;
        int kind;

        if (exponent == 0xFF && fraction != 0)
        {
            kind = (fraction & 0x400000) != 0 ? 1 : 0;
        }
        else if (exponent == 0xFF)
        {
            kind = negative ? 2 : 9;
        }
        else if (exponent == 0 && fraction == 0)
        {
            kind = negative ? 5 : 6;
        }
        else if (exponent == 0)
        {
            kind = negative ? 4 : 7;
        }
        else
        {
            kind = negative ? 3 : 8;
        }

        return (mask >> kind) & 1;
    }
}
