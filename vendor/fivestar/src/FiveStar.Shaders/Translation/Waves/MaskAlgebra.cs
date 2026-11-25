// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// The <see cref="MaskKind"/> of a translated lane-mask expression (<c>&amp;&amp;</c>, <c>||</c>, <c>!</c>,
/// <c>!= true</c>, <c>c ? a : false</c> over masks, <c>active</c> and per-lane comparisons), and whether an expression is
/// zero on lanes where every mask of a set is zero.
/// </summary>
internal static class MaskAlgebra
{
    private const string Or = "||";
    private const string And = "&&";

    public static MaskKind KindOf(string expression, WindowState state)
    {
        var text = BoolSyntax.Unwrap(expression);
        var either = BoolSyntax.Split(text, Or);

        if (either.Count > 1)
        {
            return either.Select(part => KindOf(part, state)).Aggregate(Union);
        }

        var both = BoolSyntax.Split(text, And);

        if (both.Count > 1)
        {
            return Conjunction(both, state);
        }

        var negated = BoolSyntax.Negated(text);

        if (negated is not null)
        {
            return Complement(KindOf(negated, state));
        }

        var choice = BoolSyntax.Choice(text);

        if (choice is not null)
        {
            return ChoiceKind(choice.Value, state);
        }

        return LeafKind(text, state);
    }

    /// <summary>Whether the expression is false on every lane where all masks in <paramref name="zero"/> are false.</summary>
    public static bool IsZero(string expression, IReadOnlySet<string> zero)
    {
        var text = BoolSyntax.Unwrap(expression);
        var either = BoolSyntax.Split(text, Or);

        if (either.Count > 1)
        {
            return either.All(part => IsZero(part, zero));
        }

        var both = BoolSyntax.Split(text, And);

        if (both.Count > 1)
        {
            return both.Any(part => IsZero(part, zero));
        }

        var choice = BoolSyntax.Choice(text);

        if (choice is not null)
        {
            return IsZero(choice.Value.WhenTrue, zero) && IsZero(choice.Value.WhenFalse, zero);
        }

        var difference = BoolSyntax.Difference(text);

        if (difference is not null && difference.Value.Right != "true")
        {
            return IsZero(difference.Value.Left, zero) && IsZero(difference.Value.Right, zero);
        }

        var bit = ValueRules.LaneBitPattern().Match(text);

        if (bit.Success)
        {
            // This lane's bit of an integer mask whose bits are zero on these lanes.
            return zero.Contains(bit.Groups["lo"].Value) && zero.Contains(bit.Groups["hi"].Value);
        }

        return text == "false" || zero.Contains(text);
    }

    /// <summary>
    /// Whether the expression has the same value on every active lane of the console wave (a mask that was active and
    /// a uniform value still is after EXEC shrank).
    /// </summary>
    public static bool IsActiveUniform(string expression, WindowState state)
    {
        var text = BoolSyntax.Unwrap(expression);
        var parts = BoolSyntax.Split(text, Or).SelectMany(part => BoolSyntax.Split(part, And)).ToList();

        if (parts.Count > 1)
        {
            return parts.All(part => IsActiveUniform(part, state));
        }

        var negated = BoolSyntax.Negated(text);

        if (negated is not null)
        {
            return IsActiveUniform(negated, state);
        }

        var choice = BoolSyntax.Choice(text);

        if (choice is not null)
        {
            var (condition, whenTrue, whenFalse) = choice.Value;

            return IsActiveUniform(condition, state) && IsActiveUniform(whenTrue, state) && IsActiveUniform(whenFalse, state);
        }

        if (text == WindowNames.Active)
        {
            return true;
        }

        var names = WindowStatement.Names(text);
        var calls = WindowStatement.Functions(text);
        var varies = calls.Any(call => WindowNames.IsLaneFunction(call) || WindowNames.IsWindowFunction(call)) || names.Any(WindowNames.IsLaneInput);

        return !varies && names.All(name => name == WindowNames.Active || state.Get(name).IsActiveUniform);
    }

    /// <summary>A conjunction with a part inside active and all other parts equal on active lanes is active and a uniform value.</summary>
    private static MaskKind Conjunction(IReadOnlyList<string> parts, WindowState state)
    {
        var kinds = parts.Select(part => KindOf(part, state)).ToList();

        for (var index = 0; index < parts.Count; index++)
        {
            var others = parts.Where((_, other) => other != index);

            if (kinds[index] == MaskKind.ActiveAnd && others.All(part => IsActiveUniform(part, state)))
            {
                return MaskKind.ActiveAnd;
            }
        }

        var kind = kinds.Aggregate(Intersection);

        if (kind == MaskKind.Other && parts.Contains(WindowNames.Active))
        {
            return MaskKind.SubActive;
        }

        return kind;
    }

    private static MaskKind ChoiceKind((string Condition, string WhenTrue, string WhenFalse) choice, WindowState state)
    {
        if (!WindowStatement.Names(choice.Condition).All(WindowNames.IsScalar))
        {
            return MaskKind.Other;
        }

        if (choice.WhenFalse == "false")
        {
            return Intersection(MaskKind.Uniform, KindOf(choice.WhenTrue, state));
        }

        var whenTrue = KindOf(choice.WhenTrue, state);

        if (whenTrue == KindOf(choice.WhenFalse, state))
        {
            return whenTrue;
        }

        return MaskKind.Other;
    }

    private static MaskKind LeafKind(string text, WindowState state)
    {
        if (text is "true" or "false")
        {
            return MaskKind.Uniform;
        }

        if (WindowNames.IsMask(text) && state.Get(text).Full)
        {
            return MaskKind.Uniform;
        }

        if (WindowNames.IsMask(text))
        {
            return state.Get(text).Kind;
        }

        var difference = BoolSyntax.Difference(text);

        if (difference is not null && difference.Value.Right == "true")
        {
            return Complement(KindOf(difference.Value.Left, state));
        }

        if (difference is not null && WindowNames.IsMask(difference.Value.Left) && WindowNames.IsMask(difference.Value.Right))
        {
            return Difference(KindOf(difference.Value.Left, state), KindOf(difference.Value.Right, state));
        }

        var scalar = WindowStatement.Names(text).All(WindowNames.IsScalar) && !WindowStatement.Functions(text).Any(WindowNames.IsLaneFunction);

        if (scalar)
        {
            return MaskKind.Uniform;
        }

        return MaskKind.Other;
    }

    /// <summary>Exclusive or: (active and U) xor (active and V) is active and (U xor V).</summary>
    private static MaskKind Difference(MaskKind first, MaskKind second)
    {
        if (first == second && first is MaskKind.Uniform or MaskKind.ActiveAnd)
        {
            return first;
        }

        if (ValueFact.IsSubset(first) && ValueFact.IsSubset(second))
        {
            return MaskKind.SubActive;
        }

        return MaskKind.Other;
    }

    private static MaskKind Intersection(MaskKind first, MaskKind second)
    {
        if (first == MaskKind.Uniform && second == MaskKind.Uniform)
        {
            return MaskKind.Uniform;
        }

        if (IsActiveAndWith(first, second) || IsActiveAndWith(second, first))
        {
            return MaskKind.ActiveAnd;
        }

        if (ValueFact.IsSubset(first) || ValueFact.IsSubset(second))
        {
            return MaskKind.SubActive;
        }

        if (first == MaskKind.NotActiveAnd && second == MaskKind.NotActiveAnd)
        {
            return MaskKind.NotActiveAnd;
        }

        return MaskKind.Other;
    }

    private static bool IsActiveAndWith(MaskKind kind, MaskKind other)
    {
        return kind == MaskKind.ActiveAnd && other is MaskKind.Uniform or MaskKind.ActiveAnd or MaskKind.NotActiveAnd;
    }

    private static MaskKind Union(MaskKind first, MaskKind second)
    {
        if (first == second && first is MaskKind.Uniform or MaskKind.ActiveAnd or MaskKind.NotActiveAnd or MaskKind.SubActive)
        {
            return first;
        }

        if (ValueFact.IsSubset(first) && ValueFact.IsSubset(second))
        {
            return MaskKind.SubActive;
        }

        var notActive = first == MaskKind.NotActiveAnd || second == MaskKind.NotActiveAnd;

        if (notActive && first is MaskKind.NotActiveAnd or MaskKind.Uniform && second is MaskKind.NotActiveAnd or MaskKind.Uniform)
        {
            return MaskKind.NotActiveAnd;
        }

        return MaskKind.Other;
    }

    private static MaskKind Complement(MaskKind kind)
    {
        switch (kind)
        {
            case MaskKind.Uniform:
                return MaskKind.Uniform;
            case MaskKind.ActiveAnd:
                return MaskKind.NotActiveAnd;
            case MaskKind.NotActiveAnd:
                return MaskKind.ActiveAnd;
            default:
                return MaskKind.Other;
        }
    }
}
