// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;

namespace FiveStar.Cli.Exe;

/// <summary>
/// <c>fivestar exe analyze</c>: reads the executable and writes a Markdown and JSON report. All output, including any
/// files the loader classification needs, stays under the <c>--out</c> folder.
/// </summary>
internal static class AnalyzeCommand
{
    private const int Success = 0;
    private const int UsageError = 2;
    private const string DumpVariable = "FIVESTAR_DUMP";
    private const string GdkLibVariable = "FIVESTAR_GDK_LIB";
    private const string DefaultOutName = "fivestar-exe-report";

    public static int Run(CommandArguments arguments)
    {
        var exePath = arguments.Positionals[0];

        if (!File.Exists(exePath))
        {
            Console.Error.WriteLine($"no such file: {exePath}");

            return UsageError;
        }

        var options = BuildOptions(arguments, exePath);
        Directory.CreateDirectory(options.OutDir);
        var analysis = ExeAnalyzer.Analyze(options);
        Write(options, analysis);

        return Success;
    }

    private static AnalyzeOptions BuildOptions(CommandArguments arguments, string exePath)
    {
        var dump = Existing(arguments.Option("--dump") ?? Environment.GetEnvironmentVariable(DumpVariable), true);
        var gdkLib = Existing(arguments.Option("--gdk-lib") ?? Environment.GetEnvironmentVariable(GdkLibVariable), false);
        var outDir = arguments.Option("--out") ?? Path.Combine(Directory.GetCurrentDirectory(), DefaultOutName);
        var loader = arguments.Option("--loader");

        return new AnalyzeOptions(exePath, dump, gdkLib, Path.GetFullPath(outDir), loader);
    }

    private static void Write(AnalyzeOptions options, ExeAnalysis analysis)
    {
        var baseName = Path.GetFileNameWithoutExtension(options.ExePath);
        var markdownPath = Path.Combine(options.OutDir, baseName + ".report.md");
        var jsonPath = Path.Combine(options.OutDir, baseName + ".report.json");

        File.WriteAllText(markdownPath, ReportMarkdown.Build(analysis));
        File.WriteAllText(jsonPath, ReportJson.Build(analysis));

        Summary(analysis, markdownPath, jsonPath);
    }

    private static void Summary(ExeAnalysis analysis, string markdownPath, string jsonPath)
    {
        Console.Out.WriteLine($"{analysis.FileName}: {analysis.Sections.Count} sections, {analysis.Imports.Count} imports, {analysis.GdkThunkCount} GDK thunks ({analysis.GdkNamedCount} named), {analysis.DayOneGaps.Count} day-one gaps");

        if (!analysis.LoaderClassified)
        {
            Console.Out.WriteLine("  loader classification unavailable: pass --loader or set FIVESTAR_LOADER");
        }

        Console.Out.WriteLine("  " + markdownPath);
        Console.Out.WriteLine("  " + jsonPath);
    }

    private static string? Existing(string? path, bool directory)
    {
        if (path is null)
        {
            return null;
        }

        if (directory && Directory.Exists(path))
        {
            return path;
        }

        if (!directory && File.Exists(path))
        {
            return path;
        }

        return null;
    }
}
