// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// <c>--skip-pipelines &lt;text,...&gt;</c>: a pipeline whose programs' names hold one of the texts is treated as
/// unbuildable. It is neither loaded from the pipeline cache nor built, so it stays a stand-in and its draws are skipped;
/// each one is logged once. A way past a pipeline that hangs the GPU, to see what the rest of the frame does without it.
/// </summary>
internal static class SkippedPipelines
{
    private static readonly ConcurrentDictionary<string, byte> Logged = new(StringComparer.Ordinal);

    /// <summary>The texts; empty for none.</summary>
    public static IReadOnlyList<string> Texts { get; set; } = [];

    /// <summary>The texts of a comma-separated list, blanks dropped.</summary>
    public static IReadOnlyList<string> Parse(string list)
    {
        return list.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
    }

    /// <summary>The first of <paramref name="texts"/> that <paramref name="names"/> holds; null for none.</summary>
    public static string? Match(string names, IReadOnlyList<string> texts)
    {
        return texts.FirstOrDefault(text => names.Contains(text, StringComparison.Ordinal));
    }

    /// <summary>True when the pipeline of <paramref name="names"/> is skipped; the first time, logged.</summary>
    public static bool Skips(string names)
    {
        return Skips(names, Texts);
    }

    /// <summary>True when one of <paramref name="texts"/> skips the pipeline of <paramref name="names"/>; the first time, logged.</summary>
    public static bool Skips(string names, IReadOnlyList<string> texts)
    {
        if (RayTracingPrograms.Skips(names))
        {
            return true;
        }

        var text = Match(names, texts);

        if (text is null)
        {
            return false;
        }

        if (Logged.TryAdd(names, 0))
        {
            ShimContext.Log?.Warn($"[d3d12_xs] pipeline {names} skipped (--skip-pipelines {text}): it stays a stand-in and its draws are skipped");
        }

        return true;
    }
}
