// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using FiveStar.Cli.Exe;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Builds pipelines with the desktop driver into the loader's pipeline cache (<c>&lt;work&gt;/pipeline-cache</c>, the
/// folder <c>fivestar run</c> gives the loader), so the game loads them instead of waiting for the driver: runs
/// <c>fivestar-loader --warm-pipelines</c>. By default only the pipelines earlier boots used (the loader's
/// <c>pipelines-used.bin</c>; without one there is nothing to do, since the game builds pipelines on first use), with
/// <c>--warm-all</c> also every pipeline of the console pipeline cache. An interrupted run keeps every pipeline built
/// so far, and the next run resumes.
/// </summary>
internal sealed class PipelineWarmStep : ISetupStep
{
    public const string StepName = "pipeline-warm";

    // Measured on an RTX 4090: about 140 KiB per pipeline, 28,164 pipelines.
    private const long ExpectedBytes = 4L << 30;

    private const string SegmentPattern = "*.psolib";
    private const string LoaderOption = "loader";

    /// <summary>The setup option that warms every pipeline of the console pipeline cache too.</summary>
    public const string WarmAllFlag = "warm-all";

    private const string JournalName = FiveStar.Shaders.Cache.PipelineJournalFile.FileName;

    // Measured on an RTX 4090 with 4 jobs: every 20th pipeline in 26 minutes; the mesh pipelines take minutes each.
    private static readonly TimeSpan TypicalTime = TimeSpan.FromHours(9);

    // Before a measurement of a journal-only warm-up: an assumption.
    private static readonly TimeSpan UsedTime = TimeSpan.FromMinutes(30);

    public string Name => StepName;

    public string Title => "Build the desktop pipelines into the pipeline cache";

    public IReadOnlyList<string> DependsOn => [ShaderTranslateStep.StepName, PipelineCacheStep.StepName];

    public TimeSpan Estimate(SetupContext context)
    {
        if (All(context))
        {
            return TypicalTime;
        }

        return UsedTime;
    }

    public StepFingerprint Fingerprint(SetupContext context)
    {
        var loader = Loader(context);
        var tool = "no-loader";

        if (loader is not null)
        {
            tool = Fingerprints.OfFiles(Path.GetDirectoryName(loader)!, [loader]);
        }

        var inputs = Fingerprints.Combine(Fingerprints.OfFiles(context.Dump, [context.ShaderArchive]), context.Sample, All(context).ToString(), Fingerprints.OfFiles(context.Work, [Journal(context)]));

        return new StepFingerprint(inputs, tool);
    }

    /// <summary>The cache has segments; a driver update makes the next loader run delete them, which redoes the step.</summary>
    public bool HasOutputs(SetupContext context) => (!All(context) && !File.Exists(Journal(context))) ||
        Directory.Exists(context.PipelineLibraryFolder) && Directory.EnumerateFiles(context.PipelineLibraryFolder, SegmentPattern).Any();

    public long RequiredBytes(SetupContext context) => Math.Max(0, ExpectedBytes - StepText.FolderSize(context.PipelineLibraryFolder));

    public string? MissingRequirement(SetupContext context)
    {
        if (Loader(context) is null)
        {
            return "fivestar-loader not found (build it, or pass --loader <path> or set FIVESTAR_LOADER)";
        }

        return null;
    }

    public StepResult Run(SetupContext context)
    {
        if (!All(context) && !File.Exists(Journal(context)))
        {
            return StepResult.Done($"nothing to warm yet: the game builds pipelines on first use and records them in {Journal(context)} for the next run (--warm-all warms the console pipeline cache)");
        }

        Directory.CreateDirectory(context.LogFolder);
        var log = Path.Combine(context.LogFolder, $"pipeline-warm-{DateTime.Now:yyyyMMdd-HHmmss}.log");
        var start = new ProcessStartInfo(Loader(context)!) { UseShellExecute = false };
        string[] arguments =
        [
            "--warm-pipelines", context.ShaderArchive,
            "--shader-dir", context.ProgramFolder,
            "--translated-dir", context.TranslationFolder,
            "--pipeline-cache", context.PipelineLibraryFolder,
            "--warm-step", context.Every.ToString(CultureInfo.InvariantCulture),
            "--log", log,
        ];

        if (!All(context))
        {
            start.ArgumentList.Add("--warm-used-only");
        }

        foreach (var argument in arguments)
        {
            start.ArgumentList.Add(argument);
        }

        using var process = Process.Start(start) ?? throw new InvalidOperationException("cannot start fivestar-loader");
        process.WaitForExit();

        if (process.ExitCode != 0)
        {
            return StepResult.Failed($"fivestar-loader --warm-pipelines exit code {process.ExitCode} (see {log}); run setup again to resume");
        }

        return StepResult.Done($"pipelines in {context.PipelineLibraryFolder} ({StepText.Size(StepText.FolderSize(context.PipelineLibraryFolder))})");
    }

    private static bool All(SetupContext context) => context.Arguments.Flag(WarmAllFlag);

    private static string Journal(SetupContext context) => Path.Combine(context.PipelineLibraryFolder, JournalName);

    private static string? Loader(SetupContext context)
    {
        return LoaderLocator.Locate(context.Arguments.Option(LoaderOption));
    }
}
