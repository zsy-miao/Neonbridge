// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// One 32-bit register: kind 's' (SGPR) or 'v' (VGPR) and its number. Keys order SGPRs before VGPRs, then by number,
/// which is the order of the generated variable declarations.
/// </summary>
public readonly record struct RegisterKey(char Kind, int Index) : IComparable<RegisterKey>
{
    /// <summary>The SGPR kind.</summary>
    public const char Scalar = 's';

    /// <summary>The VGPR kind.</summary>
    public const char Vector = 'v';

    /// <summary>The HLSL variable that holds this register.</summary>
    public string Name
    {
        get
        {
            if (Kind == Scalar)
            {
                var special = SpecialRegisters.VariableName(Index);

                if (special is not null)
                {
                    return special;
                }
            }

            return Kind + Index.ToString(CultureInfo.InvariantCulture);
        }
    }

    /// <summary>The name of the per-lane bool that holds this register when it is a lane mask.</summary>
    public string MaskName => "m_" + Name;

    public static RegisterKey Sgpr(int index)
    {
        return new RegisterKey(Scalar, index);
    }

    public static RegisterKey Vgpr(int index)
    {
        return new RegisterKey(Vector, index);
    }

    /// <summary>The register <paramref name="offset"/> places further.</summary>
    public RegisterKey Plus(int offset)
    {
        return this with { Index = Index + offset };
    }

    public int CompareTo(RegisterKey other)
    {
        var byKind = Kind.CompareTo(other.Kind);

        if (byKind != 0)
        {
            return byKind;
        }

        return Index.CompareTo(other.Index);
    }

    public static bool operator <(RegisterKey left, RegisterKey right)
    {
        return left.CompareTo(right) < 0;
    }

    public static bool operator >(RegisterKey left, RegisterKey right)
    {
        return left.CompareTo(right) > 0;
    }

    public static bool operator <=(RegisterKey left, RegisterKey right)
    {
        return left.CompareTo(right) <= 0;
    }

    public static bool operator >=(RegisterKey left, RegisterKey right)
    {
        return left.CompareTo(right) >= 0;
    }
}
