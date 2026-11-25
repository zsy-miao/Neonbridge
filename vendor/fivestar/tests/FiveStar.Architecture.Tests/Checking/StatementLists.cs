// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>The runs of statements the spacing rules read: each block, each <c>case</c> and the top-level statements.</summary>
public static class StatementLists
{
    public static IEnumerable<IReadOnlyList<StatementSyntax>> All(CompilationUnitSyntax root)
    {
        var globals = root.Members.OfType<GlobalStatementSyntax>().Select(global => global.Statement).ToArray();

        if (globals.Length > 0)
        {
            yield return globals;
        }

        foreach (var node in root.DescendantNodes())
        {
            if (node is BlockSyntax block)
            {
                yield return block.Statements;
            }
            else if (node is SwitchSectionSyntax section)
            {
                yield return section.Statements;
            }
        }
    }

    /// <summary>The 0-based line a statement starts on; a comment right above it belongs to it.</summary>
    public static int StartLine(CSharpSource source, StatementSyntax statement)
    {
        var token = statement.GetFirstToken();

        foreach (var trivia in token.LeadingTrivia)
        {
            if (IsComment(trivia))
            {
                return source.LineIndex(trivia.SpanStart);
            }
        }

        return source.LineIndex(token.SpanStart);
    }

    public static int EndLine(CSharpSource source, StatementSyntax statement) => source.LineIndex(statement.Span.End);

    /// <summary>Whether a blank line separates a statement from the one before it.</summary>
    public static bool BlankBetween(CSharpSource source, StatementSyntax previous, StatementSyntax next)
    {
        var start = StartLine(source, next);

        for (var line = EndLine(source, previous) + 1; line < start; line++)
        {
            if (source.IsBlank(line))
            {
                return true;
            }
        }

        return false;
    }

    private static bool IsComment(SyntaxTrivia trivia) =>
        trivia.IsKind(SyntaxKind.SingleLineCommentTrivia)
        || trivia.IsKind(SyntaxKind.MultiLineCommentTrivia)
        || trivia.IsKind(SyntaxKind.SingleLineDocumentationCommentTrivia)
        || trivia.IsKind(SyntaxKind.MultiLineDocumentationCommentTrivia);
}
