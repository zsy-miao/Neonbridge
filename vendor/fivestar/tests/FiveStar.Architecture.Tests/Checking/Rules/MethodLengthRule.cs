// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>Methods, constructors, operators, accessors and local functions span at most a fixed number of lines.</summary>
public sealed class MethodLengthRule : SyntaxRule
{
    public override string Name => "MethodLength";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        var bodies = source.Root
            .DescendantNodes()
            .Where(node => node is BaseMethodDeclarationSyntax || node is AccessorDeclarationSyntax || node is LocalFunctionStatementSyntax);

        foreach (var body in bodies)
        {
            var lines = source.LineIndex(body.Span.End) - source.LineIndex(body.SpanStart) + 1;

            if (lines > CodePolicy.MaxMethodLines)
            {
                yield return source.Violation(Name, body, $"{Describe(body)} spans {lines} lines; at most {CodePolicy.MaxMethodLines}");
            }
        }
    }

    private static string Describe(SyntaxNode body)
    {
        if (body is MethodDeclarationSyntax method)
        {
            return $"method `{method.Identifier.Text}`";
        }

        if (body is LocalFunctionStatementSyntax local)
        {
            return $"local function `{local.Identifier.Text}`";
        }

        if (body is ConstructorDeclarationSyntax)
        {
            return "constructor";
        }

        if (body is AccessorDeclarationSyntax)
        {
            return "accessor";
        }

        return "member";
    }
}
