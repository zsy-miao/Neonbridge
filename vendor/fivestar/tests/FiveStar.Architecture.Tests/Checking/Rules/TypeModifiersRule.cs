// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>Classes are sealed, abstract or static; a type is partial only for a source generator of the policy.</summary>
public sealed class TypeModifiersRule : SyntaxRule
{
    public override string Name => "TypeModifiers";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var type in source.Root.DescendantNodes().OfType<TypeDeclarationSyntax>())
        {
            if (TypeDeclarations.IsClass(type) && !IsClosed(type.Modifiers) && !HasMemberAttribute(type, CodePolicy.UnsealedFrameworkAttributes))
            {
                yield return source.Violation(Name, type.Identifier, $"class `{type.Identifier.Text}` is not sealed, abstract or static");
            }

            if (type.Modifiers.Any(SyntaxKind.PartialKeyword) && !HasGeneratorAttribute(type))
            {
                yield return source.Violation(Name, type.Identifier, $"`{type.Identifier.Text}` is partial without a source generator attribute");
            }
        }
    }

    private static bool IsClosed(SyntaxTokenList modifiers) =>
        modifiers.Any(SyntaxKind.SealedKeyword)
        || modifiers.Any(SyntaxKind.AbstractKeyword)
        || modifiers.Any(SyntaxKind.StaticKeyword);

    /// <summary>A member of the type carries one of the attributes.</summary>
    private static bool HasMemberAttribute(TypeDeclarationSyntax type, IReadOnlySet<string> names)
    {
        return type.Members
            .SelectMany(member => member.AttributeLists)
            .SelectMany(list => list.Attributes)
            .Any(attribute => names.Contains(TypeDeclarations.AttributeName(attribute)));
    }

    /// <summary>A generator attribute on the type itself or on one of its members.</summary>
    private static bool HasGeneratorAttribute(TypeDeclarationSyntax type)
    {
        var lists = type.AttributeLists.Concat(type.Members.SelectMany(member => member.AttributeLists));

        return lists
            .SelectMany(list => list.Attributes)
            .Any(attribute => CodePolicy.GeneratorAttributes.Contains(TypeDeclarations.AttributeName(attribute)));
    }
}
