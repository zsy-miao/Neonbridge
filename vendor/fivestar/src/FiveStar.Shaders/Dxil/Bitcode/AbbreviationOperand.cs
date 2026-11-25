// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Bitcode;

/// <summary>
/// One operand of an abbreviation (LLVM BitCodes.h BitCodeAbbrevOp): a literal value the record does not store, a
/// fixed-width or VBR field of <see cref="Value"/> bits, or an array whose elements use the next operand.
/// </summary>
public readonly record struct AbbreviationOperand(AbbreviationKind Kind, ulong Value)
{
    public static AbbreviationOperand Literal(ulong value)
    {
        return new AbbreviationOperand(AbbreviationKind.Literal, value);
    }

    public static AbbreviationOperand Fixed(int bits)
    {
        return new AbbreviationOperand(AbbreviationKind.Fixed, (ulong)bits);
    }

    public static AbbreviationOperand Vbr(int bits)
    {
        return new AbbreviationOperand(AbbreviationKind.Vbr, (ulong)bits);
    }

    public static AbbreviationOperand Array()
    {
        return new AbbreviationOperand(AbbreviationKind.Array, 0);
    }
}
