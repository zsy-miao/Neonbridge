// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// A parsed instruction operand. Registers carry their first and last number (<see cref="Low"/>, <see cref="High"/>);
/// immediates their 32-bit value; expressions their HLSL text. <see cref="Negate"/> and <see cref="Absolute"/> are the
/// VOP3 input modifiers.
/// </summary>
public sealed record Operand
{
    public required OperandKind Kind { get; init; }

    public required string Text { get; init; }

    public bool Negate { get; init; }

    public bool Absolute { get; init; }

    public int Low { get; init; }

    public int High { get; init; }

    public uint Value { get; init; }

    public string Expression { get; init; } = string.Empty;

    /// <summary>True for SGPR and VGPR operands.</summary>
    public bool IsRegister => Kind is OperandKind.Scalar or OperandKind.Vector;

    /// <summary>True for an immediate.</summary>
    public bool IsImmediate => Kind == OperandKind.Immediate;

    /// <summary>The first register; null for immediates and expressions.</summary>
    public RegisterKey? Key
    {
        get
        {
            if (!IsRegister)
            {
                return null;
            }

            return new RegisterKey(KindLetter, Low);
        }
    }

    /// <summary>The number of registers in the range.</summary>
    public int Width => High - Low + 1;

    private char KindLetter
    {
        get
        {
            var letter = Kind == OperandKind.Scalar ? RegisterKey.Scalar : RegisterKey.Vector;

            return letter;
        }
    }

    /// <summary>The register <paramref name="component"/> places after the first.</summary>
    public RegisterKey At(int component)
    {
        return new RegisterKey(KindLetter, Low + component);
    }

    public static Operand Immediate(uint value, string text)
    {
        return new Operand { Kind = OperandKind.Immediate, Text = text, Value = value };
    }

    public static Operand FromExpression(string expression)
    {
        return new Operand { Kind = OperandKind.Expression, Text = expression, Expression = expression };
    }

    public static Operand Register(RegisterKey key)
    {
        var kind = key.Kind == RegisterKey.Scalar ? OperandKind.Scalar : OperandKind.Vector;

        return new Operand { Kind = kind, Text = key.Name, Low = key.Index, High = key.Index };
    }

    /// <summary>A copy of this operand's input modifiers on another operand.</summary>
    public Operand WithModifiersOf(Operand source)
    {
        return this with { Negate = source.Negate, Absolute = source.Absolute };
    }
}
