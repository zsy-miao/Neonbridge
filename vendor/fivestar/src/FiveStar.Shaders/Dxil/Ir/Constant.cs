// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>
/// A constant; DXIL keeps every constant at module level. Integers hold their value sign-extended from their width,
/// floating-point constants their bits.
/// </summary>
public sealed class Constant : Value
{
    internal Constant(IrType type, ConstantKind kind, long bits, IReadOnlyList<Constant> elements)
        : base(type)
    {
        Kind = kind;
        Bits = bits;
        Elements = elements;
    }

    public ConstantKind Kind { get; }

    public long Bits { get; }

    public IReadOnlyList<Constant> Elements { get; }

    /// <summary>The integer value zero-extended from the type's width.</summary>
    public ulong Unsigned
    {
        get
        {
            if (Type.Bits >= sizeof(ulong) * BitsPerByte)
            {
                return (ulong)Bits;
            }

            return (ulong)Bits & ((1UL << Type.Bits) - 1);
        }
    }

    public bool IsInteger => Kind == ConstantKind.Integer;

    private const int BitsPerByte = 8;

    public override string ToString()
    {
        return $"{Type} {Kind} {Bits}";
    }
}
