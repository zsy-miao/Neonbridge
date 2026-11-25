// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// One statement of a translated block as the window analysis sees it: its shape, the variables it reads and assigns,
/// the functions it calls and the effects it has.
/// </summary>
internal sealed partial class WindowStatement
{
    // A brace and its space at either end of a compound statement split over lines.
    private const string Brace = "{ ";

    private static readonly FrozenSet<string> Keywords = new[]
    {
        "uint", "int", "float", "bool", "half", "true", "false", "if", "else", "for", "while", "return", "break",
        "uint2", "uint3", "uint4", "int2", "int3", "int4", "float2", "float3", "float4", "uint64_t", "int64_t",
    }.ToFrozenSet(StringComparer.Ordinal);

    private WindowStatement(StatementKind kind, string text)
    {
        Kind = kind;
        Text = text;
    }

    public StatementKind Kind { get; private init; }

    public string Text { get; }

    /// <summary>The assigned variable of an <see cref="StatementKind.Assign"/> or <see cref="StatementKind.Guarded"/>.</summary>
    public string Target { get; private init; } = string.Empty;

    /// <summary>The assigned expression of an <see cref="StatementKind.Assign"/> or <see cref="StatementKind.Guarded"/>.</summary>
    public string Value { get; private init; } = string.Empty;

    /// <summary>The target is a per-lane bool (a lane mask).</summary>
    public bool IsMask { get; private init; }

    /// <summary>Every variable the statement reads (a guarded write also reads its target and <c>active</c>).</summary>
    public IReadOnlyList<string> Reads { get; private init; } = [];

    /// <summary>Variables an <see cref="StatementKind.Other"/> or <see cref="StatementKind.Effect"/> statement may assign.</summary>
    public IReadOnlyList<string> Assigned { get; private init; } = [];

    /// <summary>The functions the statement calls.</summary>
    public IReadOnlyList<string> Calls { get; private init; } = [];

    /// <summary>Runs only on active lanes (<c>if (active) ...</c>).</summary>
    public bool IsGuarded { get; private init; }

    /// <summary>Writes a mesh output (vertex, primitive or staged export).</summary>
    public bool IsExport { get; private init; }

    /// <summary>GS_ALLOC_REQ: the first lane stores the output counts in groupshared memory.</summary>
    public bool IsAllocation { get; private init; }

    /// <summary>A group barrier.</summary>
    public bool IsBarrier { get; private init; }

    /// <summary>Reads or writes memory other threads see without a barrier: LDS, or a store or atomic.</summary>
    public bool IsShared { get; private init; }

    public static WindowStatement Parse(string line)
    {
        var text = Normalize(line);

        if (text.StartsWith("pc = ", StringComparison.Ordinal) || text.StartsWith("pc_resume = ", StringComparison.Ordinal) || text.Length == 0)
        {
            return new WindowStatement(StatementKind.Control, text);
        }

        var effect = EffectPattern().IsMatch(text) || AllocationPattern().IsMatch(text);
        var assignment = AssignPattern().Match(text);
        var guarded = GuardedPattern().Match(text);

        if (!guarded.Success)
        {
            // v = select(active, value, v): the same as if (active) v = value.
            guarded = SelectPattern().Match(text);
            assignment = guarded.Success ? Match.Empty : assignment;
        }

        if (!effect && assignment.Success && !assignment.Groups["value"].Value.Contains(';', StringComparison.Ordinal))
        {
            return Assignment(StatementKind.Assign, text, assignment);
        }

        if (!effect && guarded.Success && !guarded.Groups["value"].Value.Contains(';', StringComparison.Ordinal))
        {
            return Assignment(StatementKind.Guarded, text, guarded);
        }

        var kind = StatementKind.Other;

        if (effect)
        {
            kind = StatementKind.Effect;
        }

        var assigned = TargetPattern().Matches(text).Select(match => match.Groups[1].Value).Where(name => !Keywords.Contains(name));

        if (OutParameterPattern().IsMatch(text))
        {
            // GetDimensions writes its arguments.
            assigned = assigned.Concat(Names(text));
        }

        return new WindowStatement(kind, text)
        {
            Reads = Names(text),
            Calls = Functions(text),
            Assigned = [.. assigned.Distinct()],
            IsGuarded = text.StartsWith("if (active) ", StringComparison.Ordinal),
            IsExport = ExportPattern().IsMatch(text),
            IsAllocation = AllocationPattern().IsMatch(text),
            IsBarrier = text.Contains("GroupMemoryBarrier", StringComparison.Ordinal),
            IsShared = SharedPattern().IsMatch(text),
        };
    }

    /// <summary>The variables an expression reads: names that are not functions, members or keywords.</summary>
    public static IReadOnlyList<string> Names(string expression)
    {
        return [.. NamePattern().Matches(expression).Select(match => match.Groups[1].Value).Where(name => !Keywords.Contains(name)).Distinct()];
    }

    /// <summary>The functions an expression calls.</summary>
    public static IReadOnlyList<string> Functions(string expression)
    {
        return [.. CallPattern().Matches(expression).Select(match => match.Groups[1].Value).Distinct()];
    }

    private static WindowStatement Assignment(StatementKind kind, string text, Match match)
    {
        var target = match.Groups["target"].Value;
        var value = match.Groups["value"].Value;
        var reads = Names(value).ToList();

        if (kind == StatementKind.Guarded)
        {
            reads.AddRange([target, WindowNames.Active]);
        }

        var isMask = match.Groups["type"].Value == "bool" || WindowNames.IsMask(target);

        return new WindowStatement(kind, text)
        {
            Target = target, Value = value, IsMask = isMask, Reads = reads.Distinct().ToList(), Calls = Functions(value),
            IsGuarded = kind == StatementKind.Guarded,
        };
    }

    /// <summary>The statement without the braces of a compound statement split over lines.</summary>
    private static string Normalize(string line)
    {
        var text = line.Trim();

        if (text.StartsWith("{ ", StringComparison.Ordinal) && !text.Contains('}', StringComparison.Ordinal))
        {
            text = text[Brace.Length..].Trim();
        }

        if (text.EndsWith(" }", StringComparison.Ordinal) && !text.Contains('{', StringComparison.Ordinal))
        {
            text = text[..^Brace.Length].Trim();
        }

        return text;
    }

    [GeneratedRegex(@"^(?:(?<type>bool|u?int[234]?|float[234]?|u?int64_t)\s+)?(?<target>[A-Za-z_]\w*)\s*=\s*(?<value>[^=].*);$")]
    private static partial Regex AssignPattern();

    [GeneratedRegex(@"^if \(active\) (?<target>[A-Za-z_]\w*)\s*=\s*(?<value>[^=].*);$")]
    private static partial Regex GuardedPattern();

    [GeneratedRegex(@"^(?<target>[A-Za-z_]\w*) = select\(active, (?<value>.+), \k<target>\);$")]
    private static partial Regex SelectPattern();

    [GeneratedRegex(@"\b\w*(?:Store|Atomic|Interlocked|Append|RestartStrip|Barrier|SetMeshOutputCounts)\w*\s*\(|\bdiscard\b|/\*MSEXP|(?:\]|\.\w+)\s*=(?!=)|\bg_\w+\s*=(?!=)")]
    private static partial Regex EffectPattern();

    [GeneratedRegex(@"/\*MSEXP|\b(?:tris|prims|verts|g_ms_exp|g_ms_prim)\[")]
    private static partial Regex ExportPattern();

    [GeneratedRegex(@"^if \(WaveIsFirstLane\(\)\) \{ g_ms_nv = ")]
    private static partial Regex AllocationPattern();

    [GeneratedRegex(@"\bg_lds\[|\b\w*(?:Store|Atomic|Interlocked)\w*\s*\(")]
    private static partial Regex SharedPattern();

    [GeneratedRegex(@"(?<![\w.])([A-Za-z_]\w*)\b(?!\s*\()")]
    private static partial Regex NamePattern();

    [GeneratedRegex(@"(?<![\w.])([A-Za-z_]\w*)\s*\(")]
    private static partial Regex CallPattern();

    [GeneratedRegex(@"(?<![\w.\]])([A-Za-z_]\w*)\s*=(?!=)")]
    private static partial Regex TargetPattern();

    [GeneratedRegex(@"\.GetDimensions\(")]
    private static partial Regex OutParameterPattern();
}
