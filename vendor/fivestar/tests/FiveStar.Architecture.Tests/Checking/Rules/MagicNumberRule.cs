// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>
/// Numeric literals other than 0 and 1 only where a value gets its name: a <c>const</c>, a <c>static readonly</c> field,
/// an enum member or an attribute argument.
/// </summary>
public sealed class MagicNumberRule : SyntaxRule
{
    public override string Name => "MagicNumber";

    protected override bool Applies(CSharpSource source) =>
        !CodePolicy.Matches(source.Project?.Name, CodePolicy.MagicNumberExemptProjects);

    protected override IEnumerable<Violation> Check(CSharpSource source) =>
        source.Root
            .DescendantNodes()
            .OfType<LiteralExpressionSyntax>()
            .Where(literal => literal.IsKind(SyntaxKind.NumericLiteralExpression))
            .Where(literal => !IsZeroOrOne(literal.Token.Value) && !IsNamed(literal))
            .Select(literal => source.Violation(Name, literal, $"magic number {literal.Token.Text}; give it a name"));

    private static bool IsZeroOrOne(object? value)
    {
        if (value is null)
        {
            return false;
        }

        var number = Convert.ToDouble(value, CultureInfo.InvariantCulture);

        return number == 0 || number == 1;
    }

    private static bool IsNamed(SyntaxNode literal) =>
        literal.Ancestors().Any(ancestor =>
            ancestor is AttributeArgumentSyntax
            || ancestor is EnumMemberDeclarationSyntax
            || IsConstantField(ancestor)
            || IsConstantLocal(ancestor));

    private static bool IsConstantField(SyntaxNode node)
    {
        if (node is not FieldDeclarationSyntax field)
        {
            return false;
        }

        var modifiers = field.Modifiers;

        return modifiers.Any(SyntaxKind.ConstKeyword)
            || (modifiers.Any(SyntaxKind.StaticKeyword) && modifiers.Any(SyntaxKind.ReadOnlyKeyword));
    }

    private static bool IsConstantLocal(SyntaxNode node) =>
        node is LocalDeclarationStatementSyntax local && local.Modifiers.Any(SyntaxKind.ConstKeyword);
}
