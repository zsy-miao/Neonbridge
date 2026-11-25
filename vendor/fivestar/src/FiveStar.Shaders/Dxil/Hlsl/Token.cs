// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>
/// One token: its text, the line it is on and whether it starts that line (preprocessor directives). A number keeps its
/// parsed value and suffix (<c>u</c>, <c>l</c>, <c>f</c>, <c>h</c> or none).
/// </summary>
public readonly record struct Token(TokenKind Kind, string Text, int Line, bool StartsLine)
{
    public ulong Integer { get; init; }

    /// <summary>A real literal's value, kept in the bits of <see cref="Integer"/> (a token is one or the other).</summary>
    public double Real
    {
        get => BitConverter.UInt64BitsToDouble(Integer);
        init => Integer = BitConverter.DoubleToUInt64Bits(value);
    }

    public char Suffix { get; init; }

    /// <summary>The source offset of the first character.</summary>
    public int Offset { get; init; }

    public bool Is(string text)
    {
        return (Kind == TokenKind.Punctuator || Kind == TokenKind.Identifier) && Text == text;
    }

    public override string ToString()
    {
        return Text;
    }
}
