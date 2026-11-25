// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Cli.Progress;

/// <summary>
/// Shader stages from a translate run's <c>results.json</c>: <c>ok</c> is done, <c>ok-msfb</c> is done through the
/// mesh-shader fallback, anything else is not yet, unless the program's <c>.dxil</c> exists next to its <c>.hlsl</c>
/// (a later compile finished what the run had given up on).
/// </summary>
internal static class ShaderTally
{
    public const string Key = "shaders";

    private const string Title = "Shaders translated to HLSL that compile to DXIL";
    private const double Height = 260;
    private const string Ok = "ok";
    private const string MeshFallback = "ok-msfb";
    private const string DxilExtension = ".dxil";
    private const int ReasonsShown = 3;
    private const int ReasonLength = 40;

    private static readonly (string Stage, string Name)[] Stages =
    [
        ("ps", "Pixel"), ("ms", "Mesh"), ("cs", "Compute"), ("vs", "Vertex"), ("gs", "Geometry"),
    ];

    public static ProgressArea Measure(string resultsPath)
    {
        var directory = Path.GetDirectoryName(Path.GetFullPath(resultsPath)) ?? ".";
        var done = new Tally<string>();
        var partial = new Tally<string>();
        var todo = new Tally<string>();
        var reasons = new Dictionary<string, Tally<string>>(StringComparer.Ordinal);

        foreach (var result in StatusFile.ReadList<ShaderResult>(resultsPath))
        {
            if (result.Stage is null)
            {
                continue;
            }

            if (result.Status == Ok || File.Exists(Path.Combine(directory, result.Name + DxilExtension)))
            {
                done.Add(result.Stage);
            }
            else if (result.Status == MeshFallback)
            {
                partial.Add(result.Stage);
            }
            else
            {
                todo.Add(result.Stage);
                Reasons(reasons, result.Stage).Add(ReasonOf(result));
            }
        }

        var groups = new List<ProgressGroup>();

        foreach (var (stage, name) in Stages)
        {
            if (done[stage] + partial[stage] + todo[stage] > 0)
            {
                groups.Add(new ProgressGroup(name, done[stage], partial[stage], todo[stage], Note(reasons, stage)));
            }
        }

        return new ProgressArea(Key, Title, null, Height, groups);
    }

    private static Tally<string> Reasons(Dictionary<string, Tally<string>> reasons, string stage)
    {
        if (!reasons.TryGetValue(stage, out var tally))
        {
            tally = new Tally<string>();
            reasons[stage] = tally;
        }

        return tally;
    }

    private static string ReasonOf(ShaderResult result)
    {
        var reason = (result.Reason ?? result.Status).Split(':')[0];

        return reason[..Math.Min(reason.Length, ReasonLength)];
    }

    private static string? Note(Dictionary<string, Tally<string>> reasons, string stage)
    {
        if (!reasons.TryGetValue(stage, out var tally))
        {
            return null;
        }

        var parts = tally.MostCommon(ReasonsShown).Select(entry => $"{entry.Key} ({entry.Value})");

        return "not yet: " + string.Join("; ", parts);
    }
}
