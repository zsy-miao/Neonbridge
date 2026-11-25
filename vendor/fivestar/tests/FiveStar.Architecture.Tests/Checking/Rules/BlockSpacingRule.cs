// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>A blank line above every block statement and every exit that is not the first statement of its block or case.</summary>
public sealed class BlockSpacingRule : SyntaxRule
{
    public override string Name => "BlockSpacing";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var statements in StatementLists.All(source.Root))
        {
            for (var index = 1; index < statements.Count; index++)
            {
                var statement = statements[index];

                if (!NeedsBlankLine(statement) || source.IsBlank(StatementLists.StartLine(source, statement) - 1))
                {
                    continue;
                }

                yield return source.Violation(Name, statement, $"a blank line goes above this {Describe(statement)}");
            }
        }
    }

    private static bool NeedsBlankLine(StatementSyntax statement) =>
        statement is BlockSyntax
            or IfStatementSyntax
            or ForStatementSyntax
            or CommonForEachStatementSyntax
            or WhileStatementSyntax
            or DoStatementSyntax
            or SwitchStatementSyntax
            or TryStatementSyntax
            or UsingStatementSyntax
            or LockStatementSyntax
            or FixedStatementSyntax
            or CheckedStatementSyntax
            or UnsafeStatementSyntax
            or LocalFunctionStatementSyntax
            or ReturnStatementSyntax
            or ThrowStatementSyntax
            or YieldStatementSyntax;

    private static string Describe(StatementSyntax statement)
    {
        if (statement is ReturnStatementSyntax || statement is ThrowStatementSyntax || statement is YieldStatementSyntax)
        {
            return "exit";
        }

        return "block statement";
    }
}
