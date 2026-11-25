// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>At most a fixed number of statements in a row without a blank line between them.</summary>
public sealed class StatementGroupingRule : SyntaxRule
{
    public override string Name => "StatementGrouping";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        foreach (var statements in StatementLists.All(source.Root))
        {
            var run = 1;

            for (var index = 1; index < statements.Count; index++)
            {
                run++;

                if (StatementLists.BlankBetween(source, statements[index - 1], statements[index]))
                {
                    run = 1;
                }

                if (run == CodePolicy.MaxStatementsPerGroup + 1)
                {
                    var message = $"more than {CodePolicy.MaxStatementsPerGroup} statements in a row without a blank line";

                    yield return source.Violation(Name, statements[index], message);
                }
            }
        }
    }
}
