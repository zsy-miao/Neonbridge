// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Pipeline creations that returned no pipeline, counted by slot and cause, for the sample summary. Failures for a
/// program that is not among the extracted ones are also listed by program key, all of them: each is a program the
/// extraction misses.
/// </summary>
internal sealed class PipelineFailureTally
{
    /// <summary>The cause of a creation whose program is not extracted, followed by its key.</summary>
    public const string MissingProgram = "no extracted program ";

    /// <summary>Causes the summary names; the rest are counted only.</summary>
    private const int ShownCauses = 8;

    private readonly ConcurrentDictionary<string, int> _causes = new(StringComparer.Ordinal);

    /// <summary>Counts one failed creation through <paramref name="slot"/>.</summary>
    public void Record(int slot, string cause)
    {
        _causes.AddOrUpdate($"slot {slot}: {cause}", 1, (_, count) => count + 1);
    }

    /// <summary>The failures, most frequent cause first, then those for missing programs by key; empty when none.</summary>
    public string Summary()
    {
        var causes = _causes.ToArray();

        if (causes.Length == 0)
        {
            return string.Empty;
        }

        int total = causes.Sum(cause => cause.Value);
        var shown = causes.OrderByDescending(cause => cause.Value).ThenBy(cause => cause.Key, StringComparer.Ordinal).Take(ShownCauses);
        var text = $"{total} failed in {causes.Length} causes ({string.Join("; ", shown.Select(cause => $"{cause.Value} x {cause.Key}"))})";

        var missing = causes
            .Where(cause => cause.Key.Contains(MissingProgram, StringComparison.Ordinal))
            .GroupBy(cause => cause.Key[(cause.Key.IndexOf(MissingProgram, StringComparison.Ordinal) + MissingProgram.Length)..], StringComparer.Ordinal)
            .Select(key => (Key: key.Key, Count: key.Sum(cause => cause.Value)))
            .OrderByDescending(key => key.Count)
            .ThenBy(key => key.Key, StringComparer.Ordinal)
            .ToArray();

        if (missing.Length == 0)
        {
            return text + ", none for missing programs";
        }

        return text + $", {missing.Sum(key => key.Count)} for {missing.Length} missing programs ({string.Join("; ", missing.Select(key => $"{key.Count} x {key.Key}"))})";
    }
}
