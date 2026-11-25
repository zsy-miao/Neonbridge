// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>
/// The wall clock, <c>Stopwatch</c>, <c>Environment</c>, <c>AppContext.BaseDirectory</c> and <c>Random</c> only in the
/// policy's projects; everything else receives time, paths and randomness from its caller.
/// </summary>
public sealed class AmbientStateRule : SyntaxRule
{
    private static readonly FrozenSet<string> AmbientTypes =
        new[] { "Stopwatch", "Environment", "Random" }.ToFrozenSet(StringComparer.Ordinal);

    private static readonly FrozenSet<string> AmbientMembers =
        new[] { "DateTime.Now", "DateTime.UtcNow", "DateTime.Today", "DateTimeOffset.Now", "DateTimeOffset.UtcNow", "AppContext.BaseDirectory" }
            .ToFrozenSet(StringComparer.Ordinal);

    public override string Name => "AmbientState";

    protected override bool Applies(CSharpSource source) =>
        !CodePolicy.Matches(source.Project?.Name, CodePolicy.AmbientStateProjects);

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var node in source.Root.DescendantNodes())
        {
            var found = Ambient(node);

            if (found is not null)
            {
                yield return source.Violation(Name, node, $"`{found}` is ambient state; take it from the caller");
            }
        }
    }

    /// <summary>The ambient type or member this node names, or null.</summary>
    private static string? Ambient(SyntaxNode node)
    {
        if (node is MemberAccessExpressionSyntax access)
        {
            var member = $"{LastName(access.Expression)}.{access.Name.Identifier.Text}";

            if (AmbientMembers.Contains(member))
            {
                return member;
            }
        }

        if (node is IdentifierNameSyntax name && AmbientTypes.Contains(name.Identifier.Text) && !IsOtherMember(name))
        {
            return name.Identifier.Text;
        }

        return null;
    }

    /// <summary>The last identifier of a possibly qualified expression: <c>DateTime</c> for <c>System.DateTime</c>.</summary>
    private static string LastName(ExpressionSyntax expression)
    {
        var text = expression.ToString();

        return text[(text.LastIndexOf('.') + 1)..];
    }

    /// <summary>Whether the name is a member of something other than the <c>System</c> namespaces, such as <c>options.Random</c>.</summary>
    private static bool IsOtherMember(IdentifierNameSyntax name)
    {
        var access = name.Parent as MemberAccessExpressionSyntax;

        if (access is null || access.Name != name)
        {
            return false;
        }

        var qualifier = access.Expression.ToString();

        return qualifier != "System" && qualifier != "System.Diagnostics";
    }
}
