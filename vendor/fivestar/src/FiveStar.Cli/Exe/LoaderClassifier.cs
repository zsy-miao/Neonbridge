// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;

namespace FiveStar.Cli.Exe;

/// <summary>
/// Runs <c>fivestar-loader --classify</c> on a list of <c>module!symbol</c> queries and reads its verdicts back, so
/// the report classifies imports with the loader's own shim table instead of a second copy of the list. A missing or
/// failed loader yields null, and the report falls back to the unclassified view.
/// </summary>
internal static class LoaderClassifier
{
    private const char FieldSeparator = '\t';
    private const int FieldCount = 3;
    private const int KindField = 0;
    private const int KeyField = 1;
    private const int DetailField = 2;
    private const int TimeoutMs = 60000;
    private const string InputName = "exe-analyze-queries.txt";
    private const string OutputName = "exe-analyze-verdicts.txt";

    /// <summary>
    /// Verdicts keyed by the <c>module!symbol</c> query, or null when the loader could not be run.
    /// <paramref name="packageDir"/> is the folder the executable ships in; the loader treats modules found there as
    /// package modules.
    /// </summary>
    public static IReadOnlyDictionary<string, LoaderVerdict>? Classify(string loaderPath, IReadOnlyCollection<string> queries, string workDir, string packageDir)
    {
        var input = Path.Combine(workDir, InputName);
        var output = Path.Combine(workDir, OutputName);
        File.WriteAllLines(input, queries);

        if (!RunLoader(loaderPath, input, output, packageDir))
        {
            return null;
        }

        return Parse(output);
    }

    private static bool RunLoader(string loaderPath, string input, string output, string packageDir)
    {
        var start = new ProcessStartInfo(loaderPath)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };

        start.ArgumentList.Add("--classify");
        start.ArgumentList.Add(input);
        start.ArgumentList.Add("--classify-out");
        start.ArgumentList.Add(output);
        start.ArgumentList.Add("--search");
        start.ArgumentList.Add(packageDir);

        return Execute(start, output);
    }

    private static bool Execute(ProcessStartInfo start, string output)
    {
        try
        {
            using var process = Process.Start(start);

            if (process is null)
            {
                return false;
            }

            process.WaitForExit(TimeoutMs);

            return process.HasExited && process.ExitCode == 0 && File.Exists(output);
        }
        catch (Exception e) when (e is IOException or System.ComponentModel.Win32Exception or UnauthorizedAccessException)
        {
            return false;
        }
    }

    private static Dictionary<string, LoaderVerdict> Parse(string output)
    {
        var verdicts = new Dictionary<string, LoaderVerdict>(StringComparer.Ordinal);

        foreach (var line in File.ReadLines(output))
        {
            var fields = line.Split(FieldSeparator, FieldCount);

            if (fields.Length == FieldCount)
            {
                verdicts[fields[KeyField]] = new LoaderVerdict(fields[KindField], fields[DetailField]);
            }
        }

        return verdicts;
    }
}
