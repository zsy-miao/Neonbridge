// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>An exact rational number, so the reference model rounds each ISA result exactly once.</summary>
public readonly record struct Rational
{
    public Rational(BigInteger numerator, BigInteger denominator)
    {
        if (denominator.IsZero)
        {
            throw new DivideByZeroException();
        }

        if (denominator.Sign < 0)
        {
            numerator = -numerator;
            denominator = -denominator;
        }

        var divisor = BigInteger.GreatestCommonDivisor(numerator, denominator);

        if (!divisor.IsZero && !divisor.IsOne)
        {
            numerator /= divisor;
            denominator /= divisor;
        }

        Numerator = numerator;
        Denominator = denominator;
    }

    public BigInteger Numerator { get; }

    public BigInteger Denominator { get; }

    public int Sign => Numerator.Sign;

    public static Rational Zero => new(0, 1);

    /// <summary>The exact value of a finite double.</summary>
    public static Rational From(double value)
    {
        if (!double.IsFinite(value))
        {
            throw new ArgumentOutOfRangeException(nameof(value), "not finite");
        }

        var bits = BitConverter.DoubleToInt64Bits(value);
        var exponent = (int)((bits >> 52) & 0x7FF);
        var mantissa = bits & 0xFFFFFFFFFFFFFL;

        if (exponent == 0)
        {
            exponent = 1;
        }
        else
        {
            mantissa |= 1L << 52;
        }

        var signed = bits < 0 ? -new BigInteger(mantissa) : new BigInteger(mantissa);

        return signed * Power(exponent - 1075);
    }

    public static Rational From(long value)
    {
        return new Rational(value, 1);
    }

    /// <summary>2 to the power <paramref name="exponent"/>.</summary>
    public static Rational Power(int exponent)
    {
        if (exponent >= 0)
        {
            return new Rational(BigInteger.One << exponent, 1);
        }

        return new Rational(1, BigInteger.One << -exponent);
    }

    public static Rational operator +(Rational a, Rational b)
    {
        return new Rational((a.Numerator * b.Denominator) + (b.Numerator * a.Denominator), a.Denominator * b.Denominator);
    }

    public static Rational operator -(Rational a, Rational b)
    {
        return a + (-b);
    }

    public static Rational operator -(Rational a)
    {
        return new Rational(-a.Numerator, a.Denominator);
    }

    public static Rational operator *(Rational a, Rational b)
    {
        return new Rational(a.Numerator * b.Numerator, a.Denominator * b.Denominator);
    }

    public static Rational operator *(BigInteger a, Rational b)
    {
        return new Rational(a * b.Numerator, b.Denominator);
    }

    public static Rational operator /(Rational a, Rational b)
    {
        return new Rational(a.Numerator * b.Denominator, a.Denominator * b.Numerator);
    }

    public static bool operator <(Rational a, Rational b)
    {
        return a.CompareTo(b) < 0;
    }

    public static bool operator >(Rational a, Rational b)
    {
        return a.CompareTo(b) > 0;
    }

    public static bool operator <=(Rational a, Rational b)
    {
        return a.CompareTo(b) <= 0;
    }

    public static bool operator >=(Rational a, Rational b)
    {
        return a.CompareTo(b) >= 0;
    }

    public int CompareTo(Rational other)
    {
        return (Numerator * other.Denominator).CompareTo(other.Numerator * Denominator);
    }

    public Rational Abs()
    {
        return new Rational(BigInteger.Abs(Numerator), Denominator);
    }

    /// <summary>The largest integer not above the value.</summary>
    public BigInteger Floor()
    {
        var quotient = BigInteger.DivRem(Numerator, Denominator, out var remainder);

        if (remainder.Sign < 0)
        {
            quotient -= 1;
        }

        return quotient;
    }

    /// <summary>The value as a double; exact when it is representable.</summary>
    public double ToDouble()
    {
        return (double)Numerator / (double)Denominator;
    }
}
