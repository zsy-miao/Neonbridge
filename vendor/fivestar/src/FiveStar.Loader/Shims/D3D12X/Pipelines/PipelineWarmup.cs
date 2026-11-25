// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Builds pipelines into the disk cache ahead of the game (<c>--warm-pipelines</c>): each target becomes the desktop
/// stream the game's creation would make, is skipped when the cache has it (so a new run resumes) or when an earlier
/// run saw the driver refuse it, and is otherwise built with no time limit on up to <c>jobs</c> threads.
/// </summary>
internal sealed unsafe class PipelineWarmup
{
    /// <summary>Refused pipelines, one "key hresult names" line each, in the cache folder.</summary>
    public const string FailureFile = "warm-failures.txt";

    /// <summary>A build that takes longer than this is named in the log.</summary>
    private static readonly TimeSpan SlowBuild = TimeSpan.FromSeconds(20);

    private const long BytesPerMebibyte = 1L << 20;
    private const ulong BytesPerGibibyte = 1UL << 30;

    private static readonly TimeSpan ProgressInterval = TimeSpan.FromSeconds(30);

    private readonly PipelineFactory _factory;
    private readonly PipelineLibraryCache _cache;
    private readonly string _failurePath;
    private readonly LoaderLog _log;
    private readonly ConcurrentDictionary<string, byte> _seen = new(StringComparer.Ordinal);
    private readonly HashSet<string> _refused;
    private readonly object _failures = new();
    private readonly WarmTally _tally = new();
    private readonly BlockingCollection<(PipelineRecipe Recipe, string Key)> _heavy = [];
    private volatile bool _stopping;

    public PipelineWarmup(PipelineFactory factory, PipelineLibraryCache cache, string cacheFolder, LoaderLog log)
    {
        _factory = factory;
        _cache = cache;
        _failurePath = Path.Combine(cacheFolder, FailureFile);
        _log = log;
        _refused = ReadFailures(_failurePath);
    }

    /// <summary>What the run did so far.</summary>
    public WarmTally Tally => _tally;

    /// <summary>No new pipeline starts after this; the ones running finish.</summary>
    public void Stop()
    {
        _stopping = true;
    }

    /// <summary>Builds every target that is not cached; true when no target was left out by <see cref="Stop"/>.</summary>
    public bool Run(IReadOnlyList<WarmTarget> targets, int jobs, Action forget)
    {
        var clock = Stopwatch.StartNew();
        var options = new ParallelOptions { MaxDegreeOfParallelism = Math.Max(1, jobs) };
        var partitions = Partitioner.Create(targets, EnumerablePartitionerOptions.NoBuffering);
        using var progress = new Timer(_ => Report(targets.Count, clock), null, ProgressInterval, ProgressInterval);
        var heavy = new Thread(BuildHeavy) { IsBackground = true, Name = "fivestar warm-up heavy" };
        heavy.Start();

        Parallel.ForEach(partitions, options, (target, loop) =>
        {
            if (_stopping)
            {
                loop.Stop();

                return;
            }

            Warm(target);

            if (_tally.Done % WarmTally.ForgetInterval == 0)
            {
                forget();
            }
        });

        _heavy.CompleteAdding();
        heavy.Join();
        Report(targets.Count, clock);

        return !_stopping;
    }

    /// <summary>
    /// Builds the pipelines of <see cref="PipelineFactory.HeavyShaderBytes"/> or more one after another on their own
    /// thread, so the job threads go on with the others instead of waiting for the factory's lock.
    /// </summary>
    private void BuildHeavy()
    {
        foreach (var (recipe, key) in _heavy.GetConsumingEnumerable())
        {
            if (_stopping || !WarmMemoryGuard.RoomForHeavy())
            {
                Defer(recipe);

                continue;
            }

            Build(recipe, key);
        }
    }

    /// <summary>A heavy pipeline left to the game: the machine lacks the memory to build it now.</summary>
    private void Defer(PipelineRecipe recipe)
    {
        if (!_stopping)
        {
            _log.Warn($"warm-up: {recipe.Names} ({recipe.Stream.ShaderBytes / BytesPerMebibyte} MiB of DXIL) left out: less than {(WarmMemoryGuard.HeavyBytes + WarmMemoryGuard.ReserveBytes) / BytesPerGibibyte} GB of commit available");
            _tally.Count(WarmOutcome.Deferred);
        }

        recipe.Stream.Dispose();
    }

    private void Warm(WarmTarget target)
    {
        PipelineRecipe? recipe;

        try
        {
            recipe = Prepare(target);
        }
        catch (Exception exception) when (exception is InvalidDataException or NotSupportedException)
        {
            recipe = null;
        }

        if (recipe is null)
        {
            _tally.Count(WarmOutcome.Untranslated);

            return;
        }

        var key = recipe.Stream.Key;

        if (_cache.Contains(key) || _refused.Contains(key) || !_seen.TryAdd(key, 0))
        {
            recipe.Stream.Dispose();
            _tally.Count(WarmOutcome.Skipped);

            return;
        }

        if (recipe.Stream.ShaderBytes >= PipelineFactory.HeavyShaderBytes)
        {
            _heavy.Add((recipe, key));

            return;
        }

        Build(recipe, key);
    }

    private PipelineRecipe? Prepare(WarmTarget target)
    {
        // Programs neither translated nor in the program cache are made first, when there is a compiler.
        if (target.Compute is not null)
        {
            _factory.MakePrograms([target.Compute.Shader]);

            return _factory.PrepareCompute(target.Compute);
        }

        _factory.MakePrograms(target.Graphics!.Shaders);

        return _factory.PrepareGraphics(target.Graphics!);
    }

    private void Build(PipelineRecipe recipe, string key)
    {
        WarmMemoryGuard.WaitForRoom(() => _stopping);
        var started = Stopwatch.GetTimestamp();
        long shaderBytes = recipe.Stream.ShaderBytes;
        long before = CompileBudget.ProcessPrivateBytes;
        var result = _factory.Build(recipe.Stream, DesktopDevice.PipelineStateIid, recipe.Names);

        var took = Stopwatch.GetElapsedTime(started);
        long grown = CompileBudget.ProcessPrivateBytes - before;
        long estimate = CompileBudget.Estimate(shaderBytes);

        // The private growth over the build counts the other compiles running meanwhile too: an upper bound of this one's.
        if (took > SlowBuild || grown > estimate)
        {
            _log.Info($"  {took.TotalSeconds:F0} s: {recipe.Names} ({shaderBytes / BytesPerMebibyte} MiB of DXIL, estimate {(double)estimate / BytesPerGibibyte:F0} GB, private {(double)grown / BytesPerGibibyte:+0.0;-0.0} GB over the build)");
        }

        if (HResult.Failed(result.Hr) || result.Pipeline == 0)
        {
            _tally.Count(WarmOutcome.Refused);
            RecordFailure(key, result.Hr, recipe.Names);

            return;
        }

        DesktopDevice.Release(result.Pipeline);
        _tally.Count(WarmOutcome.Built);
        _tally.AddBuildTime(took);
    }

    private void RecordFailure(string key, int hr, string names)
    {
        lock (_failures)
        {
            File.AppendAllLines(_failurePath, [$"{key} {HResult.Format(hr)} {names}"]);
        }
    }

    private void Report(int total, Stopwatch clock)
    {
        _log.Info($"  warm-up {Progress(_tally.Done, total, clock.Elapsed)}: {_tally.Summary}; {_cache.Summary}; {CompileBudget.Summary}; {CompileBudget.Refusals}; {Diagnostics.MemorySnapshot.Summary()}; {Diagnostics.WriteCombinedMemory.Summary()}");
    }

    /// <summary>Done of total after the time, what is left, the rate so far and the time left at that rate.</summary>
    public static string Progress(int done, int total, TimeSpan elapsed)
    {
        int left = Math.Max(0, total - done);
        double rate = done / Math.Max(elapsed.TotalSeconds, 1);
        string remaining = "unknown";

        if (rate > 0)
        {
            remaining = TimeSpan.FromSeconds(left / rate).ToString(@"hh\:mm\:ss", System.Globalization.CultureInfo.InvariantCulture);
        }

        return string.Create(System.Globalization.CultureInfo.InvariantCulture, $"{done}/{total} after {elapsed:hh\\:mm\\:ss}, {left} left at {rate:F2}/s (about {remaining})");
    }

    private static HashSet<string> ReadFailures(string path)
    {
        if (!File.Exists(path))
        {
            return new HashSet<string>(StringComparer.Ordinal);
        }

        return File.ReadLines(path).Select(line => line.Split(' ')[0]).ToHashSet(StringComparer.Ordinal);
    }
}
