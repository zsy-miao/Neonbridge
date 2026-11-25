// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>
/// No constructor subscribes to an event (a <c>+=</c> of a handler, or onto another object's member) or draws a random
/// number; subscriptions happen in an ordered attach step.
/// </summary>
public sealed class ConstructorSubscriptionRule : SyntaxRule
{
    private static readonly FrozenSet<string> RandomDraws =
        new[] { "Next", "NextDouble", "NextSingle", "NextInt64", "NextBytes", "Shuffle", "GetItems" }.ToFrozenSet(StringComparer.Ordinal);

    public override string Name => "ConstructorSubscription";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var constructor in source.Root.DescendantNodes().OfType<ConstructorDeclarationSyntax>())
        {
            var methods = MethodNames(constructor);
            var nodes = constructor.DescendantNodes().ToArray();

            foreach (var assignment in nodes.OfType<AssignmentExpressionSyntax>().Where(node => IsSubscription(node, methods)))
            {
                yield return source.Violation(Name, assignment, $"the constructor subscribes: `{assignment}`");
            }

            foreach (var invocation in nodes.OfType<InvocationExpressionSyntax>().Where(IsRandomDraw))
            {
                yield return source.Violation(Name, invocation, $"the constructor draws a random number: `{invocation}`");
            }
        }
    }

    private static FrozenSet<string> MethodNames(ConstructorDeclarationSyntax constructor)
    {
        var type = constructor.Parent as TypeDeclarationSyntax;

        if (type is null)
        {
            return FrozenSet<string>.Empty;
        }

        return type.Members.OfType<MethodDeclarationSyntax>().Select(method => method.Identifier.Text).ToFrozenSet(StringComparer.Ordinal);
    }

    private static bool IsSubscription(AssignmentExpressionSyntax assignment, FrozenSet<string> methods)
    {
        if (!assignment.IsKind(SyntaxKind.AddAssignmentExpression))
        {
            return false;
        }

        if (assignment.Left is MemberAccessExpressionSyntax target && target.Expression is not ThisExpressionSyntax)
        {
            return true;
        }

        var handler = assignment.Right;

        if (handler is AnonymousFunctionExpressionSyntax)
        {
            return true;
        }

        return handler is IdentifierNameSyntax name && methods.Contains(name.Identifier.Text);
    }

    private static bool IsRandomDraw(InvocationExpressionSyntax invocation) =>
        invocation.Expression is MemberAccessExpressionSyntax access && RandomDraws.Contains(access.Name.Identifier.Text);
}
