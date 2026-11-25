// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>Formats violations as a test failure message, one <c>path:line: [Rule] message</c> per line.</summary>
public static class ViolationReport
{
    public static string Format(IReadOnlyList<Violation> violations)
    {
        var lines = violations
            .OrderBy(violation => violation.Path, StringComparer.Ordinal)
            .ThenBy(violation => violation.Line)
            .Select(violation => violation.ToString());

        return $"{violations.Count} violation(s):{Environment.NewLine}{string.Join(Environment.NewLine, lines)}";
    }
}
