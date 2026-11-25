// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using FiveStar.Cli.Exe;
using FiveStar.Cli.Run;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Builds pipelines with the desktop driver into the loader's pipeline cache (<c>&lt;work&gt;/pipeline-cache</c>, the
/// folder <c>fivestar run</c> gives the loader), so the game loads them instead of waiting for the driver: runs
/// <c>fivestar-loader --warm-pipelines</c>. By default only the pipelines earlier boots used (the loader's
/// <c>pipelines-used.bin</c>; without one there is nothing to do, since the game builds pipelines on first use), with
/// <c>--warm-all</c> also every pipeline of the console pipeline cache. An interrupted run keeps every pipeline built
/// so far, and the next run resumes. The programs come from the same folders as the boot of <c>fivestar run</c> (the
/// extracted programs made on demand into the work folder's program cache, the translation only with
/// <c>--translate-ahead</c>), so their DXIL and the pipelines' keys are the boot's; a finished run stamps the cache with
/// the translator it built for, as <c>fivestar run --warm</c> does (<see cref="JournalWarmUp"/>).
/// </summary>
internal sealed class PipelineWarmStep : ISetupStep
{
    public const string StepName = "pipeline-warm";

    // Measured on an RTX 4090: about 140 KiB per pipeline, 28,164 pipelines.
    private const long ExpectedBytes = 4L << 30;

    private const string SegmentPattern = "*.psolib";
    private const string LoaderOption = "loader";
    private const string DxcOption = "dxc";

    /// <summary>The setup option that warms every pipeline of the console pipeline cache too.</summary>
    public const string WarmAllFlag = "warm-all";

    private const string JournalName = FiveStar.Shaders.Cache.PipelineJournalFile.FileName;

    // Measured on an RTX 4090 with 4 jobs: every 20th pipeline in 26 minutes; the mesh pipelines take minutes each.
    private static readonly TimeSpan TypicalTime = TimeSpan.FromHours(9);

    // Before a measurement of a journal-only warm-up: an assumption.
    private static readonly TimeSpan UsedTime = TimeSpan.FromMinutes(30);

    public string Name => StepName;

    public string Title => "Build the desktop pipelines into the pipeline cache";

    public IReadOnlyList<string> DependsOn => [ShaderExtractStep.StepName, PipelineCacheStep.StepName];

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

        foreach (var argument in Arguments(context, log))
        {
            start.ArgumentList.Add(argument);
        }

        using var process = Process.Start(start) ?? throw new InvalidOperationException("cannot start fivestar-loader");
        process.WaitForExit();

        if (process.ExitCode != 0)
        {
            return StepResult.Failed($"fivestar-loader --warm-pipelines exit code {process.ExitCode} (see {log}); run setup again to resume");
        }

        JournalWarmUp.Stamp(context.PipelineLibraryFolder, JournalWarmUp.Translator(Loader(context)!));

        return StepResult.Done($"pipelines in {context.PipelineLibraryFolder} ({StepText.Size(StepText.FolderSize(context.PipelineLibraryFolder))})");
    }

    /// <summary>The loader arguments: the boot's folders of <c>fivestar run</c> (<see cref="FiveStar.Cli.Run.LoaderCommandLine"/>), the sample, the log.</summary>
    public static List<string> Arguments(SetupContext context, string log)
    {
        var arguments = new List<string>
        {
            "--warm-pipelines", context.ShaderArchive,
            "--shader-dir", context.ProgramFolder,
            "--program-cache", context.ProgramCacheFolder,
            "--pipeline-cache", context.PipelineLibraryFolder,
            "--warm-step", context.Every.ToString(CultureInfo.InvariantCulture),
            "--log", log,
        };

        if (!All(context))
        {
            arguments.Add("--warm-used-only");
        }

        if (context.Arguments.Flag(SetupCommand.TranslateAheadFlag))
        {
            arguments.AddRange(["--translated-dir", context.TranslationFolder]);
        }

        var dxc = context.Arguments.Option(DxcOption);

        if (dxc is not null)
        {
            arguments.AddRange(["--dxc", Path.GetFullPath(dxc)]);
        }

        return arguments;
    }

    private static bool All(SetupContext context) => context.Arguments.Flag(WarmAllFlag);

    private static string Journal(SetupContext context) => Path.Combine(context.PipelineLibraryFolder, JournalName);

    private static string? Loader(SetupContext context)
    {
        return LoaderLocator.Locate(context.Arguments.Option(LoaderOption));
    }
}
