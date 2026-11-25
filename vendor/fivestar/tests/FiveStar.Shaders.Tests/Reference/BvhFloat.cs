// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// The f32 arithmetic of the BVH intersection model: each operation is one IEEE round-to-nearest-even of the exact
/// result (double then f32; the double rounding is harmless for + - * / of f32 operands since 53 >= 2 * 24 + 2).
/// </summary>
public static class BvhFloat
{
    public static double Round(double value)
    {
        return (float)value;
    }

    public static double Add(double a, double b)
    {
        return Round(a + b);
    }

    public static double Subtract(double a, double b)
    {
        return Round(a - b);
    }

    public static double Multiply(double a, double b)
    {
        return Round(a * b);
    }

    /// <summary>Correctly rounded division; x / 0 is a signed infinity, 0 / 0 and NaN / 0 are NaN.</summary>
    public static double Divide(double a, double b)
    {
        if (b == 0.0)
        {
            if (a == 0.0 || double.IsNaN(a))
            {
                return double.NaN;
            }

            return Math.CopySign(double.PositiveInfinity, a) * Math.CopySign(1.0, b);
        }

        if (double.IsInfinity(a) && double.IsInfinity(b))
        {
            return double.NaN;
        }

        return Round(a / b);
    }

    /// <summary>IEEE maxNum: a NaN operand is ignored.</summary>
    public static double MaxNumber(double a, double b)
    {
        if (double.IsNaN(a))
        {
            return b;
        }

        if (double.IsNaN(b) || a >= b)
        {
            return a;
        }

        return b;
    }

    /// <summary>IEEE minNum: a NaN operand is ignored.</summary>
    public static double MinNumber(double a, double b)
    {
        if (double.IsNaN(a))
        {
            return b;
        }

        if (double.IsNaN(b) || a <= b)
        {
            return a;
        }

        return b;
    }

    /// <summary>f32 bits of a value (NaN as the quiet NaN).</summary>
    public static uint Bits(double value)
    {
        return FloatBits.Of(value);
    }
}
