// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>A conditional expression only as the whole value of one simple assignment or initializer, never nested.</summary>
public sealed class ConditionalExpressionRule : SyntaxRule
{
    public override string Name => "ConditionalExpression";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var conditional in source.Root.DescendantNodes().OfType<ConditionalExpressionSyntax>())
        {
            if (conditional.Ancestors().OfType<ConditionalExpressionSyntax>().Any())
            {
                yield return source.Violation(Name, conditional, "a conditional expression is nested in another; write an explicit if/else");
            }
            else if (!IsSimpleAssignment(conditional))
            {
                yield return source.Violation(Name, conditional, "a conditional expression outside one simple assignment; write an explicit if/else");
            }
        }
    }

    private static bool IsSimpleAssignment(ConditionalExpressionSyntax conditional)
    {
        var value = Unwrap(conditional);
        var parent = value.Parent;

        if (parent is EqualsValueClauseSyntax)
        {
            return parent.Parent is VariableDeclaratorSyntax || parent.Parent is PropertyDeclarationSyntax;
        }

        if (parent is AssignmentExpressionSyntax assignment)
        {
            return assignment.IsKind(SyntaxKind.SimpleAssignmentExpression)
                && assignment.Right == value
                && assignment.Parent is ExpressionStatementSyntax;
        }

        return false;
    }

    private static SyntaxNode Unwrap(SyntaxNode expression)
    {
        var current = expression;

        while (current.Parent is ParenthesizedExpressionSyntax)
        {
            current = current.Parent;
        }

        return current;
    }
}
