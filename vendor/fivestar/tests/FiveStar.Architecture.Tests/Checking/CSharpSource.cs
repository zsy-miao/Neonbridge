// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>A C# file parsed for its syntax only, with the project that holds it and its folder below that project.</summary>
public sealed class CSharpSource
{
    private static readonly CSharpParseOptions ParseOptions = new(LanguageVersion.Preview);

    public CSharpSource(SourceFile file, ProjectFile? project)
    {
        File = file;
        Project = project;
        Tree = CSharpSyntaxTree.ParseText(file.Text ?? string.Empty, ParseOptions, file.Path);
        Text = Tree.GetText();
        Root = Tree.GetCompilationUnitRoot();
    }

    public SourceFile File { get; }

    public ProjectFile? Project { get; }

    public SyntaxTree Tree { get; }

    public SourceText Text { get; }

    public CompilationUnitSyntax Root { get; }

    /// <summary>The folders between the project folder and the file, joined with <c>/</c>; empty at the project root.</summary>
    public string Folder
    {
        get
        {
            var start = 0;

            if (Project is not null && Project.Folder.Length > 0)
            {
                start = Project.Folder.Length + 1;
            }

            var slash = File.Path.LastIndexOf('/');

            if (slash < start)
            {
                return string.Empty;
            }

            return File.Path[start..slash];
        }
    }

    /// <summary>The 0-based line a position falls on.</summary>
    public int LineIndex(int position) => Text.Lines.GetLineFromPosition(position).LineNumber;

    /// <summary>Whether the 0-based line holds only white space.</summary>
    public bool IsBlank(int lineIndex)
    {
        if (lineIndex < 0 || lineIndex >= Text.Lines.Count)
        {
            return false;
        }

        return string.IsNullOrWhiteSpace(Text.Lines[lineIndex].ToString());
    }

    public Violation Violation(string rule, SyntaxNode node, string message) =>
        new(File.Path, LineIndex(node.SpanStart) + 1, rule, message);

    public Violation Violation(string rule, SyntaxToken token, string message) =>
        new(File.Path, LineIndex(token.SpanStart) + 1, rule, message);
}
