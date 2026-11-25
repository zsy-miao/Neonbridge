// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Architecture.Tests.Checking;
using Xunit;

namespace FiveStar.Architecture.Tests;

/// <summary>Checks every file of the repository against each code rule, one test case per rule.</summary>
public sealed class RepositoryRulesTests
{
    public static TheoryData<string> Rules
    {
        get
        {
            var rules = new TheoryData<string>();

            foreach (var rule in RuleCatalog.All)
            {
                rules.Add(rule.Name);
            }

            return rules;
        }
    }

    [Theory]
    [MemberData(nameof(Rules))]
    public void The_repository_follows_the_rule(string rule)
    {
        var codeBase = CodeBaseLoader.Load(CodeBaseLoader.FindRoot(AppContext.BaseDirectory));

        var violations = RuleCatalog.Find(rule).Check(codeBase);

        Assert.True(violations.Count == 0, ViolationReport.Format(violations));
    }
}
