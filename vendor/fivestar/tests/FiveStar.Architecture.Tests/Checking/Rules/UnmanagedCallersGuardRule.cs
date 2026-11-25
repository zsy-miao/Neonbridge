// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>
/// An <c>[UnmanagedCallersOnly]</c> method that calls, creates or indexes anything is one try statement that catches
/// <c>Exception</c>: an exception that leaves such a method crosses into native frames and ends the process without a
/// managed stack. Methods that do none of these (return a constant, read a pointer) cannot throw and need no guard.
/// Checked in the product (src); test and benchmark callbacks are left out.
/// </summary>
public sealed class UnmanagedCallersGuardRule : SyntaxRule
{
    private const string Attribute = "UnmanagedCallersOnly";

    public override string Name => "UnmanagedCallersGuard";

    protected override bool Applies(CSharpSource source) =>
        source.File.Path.Replace('\\', '/').StartsWith("src/", StringComparison.Ordinal);

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var method in source.Root.DescendantNodes().OfType<MethodDeclarationSyntax>())
        {
            if (IsEntryPoint(method) && CanThrow(method) && !IsGuarded(method))
            {
                yield return source.Violation(Name, method.Identifier, $"[UnmanagedCallersOnly] method {method.Identifier.Text} can throw: make its body one try statement that catches Exception");
            }
        }
    }

    private static bool IsEntryPoint(MethodDeclarationSyntax method) =>
        method.AttributeLists.SelectMany(list => list.Attributes).Any(attribute => TypeDeclarations.AttributeName(attribute) == Attribute);

    private static bool CanThrow(MethodDeclarationSyntax method)
    {
        SyntaxNode? body = (SyntaxNode?)method.Body ?? method.ExpressionBody;

        return body is not null && body.DescendantNodes().Any(node =>
            node is InvocationExpressionSyntax or BaseObjectCreationExpressionSyntax or ElementAccessExpressionSyntax);
    }

    private static bool IsGuarded(MethodDeclarationSyntax method)
    {
        var body = method.Body;

        if (body is null || body.Statements.Count != 1 || body.Statements[0] is not TryStatementSyntax attempt)
        {
            return false;
        }

        return attempt.Catches.Any(clause => clause.Declaration is null || clause.Declaration.Type.ToString() is "Exception" or "System.Exception");
    }
}
