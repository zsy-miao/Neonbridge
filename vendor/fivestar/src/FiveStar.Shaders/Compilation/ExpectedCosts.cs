// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// What each program of a run is expected to cost, in milliseconds of dxc: its compiles' time in the output folder's
/// <see cref="TimedCompiler.LogName"/> from an earlier run, else its FSSM size at the logged programs' milliseconds per
/// byte (one when nothing is logged, then only the proportions mean anything). dxc time grows far faster than the
/// program count suggests: the largest programs take minutes, most take a second.
/// </summary>
public static class ExpectedCosts
{
    private const int NameField = 0;
    private const int OptimizationField = 2;
    private const int MillisecondsField = 5;
    private const int OutcomeField = 6;
    private const string OptimizedFlag = "-O3";
    private const string TimeoutOutcome = "timeout";

    /// <summary>The cost of each of <paramref name="paths"/>, in their order.</summary>
    public static double[] Of(IReadOnlyList<string> paths, string outputFolder)
    {
        var sizes = paths.Select(Size).ToArray();
        var logged = Logged(Path.Combine(outputFolder, TimedCompiler.LogName));
        long loggedBytes = 0;
        double loggedMilliseconds = 0;

        for (var index = 0; index < paths.Count; index++)
        {
            if (logged.TryGetValue(Path.GetFileNameWithoutExtension(paths[index]), out var milliseconds))
            {
                loggedBytes += sizes[index];
                loggedMilliseconds += milliseconds;
            }
        }

        double perByte = 1;

        if (loggedBytes > 0)
        {
            perByte = loggedMilliseconds / loggedBytes;
        }

        var costs = new double[paths.Count];

        for (var index = 0; index < paths.Count; index++)
        {
            if (!logged.TryGetValue(Path.GetFileNameWithoutExtension(paths[index]), out costs[index]))
            {
                costs[index] = sizes[index] * perByte;
            }
        }

        return costs;
    }

    /// <summary>
    /// The HLSL files whose -O3 compile timed out in the output folder's log from an earlier run: they compile with -Od
    /// from the start instead of spending the whole limit at -O3 first.
    /// </summary>
    public static HashSet<string> TimedOut(string outputFolder)
    {
        var files = new HashSet<string>(StringComparer.Ordinal);
        var path = Path.Combine(outputFolder, TimedCompiler.LogName);

        if (!File.Exists(path))
        {
            return files;
        }

        using var reader = new StreamReader(File.Open(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite));

        for (var line = reader.ReadLine(); line is not null; line = reader.ReadLine())
        {
            var fields = line.Split('\t');

            if (fields.Length > OutcomeField && fields[OptimizationField] == OptimizedFlag && fields[OutcomeField] == TimeoutOutcome)
            {
                files.Add(fields[NameField]);
            }
        }

        return files;
    }

    private static long Size(string path)
    {
        if (!File.Exists(path))
        {
            return 0;
        }

        return new FileInfo(path).Length;
    }

    /// <summary>Milliseconds per program in a compile log (every form of a program counts towards it); empty without one.</summary>
    private static Dictionary<string, double> Logged(string path)
    {
        var programs = new Dictionary<string, double>(StringComparer.Ordinal);

        if (!File.Exists(path))
        {
            return programs;
        }

        using var reader = new StreamReader(File.Open(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite));

        for (var line = reader.ReadLine(); line is not null; line = reader.ReadLine())
        {
            var fields = line.Split('\t');

            if (fields.Length <= MillisecondsField || !double.TryParse(fields[MillisecondsField], NumberStyles.Float, CultureInfo.InvariantCulture, out var milliseconds))
            {
                continue;
            }

            var name = fields[NameField];
            var stem = name[..Math.Max(0, name.IndexOf('.', StringComparison.Ordinal))];
            programs[stem] = programs.GetValueOrDefault(stem) + milliseconds;
        }

        return programs;
    }
}
