// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// What is known about one HLSL variable of a translated program on a hardware wave that holds part of the console wave.
/// <paramref name="Exact"/>: the bits that equal the console's value (on every lane of the window).
/// <paramref name="Held"/>: the bits that equal the console's when the window holds an active lane (EXEC only shrank
/// since: a lane active now was active then); a superset of <paramref name="Exact"/>.
/// <paramref name="Uniform"/>: the bits that are equal on every active lane of the console wave.
/// <paramref name="AllUniform"/>: the bits that are equal on every lane of the console wave.
/// <paramref name="Kind"/>: for a lane mask, how it relates to <c>active</c>.
/// <paramref name="NonEmpty"/>: a lane mask with at least one lane set in this hardware wave.
/// </summary>
internal readonly record struct ValueFact(uint Exact, uint Held, uint Uniform, uint AllUniform, MaskKind Kind, bool NonEmpty, long? Constant)
{
    public const uint AllBits = uint.MaxValue;

    /// <summary>A value known only to be what the console computes.</summary>
    public static ValueFact ExactValue => new(AllBits, AllBits, 0, 0, MaskKind.Other, false, null);

    /// <summary>A value that may differ from the console's.</summary>
    public static ValueFact Unknown => new(0, 0, 0, 0, MaskKind.Other, false, null);

    /// <summary>A value equal everywhere and to the console's (a scalar register, a literal).</summary>
    public static ValueFact Scalar => new(AllBits, AllBits, AllBits, AllBits, MaskKind.Uniform, false, null);

    /// <summary>A lane mask with every lane set (EXEC after <c>s_mov exec, -1</c>, or a copy of it).</summary>
    public bool Full { get; init; }

    /// <summary>A lane mask as an integer (a ballot) whose bit for each lane of the window is the console's.</summary>
    public bool OwnBits { get; init; }

    public bool IsExact => Exact == AllBits;

    /// <summary>Each window lane's own bit is the console's.</summary>
    public bool HasOwnBits => OwnBits || IsExact;

    /// <summary>The value is the console's when the window holds an active lane.</summary>
    public bool IsHeld => Held == AllBits;

    /// <summary>The value on every active lane of the console wave is the same.</summary>
    public bool IsActiveUniform => Uniform == AllBits;

    /// <summary>What holds on both of two paths that meet.</summary>
    public static ValueFact Merge(ValueFact first, ValueFact second)
    {
        long? constant = null;

        if (first.Constant == second.Constant)
        {
            constant = first.Constant;
        }

        var exact = first.Exact & second.Exact;
        var held = first.Held & second.Held;
        var uniform = first.Uniform & second.Uniform;
        var allUniform = first.AllUniform & second.AllUniform;
        var nonEmpty = first.NonEmpty && second.NonEmpty;
        var kind = MergeKinds(first.Kind, second.Kind);

        return new ValueFact(exact, held, uniform, allUniform, kind, nonEmpty, constant)
        {
            Full = first.Full && second.Full, OwnBits = first.HasOwnBits && second.HasOwnBits,
        };
    }

    private static MaskKind MergeKinds(MaskKind first, MaskKind second)
    {
        if (first == second)
        {
            return first;
        }

        if (IsSubset(first) && IsSubset(second))
        {
            return MaskKind.SubActive;
        }

        return MaskKind.Other;
    }

    /// <summary>A mask that never holds a lane outside active.</summary>
    public static bool IsSubset(MaskKind kind)
    {
        return kind is MaskKind.ActiveAnd or MaskKind.SubActive;
    }
}
