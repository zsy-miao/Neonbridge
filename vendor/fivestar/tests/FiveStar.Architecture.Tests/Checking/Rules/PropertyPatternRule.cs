// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>No property patterns (<c>x is { } y</c>, <c>x is not { }</c>, <c>x is { Count: 0 }</c>); type, constant and relational patterns stay.</summary>
public sealed class PropertyPatternRule : SyntaxRule
{
    public override string Name => "PropertyPattern";

    protected override IEnumerable<Violation> Check(CSharpSource source) =>
        source.Root
            .DescendantNodes()
            .OfType<RecursivePatternSyntax>()
            .Where(pattern => pattern.PropertyPatternClause is not null)
            .Select(pattern => source.Violation(Name, pattern, $"property pattern `{pattern}`; put the value in a local and test it plainly"));
}
