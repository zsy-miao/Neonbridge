// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>No path and no line of a text file holds a word of the policy, in any letter case.</summary>
public sealed class ForbiddenWordsRule : IRule
{
    private const int PathLine = 1;

    public string Name => "ForbiddenWords";

    public IReadOnlyList<Violation> Check(CodeBase codeBase)
    {
        var violations = new List<Violation>();

        foreach (var file in codeBase.Files)
        {
            var inPath = FindWord(file.Path);

            if (inPath is not null)
            {
                violations.Add(new Violation(file.Path, PathLine, Name, $"the path holds the forbidden word `{inPath}`"));
            }

            var lines = file.Lines();

            for (var index = 0; index < lines.Count; index++)
            {
                var inLine = FindWord(lines[index]);

                if (inLine is not null)
                {
                    violations.Add(new Violation(file.Path, index + 1, Name, $"the line holds the forbidden word `{inLine}`"));
                }
            }
        }

        return violations;
    }

    private static string? FindWord(string text) =>
        CodePolicy.ForbiddenWords.FirstOrDefault(word => text.Contains(word, StringComparison.OrdinalIgnoreCase));
}
