// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Numerics;
using System.Text;

namespace FiveStar.Formats.Common;

/// <summary>
/// Exact decimal text for doubles: fixed-point with round-half-even on the exact binary value, and the shortest
/// round-trip form (fixed between 1e-4 and 1e16, else scientific with a two-digit exponent). Tool reports print
/// numbers this way so they compare digit for digit across implementations.
/// </summary>
public static class DecimalText
{
    // IEEE 754 binary64 layout.
    private const int MantissaBits = 52;
    private const int ExponentMask = 0x7FF;
    private const int ExponentBias = 1075;
    private const int Radix = 10;

    // Shortest form: fixed notation while the decimal point sits in (-4, 16].
    private const int LowestFixedPoint = -4;
    private const int HighestFixedPoint = 16;
    private const int MinExponentDigits = 2;
    private const int MaxNarrowDecimals = 6;
    private const int NarrowBits = 128;

    /// <summary>The value with exactly <paramref name="decimals"/> digits after the point, rounded half to even.</summary>
    public static string Fixed(double value, int decimals)
    {
        if (double.IsNaN(value))
        {
            return "nan";
        }

        if (double.IsInfinity(value))
        {
            return InfinityText(value);
        }

        var digits = ScaledHalfEven(Math.Abs(value), decimals);
        digits = digits.PadLeft(decimals + 1, '0');
        var text = new StringBuilder();

        if (double.IsNegative(value))
        {
            text.Append('-');
        }

        text.Append(digits, 0, digits.Length - decimals);

        if (decimals > 0)
        {
            text.Append('.').Append(digits, digits.Length - decimals, decimals);
        }

        return text.ToString();
    }

    /// <summary>The double nearest to the value rounded half to even at <paramref name="decimals"/> places.</summary>
    public static double Round(double value, int decimals)
    {
        if (!double.IsFinite(value))
        {
            return value;
        }

        return double.Parse(Fixed(value, decimals), CultureInfo.InvariantCulture);
    }

    /// <summary>The shortest digits that read back as the same double.</summary>
    public static string Shortest(double value)
    {
        if (double.IsNaN(value))
        {
            return "nan";
        }

        if (double.IsInfinity(value))
        {
            return InfinityText(value);
        }

        var (digits, point) = ShortestDigits(Math.Abs(value));
        var sign = double.IsNegative(value) ? "-" : string.Empty;

        if (point > LowestFixedPoint && point <= HighestFixedPoint)
        {
            return sign + FixedForm(digits, point);
        }

        return sign + ScientificForm(digits, point);
    }

    /// <summary>A value as <c>0x</c> and lower-case hex digits.</summary>
    public static string Hex(long value)
    {
        if (value < 0)
        {
            return "-0x" + (-value).ToString("x", CultureInfo.InvariantCulture);
        }

        return "0x" + value.ToString("x", CultureInfo.InvariantCulture);
    }

    private static string InfinityText(double value)
    {
        var text = value > 0 ? "inf" : "-inf";

        return text;
    }

    // The value times 10^decimals rounded half to even, as decimal digits. Small scales of fractions take a 128-bit
    // path: the mantissa (53 bits) times 10^6 fits in 73 bits.
    private static string ScaledHalfEven(double magnitude, int decimals)
    {
        var bits = BitConverter.DoubleToInt64Bits(magnitude);
        var exponent = (int)((bits >> MantissaBits) & ExponentMask);
        var mantissa = bits & ((1L << MantissaBits) - 1);

        if (exponent == 0)
        {
            exponent = 1;
        }
        else
        {
            mantissa |= 1L << MantissaBits;
        }

        exponent -= ExponentBias;

        if (exponent < 0 && decimals <= MaxNarrowDecimals)
        {
            return NarrowHalfEven((UInt128)mantissa * (UInt128)Pow10(decimals), -exponent);
        }

        var scaled = mantissa * BigInteger.Pow(Radix, decimals);

        if (exponent >= 0)
        {
            return (scaled << exponent).ToString(CultureInfo.InvariantCulture);
        }

        var divisor = BigInteger.One << -exponent;
        var quotient = BigInteger.DivRem(scaled, divisor, out var remainder);
        var half = (remainder << 1).CompareTo(divisor);

        if (half > 0 || (half == 0 && !quotient.IsEven))
        {
            quotient += BigInteger.One;
        }

        return quotient.ToString(CultureInfo.InvariantCulture);
    }

    private static string NarrowHalfEven(UInt128 scaled, int shift)
    {
        if (shift >= NarrowBits)
        {
            return "0";
        }

        var divisor = UInt128.One << shift;
        var quotient = scaled >> shift;
        var remainder = scaled & (divisor - 1);
        var twice = remainder << 1;

        if (twice > divisor || (twice == divisor && (quotient & UInt128.One) != 0))
        {
            quotient++;
        }

        return quotient.ToString(CultureInfo.InvariantCulture);
    }

    private static ulong Pow10(int decimals)
    {
        ulong power = 1;

        for (var index = 0; index < decimals; index++)
        {
            power *= Radix;
        }

        return power;
    }

    // Digits without leading or trailing zeros and the decimal point position: value = 0.DIGITS * 10^point.
    private static (string Digits, int Point) ShortestDigits(double magnitude)
    {
        var text = magnitude.ToString("R", CultureInfo.InvariantCulture);
        var exponent = 0;
        var marker = text.IndexOf('E', StringComparison.Ordinal);

        if (marker >= 0)
        {
            exponent = int.Parse(text.AsSpan(marker + 1), NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture);
            text = text[..marker];
        }

        var dot = text.IndexOf('.', StringComparison.Ordinal);
        var integerLength = dot >= 0 ? dot : text.Length;
        var digits = text.Replace(".", string.Empty, StringComparison.Ordinal);
        var point = integerLength + exponent;

        // Leading zeros move the point left; trailing zeros carry nothing.
        var trimmed = digits.TrimStart('0');
        point -= digits.Length - trimmed.Length;
        trimmed = trimmed.TrimEnd('0');

        if (trimmed.Length == 0)
        {
            return ("0", 1);
        }

        return (trimmed, point);
    }

    private static string FixedForm(string digits, int point)
    {
        if (point <= 0)
        {
            return "0." + new string('0', -point) + digits;
        }

        if (point >= digits.Length)
        {
            return digits + new string('0', point - digits.Length) + ".0";
        }

        return digits[..point] + "." + digits[point..];
    }

    private static string ScientificForm(string digits, int point)
    {
        var text = new StringBuilder();
        text.Append(digits[0]);

        if (digits.Length > 1)
        {
            text.Append('.').Append(digits, 1, digits.Length - 1);
        }

        var exponent = point - 1;
        var sign = exponent < 0 ? '-' : '+';
        text.Append('e').Append(sign);
        text.Append(Math.Abs(exponent).ToString(CultureInfo.InvariantCulture).PadLeft(MinExponentDigits, '0'));

        return text.ToString();
    }
}
