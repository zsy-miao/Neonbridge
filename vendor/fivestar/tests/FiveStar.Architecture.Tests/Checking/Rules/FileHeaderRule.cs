// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>Every code file starts with the two header lines in its own comment style; a shebang may come first.</summary>
public sealed class FileHeaderRule : IRule
{
    private const string Shebang = "#!";

    public string Name => "FileHeader";

    public IReadOnlyList<Violation> Check(CodeBase codeBase)
    {
        var violations = new List<Violation>();

        foreach (var file in codeBase.Files)
        {
            if (file.Text is null || !CodePolicy.CommentMarkers.TryGetValue(file.Extension, out var marker))
            {
                continue;
            }

            var violation = CheckFile(file, marker);

            if (violation is not null)
            {
                violations.Add(violation);
            }
        }

        return violations;
    }

    private Violation? CheckFile(SourceFile file, string marker)
    {
        var lines = file.Lines();
        var first = 0;

        if (marker == "#" && lines.Count > 0 && lines[0].StartsWith(Shebang, StringComparison.Ordinal))
        {
            first = 1;
        }

        for (var index = 0; index < CodePolicy.HeaderLines.Count; index++)
        {
            var expected = $"{marker} {CodePolicy.HeaderLines[index]}";
            var lineIndex = first + index;

            if (lineIndex >= lines.Count || lines[lineIndex] != expected)
            {
                return new Violation(file.Path, lineIndex + 1, Name, $"expected the header line `{expected}`");
            }
        }

        return null;
    }
}
