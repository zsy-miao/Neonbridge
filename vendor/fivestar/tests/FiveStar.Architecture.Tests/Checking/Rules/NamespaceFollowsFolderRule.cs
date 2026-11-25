// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>The namespace is the project name followed by the folders between the project and the file.</summary>
public sealed class NamespaceFollowsFolderRule : SyntaxRule
{
    public override string Name => "NamespaceFollowsFolder";

    protected override bool Applies(CSharpSource source) => source.Project is not null;

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        var expected = ExpectedNamespace(source);
        var namespaces = source.Root.DescendantNodes().OfType<BaseNamespaceDeclarationSyntax>().ToArray();
        var types = TypeDeclarations.TopLevel(source.Root);

        if (namespaces.Length == 0 && types.Count > 0)
        {
            yield return source.Violation(Name, types[0], $"the type is in no namespace; expected `{expected}`");
        }

        foreach (var declaration in namespaces)
        {
            var actual = FullName(declaration);

            if (actual != expected)
            {
                yield return source.Violation(Name, declaration, $"namespace `{actual}` should be `{expected}`");
            }
        }
    }

    private static string ExpectedNamespace(CSharpSource source)
    {
        var project = source.Project!.Name;
        var folder = source.Folder;

        if (folder.Length == 0)
        {
            return project;
        }

        return $"{project}.{folder.Replace('/', '.')}";
    }

    private static string FullName(BaseNamespaceDeclarationSyntax declaration)
    {
        var parts = declaration
            .AncestorsAndSelf()
            .OfType<BaseNamespaceDeclarationSyntax>()
            .Reverse()
            .Select(part => part.Name.ToString());

        return string.Join('.', parts);
    }
}
