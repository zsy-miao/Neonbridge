// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Host;

/// <summary>
/// <c>--warm-processes N</c>: the warm-up split over N processes of this executable (<c>--warm-shard i/N</c>), each
/// building every N-th pipeline into the same disk cache, as XCL fills the driver cache from several processes. One
/// process admits one compile of more than 2 MB at a time (the compile budget's heavy rule held the 4-thread warm-up
/// to 0.8 pipelines a second, 2,651 refusals in 28 minutes, while its compiles grew private bytes by 1.5 GB); N processes
/// admit N. Pipelines of 4 MiB of DXIL or more are left to a last pass in this process, one at a time, so no two
/// such compiles meet. Each shard skips what the cache holds, so a stopped run resumes.
/// </summary>
internal static partial class WarmShards
{
    /// <summary>Memory per warm-up process by default: one per 16 GB of RAM, 1 to <see cref="MaxProcesses"/>.</summary>
    private const ulong BytesPerProcess = 16UL << 30;
    private const int MaxProcesses = 4;

    /// <summary>The parts of `i/n`.</summary>
    private const int ShardParts = 2;

    private static readonly TimeSpan ProgressInterval = TimeSpan.FromSeconds(30);

    /// <summary>The options a parent passes down itself, each with its value.</summary>
    private static readonly HashSet<string> ParentOptions = new(StringComparer.Ordinal) { "--warm-processes", "--warm-shard", "--warm-jobs", "--max-driver-compiles", "--log" };

    /// <summary>The warm-up processes for a machine of <paramref name="totalPhysical"/> bytes of RAM.</summary>
    public static int ProcessesFor(ulong totalPhysical)
    {
        return (int)Math.Clamp(totalPhysical / BytesPerProcess, 1, MaxProcesses);
    }

    /// <summary>Whether target <paramref name="position"/> belongs to shard <paramref name="shard"/> of <paramref name="count"/>.</summary>
    public static bool InShard(int position, int shard, int count)
    {
        return count <= 1 || position % count == shard;
    }

    /// <summary>"i/n" as (shard, count); null when it is not one.</summary>
    public static (int Shard, int Count)? ParseShard(string text)
    {
        var parts = text.Split('/');

        if (parts.Length != ShardParts || !int.TryParse(parts[0], CultureInfo.InvariantCulture, out int shard) || !int.TryParse(parts[1], CultureInfo.InvariantCulture, out int count) || count < 1 || shard < 0 || shard >= count)
        {
            return null;
        }

        return (shard, count);
    }

    /// <summary>The arguments of shard <paramref name="shard"/>: the parent's without what it sets per shard, then those.</summary>
    public static List<string> ChildArguments(IReadOnlyList<string> parent, int shard, int count, int jobs, int compiles, string? log)
    {
        var arguments = new List<string>();

        for (int i = 0; i < parent.Count; i++)
        {
            if (ParentOptions.Contains(parent[i]))
            {
                i++;

                continue;
            }

            arguments.Add(parent[i]);
        }

        arguments.AddRange(["--warm-shard", $"{shard}/{count}", "--warm-jobs", jobs.ToString(CultureInfo.InvariantCulture), "--max-driver-compiles", compiles.ToString(CultureInfo.InvariantCulture)]);

        if (log is not null)
        {
            arguments.AddRange(["--log", $"{log}.shard{shard}.log"]);
        }

        return arguments;
    }

    /// <summary>
    /// Runs the shards and waits for them, logging each one's lines with its number and every 30 s the run's progress;
    /// true when every shard ended with success.
    /// </summary>
    public static bool Run(HostOptions options, int count, LoaderLog log)
    {
        int compiles = Math.Max(1, CompileBudget.MaxCompilesFor(WarmMemoryGuard.TotalPhysical()) / count);
        int jobs = Math.Max(1, (options.WarmJobs ?? compiles * count) / count);
        var parent = Environment.GetCommandLineArgs().Skip(1).ToList();
        var progress = new (int Done, int Total)[count];
        var clock = Stopwatch.StartNew();
        log.Info($"warm-up: {count} processes of {jobs} threads and {compiles} driver compiles each; pipelines of 4 MiB of DXIL or more are left to a last pass here");

        var shards = Enumerable.Range(0, count).Select(shard => Start(ChildArguments(parent, shard, count, jobs, compiles, options.LogFile), shard, progress, log)).ToList();
        using var timer = new Timer(_ => Report(progress, clock, log), null, ProgressInterval, ProgressInterval);
        shards.ForEach(process => process.WaitForExit());
        Report(progress, clock, log);

        return shards.All(process => process.ExitCode == LoaderHost.Success);
    }

    /// <summary>"warm-up shards: D/T after ..., ..." over every shard's last progress line.</summary>
    public static string Overall((int Done, int Total)[] progress, TimeSpan elapsed)
    {
        int done = progress.Sum(shard => shard.Done);
        int total = progress.Sum(shard => shard.Total);

        return $"warm-up shards: {PipelineWarmup.Progress(done, total, elapsed)}";
    }

    /// <summary>Notes a shard's progress line ("warm-up D/T after ...") in <paramref name="progress"/>.</summary>
    public static void Note(string line, int shard, (int Done, int Total)[] progress)
    {
        var match = ProgressLine().Match(line);

        if (match.Success)
        {
            progress[shard] = (int.Parse(match.Groups["done"].Value, CultureInfo.InvariantCulture), int.Parse(match.Groups["total"].Value, CultureInfo.InvariantCulture));
        }
    }

    private static Process Start(List<string> arguments, int shard, (int Done, int Total)[] progress, LoaderLog log)
    {
        var info = new ProcessStartInfo(Environment.ProcessPath!) { RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false };
        arguments.ForEach(info.ArgumentList.Add);
        var process = Process.Start(info)!;

        DataReceivedEventHandler forward = (_, line) =>
        {
            if (line.Data is null)
            {
                return;
            }

            Note(line.Data, shard, progress);
            log.Info($"[shard {shard}] {line.Data}");
        };

        process.OutputDataReceived += forward;
        process.ErrorDataReceived += forward;
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();

        return process;
    }

    private static void Report((int Done, int Total)[] progress, Stopwatch clock, LoaderLog log)
    {
        log.Info(Overall(progress, clock.Elapsed));
    }

    [GeneratedRegex(@"warm-up (?<done>\d+)/(?<total>\d+) after")]
    private static partial Regex ProgressLine();
}
