// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Cli.Setup;
using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Run;

/// <summary>
/// The warm-up of the pipelines earlier boots used, after a translator update (docs/running.md). A pipeline's cache key
/// holds its DXIL, so a new translator build leaves the journal's pipelines unbuilt until the game meets each one, 20
/// to 60 s of driver compile at its first use. <c>fivestar run</c> offers the warm-up when the translator beside the
/// loader is not the one the last warm-up built for (a stamp in the pipeline cache), and <c>fivestar run --warm</c> runs
/// it before the boot: <c>fivestar-loader --warm-pipelines --warm-used-only</c> with the boot's shader folders, program
/// cache and loader options, in one process per 16 GB of RAM.
/// </summary>
internal static class JournalWarmUp
{
    /// <summary>The <c>fivestar run</c> flag that warms the journal before the boot.</summary>
    public const string Flag = "warm";

    /// <summary>The translator build the last warm-up built for, in the pipeline cache.</summary>
    public const string StampName = "journal-warm.txt";

    /// <summary>The translator, beside the loader (or the loader itself when it is published as one file).</summary>
    private const string TranslatorAssembly = "FiveStar.Shaders.dll";

    /// <summary>Warm-up processes: one per 16 GB of RAM, 1 to 4 (the loader's own default).</summary>
    private const ulong BytesPerProcess = 16UL << 30;
    private const int MaxProcesses = 4;

    /// <summary>
    /// Measured: 5,049 journal pipelines, a third of them built anew, in 21 minutes with 3 processes on a 64 GB machine
    /// with an RTX 4090: about 1.3 a second per process.
    /// </summary>
    private const double EntriesPerSecondPerProcess = 1.3;

    /// <summary>The warm-up processes for a machine of <paramref name="totalPhysical"/> bytes of RAM.</summary>
    public static int ProcessesFor(ulong totalPhysical)
    {
        return (int)Math.Clamp(totalPhysical / BytesPerProcess, 1, MaxProcesses);
    }

    /// <summary>About how long <paramref name="entries"/> journal pipelines take on <paramref name="processes"/> processes.</summary>
    public static TimeSpan Estimate(int entries, int processes)
    {
        return TimeSpan.FromSeconds(entries / (EntriesPerSecondPerProcess * Math.Max(1, processes)));
    }

    /// <summary>The translator build <paramref name="loaderPath"/> runs: its translator assembly's content, else the loader's.</summary>
    public static string Translator(string loaderPath)
    {
        string folder = Path.GetDirectoryName(loaderPath)!;
        string translator = Path.Combine(folder, TranslatorAssembly);

        if (File.Exists(translator))
        {
            return Fingerprints.OfContent(translator);
        }

        return Fingerprints.OfContent(loaderPath);
    }

    /// <summary>
    /// The journal pipelines a warm-up would build for <paramref name="translator"/>: all of the journal's when the stamp
    /// names another translator build (or none), 0 when it names this one or there is no journal.
    /// </summary>
    public static int Due(string pipelineCache, string translator)
    {
        string stamp = Path.Combine(pipelineCache, StampName);

        if (File.Exists(stamp) && File.ReadAllText(stamp).Trim() == translator)
        {
            return 0;
        }

        return PipelineJournalFile.Records(Path.Combine(pipelineCache, PipelineJournalFile.FileName))?.Count ?? 0;
    }

    /// <summary>Notes that the journal is built for <paramref name="translator"/>.</summary>
    public static void Stamp(string pipelineCache, string translator)
    {
        File.WriteAllText(Path.Combine(pipelineCache, StampName), translator);
    }

    /// <summary>The line <c>fivestar run</c> prints when a warm-up is due.</summary>
    public static string Offer(int entries, int processes)
    {
        var minutes = Math.Ceiling(Estimate(entries, processes).TotalMinutes).ToString(CultureInfo.InvariantCulture);

        return $"the translator changed since the last warm-up: the {entries} pipelines earlier boots used build at their first use in the game (20 to 60 s each); "
            + $"fivestar run --warm builds them first, about {minutes} min in {processes} process(es) on this machine (a run stopped with Ctrl+C resumes)";
    }

    /// <summary>
    /// The loader arguments that warm the journal for the boot of <paramref name="plan"/>: its shader folders, program
    /// cache and pipeline cache, and its loader options, which name the modes the programs are made in.
    /// </summary>
    public static IReadOnlyList<string> Arguments(RunPlan plan, string logFile)
    {
        var arguments = new List<string>
        {
            "--warm-pipelines", plan.Context.ShaderArchive,
            "--warm-used-only",
            "--shader-dir", plan.ProgramFolder,
            "--program-cache", plan.Context.ProgramCacheFolder,
            "--pipeline-cache", plan.Context.PipelineLibraryFolder,
            "--log", logFile,
        };

        if (plan.Options.PrebuiltShaders || plan.Options.TranslateAhead)
        {
            arguments.AddRange(["--translated-dir", plan.TranslatedFolder]);
        }

        var dxc = plan.Options.Tools.Option("dxc");

        if (dxc is not null)
        {
            arguments.AddRange(["--dxc", Path.GetFullPath(dxc)]);
        }

        arguments.AddRange(plan.Options.LoaderArguments);

        return arguments;
    }
}
