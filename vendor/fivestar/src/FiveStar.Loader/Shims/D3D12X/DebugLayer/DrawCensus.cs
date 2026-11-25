// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Text;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// Part of <c>--d3d12-trace-draws</c>: every draw, dispatch and ExecuteIndirect between two captured presents, counted
/// by pipeline and first render target, recorded or skipped (a stand-in's work), so a capture shows which passes ran
/// into which targets and which the frame lost. With <see cref="Passes"/> (<c>--census-passes</c>) every pair whose
/// pipeline or target names one of the texts is listed too, however few its draws, with where a skipped stand-in's
/// build is: the passes after the G-buffer (lighting, sky, exposure, tonemap, composite) are few draws each and never
/// reach the largest lines.
/// </summary>
internal sealed class DrawCensus
{
    private const int LinesShown = 60;
    private const int TargetsShown = 12;

    private readonly ConcurrentDictionary<(nint Pipeline, nint Target, bool Skipped), long> _counts = new();

    private readonly LoaderLog _log;
    private long _since;

    public DrawCensus(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>Texts (pipeline names or target descriptions such as "format 10") whose pairs are all listed.</summary>
    public static IReadOnlyList<string> Passes { get; set; } = [];

    /// <summary>Counts one piece of work.</summary>
    public void Add(nint pipeline, nint target, bool skipped)
    {
        _counts.AddOrUpdate((pipeline, target, skipped), 1, static (_, count) => count + 1);
    }

    /// <summary>Logs and clears the counts, the largest first; <paramref name="name"/> names a pipeline, <paramref name="describe"/> a target.</summary>
    public void Report(long present, Func<nint, bool, string> name, Func<nint, string> describe, Func<nint, string> standIn)
    {
        var counts = _counts.ToArray();
        _counts.Clear();

        var recorded = counts.Where(entry => !entry.Key.Skipped).Sum(entry => entry.Value);
        var skipped = counts.Where(entry => entry.Key.Skipped).Sum(entry => entry.Value);
        var text = new StringBuilder($"  [census] presents {_since}-{present}: {recorded} recorded, {skipped} skipped, {counts.Length} pipeline-target pairs");
        _since = present;

        // Per target first: which passes ran at all, then the pairs.
        foreach (var target in counts.Where(entry => entry.Key.Target != 0).GroupBy(entry => entry.Key.Target).OrderByDescending(group => group.Sum(entry => entry.Value)).Take(TargetsShown))
        {
            var ran = target.Where(entry => !entry.Key.Skipped).Sum(entry => entry.Value);
            var lost = target.Where(entry => entry.Key.Skipped).Sum(entry => entry.Value);
            text.Append($"\n    target 0x{target.Key:x} {describe(target.Key)}: {ran} recorded, {lost} skipped");
        }

        foreach (var (key, count) in counts.OrderByDescending(entry => entry.Value).Take(LinesShown))
        {
            string target = "no target";

            if (key.Target != 0)
            {
                target = $"0x{key.Target:x} {describe(key.Target)}";
            }

            string kind = "recorded";

            if (key.Skipped)
            {
                kind = "skipped";
            }

            text.Append($"\n    {count} {kind} {name(key.Pipeline, key.Skipped)} -> {target}");
        }

        AppendPasses(text, counts, name, describe, standIn);
        _log.Info(text.ToString());
    }

    /// <summary>Every pair a text of <see cref="Passes"/> names, by pipeline name; a skipped one with its stand-in's state.</summary>
    private static void AppendPasses(StringBuilder text, KeyValuePair<(nint Pipeline, nint Target, bool Skipped), long>[] counts, Func<nint, bool, string> name, Func<nint, string> describe, Func<nint, string> standIn)
    {
        if (Passes.Count == 0)
        {
            return;
        }

        var lines = new List<string>();

        foreach (var (key, count) in counts)
        {
            string pipeline = name(key.Pipeline, key.Skipped);
            string target = key.Target == 0 ? "no target" : $"0x{key.Target:x} {describe(key.Target)}";

            if (!Passes.Any(pass => pipeline.Contains(pass, StringComparison.OrdinalIgnoreCase) || target.Contains(pass, StringComparison.OrdinalIgnoreCase)))
            {
                continue;
            }

            string kind = key.Skipped ? $"skipped (stand-in {standIn(key.Pipeline)})" : "recorded";
            lines.Add($"\n      {pipeline} -> {target}: {count} {kind}");
        }

        text.Append($"\n    passes matching {string.Join(", ", Passes)}: {lines.Count} pairs");

        foreach (var line in lines.Order(StringComparer.Ordinal))
        {
            text.Append(line);
        }
    }
}
