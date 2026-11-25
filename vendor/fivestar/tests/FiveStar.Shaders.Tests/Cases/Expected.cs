// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// The expected value of one result dword: exact bits, f32 within n ULPs, f32 within an absolute tolerance, any NaN,
/// either of two exact values (a result the HLSL leaves to the device), or don't care (a result the modelled wave cannot
/// define).
/// </summary>
public sealed record Expected
{
    private Expected(ExpectedKind kind, uint bits, double value, double tolerance)
    {
        Kind = kind;
        Bits = bits;
        Value = value;
        Tolerance = tolerance;
    }

    public ExpectedKind Kind { get; }

    public uint Bits { get; }

    public double Value { get; }

    public double Tolerance { get; }

    public static Expected Any { get; } = new(ExpectedKind.Any, 0, 0, 0);

    public static Expected Nan { get; } = new(ExpectedKind.Nan, 0, 0, 0);

    public static Expected Exact(uint bits)
    {
        return new Expected(ExpectedKind.Exact, bits, 0, 0);
    }

    public static Expected Ulp(uint bits, int ulps)
    {
        return new Expected(ExpectedKind.Ulp, bits, 0, ulps);
    }

    public static Expected Near(double value, double tolerance)
    {
        return new Expected(ExpectedKind.Near, 0, value, tolerance);
    }

    /// <summary>
    /// <paramref name="bits"/> or <paramref name="alternative"/>: for a result HLSL leaves to the device, such as mad(),
    /// which AMD and NVIDIA fuse and WARP does not.
    /// </summary>
    public static Expected Either(uint bits, uint alternative)
    {
        return new Expected(ExpectedKind.Either, bits, alternative, 0);
    }

    /// <summary>Exact bits, or don't care for null.</summary>
    public static Expected Maybe(uint? bits)
    {
        if (bits is null)
        {
            return Any;
        }

        return Exact(bits.Value);
    }

    public static implicit operator Expected(uint bits)
    {
        return Exact(bits);
    }

    public static implicit operator Expected(int bits)
    {
        return Exact(unchecked((uint)bits));
    }

    public bool Matches(uint actual)
    {
        switch (Kind)
        {
            case ExpectedKind.Exact:
                return actual == Bits;
            case ExpectedKind.Ulp:
                return UlpMatch(actual);
            case ExpectedKind.Near:
                return Math.Abs(FloatBits.Value(actual) - Value) <= Tolerance;
            case ExpectedKind.Nan:
                return double.IsNaN(FloatBits.Value(actual));
            case ExpectedKind.Either:
                return actual == Bits || actual == (uint)Value;
            default:
                return true;
        }
    }

    public override string ToString()
    {
        switch (Kind)
        {
            case ExpectedKind.Exact:
                return Hex(Bits);
            case ExpectedKind.Ulp:
                return string.Create(CultureInfo.InvariantCulture, $"{Hex(Bits)}(+-{Tolerance}ulp)");
            case ExpectedKind.Near:
                return string.Create(CultureInfo.InvariantCulture, $"~{Value:G7}(+-{Tolerance:G})");
            case ExpectedKind.Nan:
                return "NaN";
            case ExpectedKind.Either:
                return $"{Hex(Bits)} or {Hex((uint)Value)}";
            default:
                return "-";
        }
    }

    public static string Hex(uint bits)
    {
        return string.Create(CultureInfo.InvariantCulture, $"0x{bits:x8}");
    }

    private bool UlpMatch(uint actual)
    {
        if (double.IsNaN(FloatBits.Value(actual)) || double.IsNaN(FloatBits.Value(Bits)))
        {
            return false;
        }

        return Math.Abs(Ordered(actual) - Ordered(Bits)) <= Tolerance;
    }

    /// <summary>f32 bits on a line where adjacent floats differ by one.</summary>
    private static long Ordered(uint bits)
    {
        if ((bits >> 31) == 0)
        {
            return bits & 0x7FFFFFFF;
        }

        return -(long)(bits & 0x7FFFFFFF);
    }
}
