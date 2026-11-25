// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>Splits translated bool expressions at their outermost operators (outside parentheses).</summary>
internal static partial class BoolSyntax
{
    // The parts of a binary operation.
    private const int Operands = 2;

    /// <summary>The expression without parentheses that enclose all of it.</summary>
    public static string Unwrap(string expression)
    {
        var text = expression.Trim();

        while (text.StartsWith('(') && Closing(text, 0) == text.Length - 1)
        {
            text = text[1..^1].Trim();
        }

        return text;
    }

    /// <summary>The operands of <paramref name="operation"/> at the outermost level (one part when it is not there).</summary>
    public static IReadOnlyList<string> Split(string expression, string operation)
    {
        var parts = new List<string>();
        var depth = 0;
        var start = 0;

        for (var index = 0; index < expression.Length; index++)
        {
            depth += Depth(expression[index]);

            if (depth == 0 && string.CompareOrdinal(expression, index, operation, 0, operation.Length) == 0)
            {
                parts.Add(expression[start..index].Trim());
                start = index + operation.Length;
                index = start - 1;
            }
        }

        parts.Add(expression[start..].Trim());

        return parts;
    }

    /// <summary>The operand of a leading <c>!</c> applied to the whole expression, or null.</summary>
    public static string? Negated(string expression)
    {
        if (!expression.StartsWith('!') || expression.StartsWith("!=", StringComparison.Ordinal))
        {
            return null;
        }

        var operand = expression[1..].Trim();

        if (NamePattern().IsMatch(operand) || (operand.StartsWith('(') && Closing(operand, 0) == operand.Length - 1))
        {
            return operand;
        }

        return null;
    }

    /// <summary>The parts of <c>condition ? whenTrue : whenFalse</c> at the outermost level, or null.</summary>
    public static (string Condition, string WhenTrue, string WhenFalse)? Choice(string expression)
    {
        var question = Split(expression, "?");

        if (question.Count != Operands)
        {
            return null;
        }

        var arms = Split(question[1], ":");

        if (arms.Count != Operands)
        {
            return null;
        }

        return (question[0], arms[0], arms[1]);
    }

    /// <summary>The operands of an outermost <c>!=</c>, or null.</summary>
    public static (string Left, string Right)? Difference(string expression)
    {
        var parts = Split(expression, "!=");

        if (parts.Count != Operands)
        {
            return null;
        }

        return (parts[0], parts[1]);
    }

    private static int Depth(char character)
    {
        if (character == '(')
        {
            return 1;
        }

        if (character == ')')
        {
            return -1;
        }

        return 0;
    }

    /// <summary>The index of the parenthesis closing the one at <paramref name="open"/>.</summary>
    private static int Closing(string text, int open)
    {
        var depth = 0;

        for (var index = open; index < text.Length; index++)
        {
            depth += Depth(text[index]);

            if (depth == 0)
            {
                return index;
            }
        }

        return -1;
    }

    [GeneratedRegex(@"^[A-Za-z_]\w*$")]
    private static partial Regex NamePattern();
}
