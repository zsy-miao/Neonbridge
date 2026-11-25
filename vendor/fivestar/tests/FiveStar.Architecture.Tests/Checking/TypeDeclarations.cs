// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>Questions the rules ask about type declarations and type names.</summary>
public static class TypeDeclarations
{
    private const string AttributeSuffix = "Attribute";

    /// <summary>The types declared directly in a file or its namespaces, not nested in another type.</summary>
    public static IReadOnlyList<MemberDeclarationSyntax> TopLevel(CompilationUnitSyntax root) =>
        root.DescendantNodes(node => node is CompilationUnitSyntax || node is BaseNamespaceDeclarationSyntax)
            .OfType<MemberDeclarationSyntax>()
            .Where(member => member is BaseTypeDeclarationSyntax || member is DelegateDeclarationSyntax)
            .ToArray();

    public static string NameOf(MemberDeclarationSyntax type)
    {
        if (type is DelegateDeclarationSyntax callback)
        {
            return callback.Identifier.Text;
        }

        return ((BaseTypeDeclarationSyntax)type).Identifier.Text;
    }

    /// <summary>Whether the declaration is a class or a record class.</summary>
    public static bool IsClass(TypeDeclarationSyntax type)
    {
        if (type is ClassDeclarationSyntax)
        {
            return true;
        }

        return type is RecordDeclarationSyntax record && !record.ClassOrStructKeyword.IsKind(SyntaxKind.StructKeyword);
    }

    /// <summary>Whether the declaration is a struct, a record struct or an enum.</summary>
    public static bool IsValueType(BaseTypeDeclarationSyntax type)
    {
        if (type is StructDeclarationSyntax || type is EnumDeclarationSyntax)
        {
            return true;
        }

        return type is RecordDeclarationSyntax record && record.ClassOrStructKeyword.IsKind(SyntaxKind.StructKeyword);
    }

    /// <summary>An attribute's name without its namespace and without the <c>Attribute</c> suffix.</summary>
    public static string AttributeName(AttributeSyntax attribute)
    {
        var name = SimpleName(attribute.Name);

        if (name.EndsWith(AttributeSuffix, StringComparison.Ordinal))
        {
            name = name[..^AttributeSuffix.Length];
        }

        return name;
    }

    /// <summary>The simple name of a type: <c>List</c> for <c>System.Collections.Generic.List&lt;int&gt;</c>.</summary>
    public static string SimpleName(TypeSyntax type)
    {
        if (type is QualifiedNameSyntax qualified)
        {
            return SimpleName(qualified.Right);
        }

        if (type is AliasQualifiedNameSyntax alias)
        {
            return SimpleName(alias.Name);
        }

        if (type is SimpleNameSyntax simple)
        {
            return simple.Identifier.Text;
        }

        return type.ToString();
    }
}
