// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>A class has at most one public constructor (a primary constructor counts), without optional parameters.</summary>
public sealed class ConstructorShapeRule : SyntaxRule
{
    public override string Name => "ConstructorShape";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var type in source.Root.DescendantNodes().OfType<TypeDeclarationSyntax>().Where(TypeDeclarations.IsClass))
        {
            var parameterLists = PublicConstructors(type).ToArray();

            if (parameterLists.Length > 1)
            {
                yield return source.Violation(Name, type.Identifier, $"class `{type.Identifier.Text}` has {parameterLists.Length} public constructors");
            }

            var optional = parameterLists.SelectMany(list => list.Parameters).Where(parameter => parameter.Default is not null);

            foreach (var parameter in optional)
            {
                yield return source.Violation(Name, parameter, $"optional constructor parameter `{parameter.Identifier.Text}`");
            }
        }
    }

    /// <summary>The parameter lists of the public constructors, the primary constructor first.</summary>
    private static IEnumerable<ParameterListSyntax> PublicConstructors(TypeDeclarationSyntax type)
    {
        if (type.ParameterList is not null)
        {
            yield return type.ParameterList;
        }

        var constructors = type.Members
            .OfType<ConstructorDeclarationSyntax>()
            .Where(constructor => constructor.Modifiers.Any(SyntaxKind.PublicKeyword));

        foreach (var constructor in constructors)
        {
            yield return constructor.ParameterList;
        }
    }
}
