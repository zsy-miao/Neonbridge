// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Builds, from device creation on, every pipeline earlier boots used (the journal, <see cref="PipelineJournal"/>) that
/// the disk cache lacks, in first-use order, on <see cref="Jobs"/> threads of below-normal priority, so the game finds
/// it cached when it creates it instead of waiting 20 to 60 s for a driver compile at its first use (the world's
/// G-buffer pipelines at the cutscene). The journal's pipelines were queued only when the game created them, just
/// before it drew with them. A build here never waits in the compile budget: one that may not start now is given back
/// (<see cref="CompileBudget.GiveBack"/>) and tried in a later pass, so the game's own compiles always go first. The
/// idea of starting the last run's pipelines at device creation follows VIctory (THIRD_PARTY_NOTICES.md).
/// </summary>
internal sealed class JournalPrebuild
{
    /// <summary>Threads by default (<c>--prebuild-jobs</c>): none, until a boot A/B shows it helps without adding device removals.</summary>
    public const int DefaultJobs = 0;

    /// <summary>The pause before a pass over the pipelines a pass could not start.</summary>
    private static readonly TimeSpan RetryPause = TimeSpan.FromSeconds(30);

    /// <summary>Passes in a row that built nothing before the prebuild ends (the rest waits for programs never made).</summary>
    private const int IdlePasses = 20;

    private readonly PipelineFactory _factory;
    private readonly PipelineLibraryCache _cache;
    private readonly HashSet<string> _seen = new(StringComparer.Ordinal);
    private readonly ConcurrentQueue<WarmTarget> _later = new();
    private long _built;
    private long _cached;
    private long _notReady;
    private long _givenBack;
    private long _refused;
    private int _next;

    public JournalPrebuild(PipelineFactory factory, PipelineLibraryCache cache)
    {
        _factory = factory;
        _cache = cache;
    }

    /// <summary>Threads the prebuild runs on (<c>--prebuild-jobs</c>).</summary>
    public static int Jobs { get; set; } = DefaultJobs;

    /// <summary>The prebuild of this boot, for the sample line; null when none runs.</summary>
    public static JournalPrebuild? Current { get; private set; }

    /// <summary>"journal prebuild: B built, C cached, ..." for the sample line.</summary>
    public string Summary => $"journal prebuild: {Interlocked.Read(ref _built)} built, {Interlocked.Read(ref _cached)} cached already, {Interlocked.Read(ref _refused)} refused; last pass {Interlocked.Read(ref _givenBack)} given back to the game's compiles, {Interlocked.Read(ref _notReady)} with programs not made yet";

    /// <summary>Starts the prebuild of the journal in <paramref name="folder"/> when <see cref="Jobs"/> allows.</summary>
    public static void Start(PipelineFactory factory, PipelineLibraryCache? cache, string? folder, LoaderLog log)
    {
        if (Jobs <= 0 || cache is null || folder is null)
        {
            return;
        }

        var prebuild = new JournalPrebuild(factory, cache);
        Current = prebuild;
        var thread = new Thread(() => prebuild.Run(folder, log)) { IsBackground = true, Name = "fivestar journal prebuild", Priority = ThreadPriority.BelowNormal };
        thread.Start();
    }

    /// <summary>Builds one target on this thread; what became of it.</summary>
    public PrebuildOutcome Build(WarmTarget target)
    {
        var recipe = Prepare(target);

        if (recipe is null)
        {
            return Count(PrebuildOutcome.NotReady, ref _notReady);
        }

        string key = recipe.Stream.Key;

        if (_cache.Contains(key) || !See(key))
        {
            recipe.Stream.Dispose();

            return Count(PrebuildOutcome.Cached, ref _cached);
        }

        CompileBudget.GiveBack = true;
        var result = _factory.Build(recipe.Stream, DesktopDevice.PipelineStateIid, recipe.Names);

        if (result.Hr == HResult.Pending)
        {
            // Not started: the stream is still ours, and the key may be tried again.
            recipe.Stream.Dispose();
            Forget(key);

            return Count(PrebuildOutcome.GivenBack, ref _givenBack);
        }

        if (HResult.Failed(result.Hr) || result.Pipeline == 0)
        {
            return Count(PrebuildOutcome.Refused, ref _refused);
        }

        DesktopDevice.Release(result.Pipeline);

        return Count(PrebuildOutcome.Built, ref _built);
    }

    /// <summary>Passes over the journal, the first in first-use order, then every 30 s over the ones given back or not ready yet, until <see cref="IdlePasses"/> in a row build nothing.</summary>
    private void Run(string folder, LoaderLog log)
    {
        List<WarmTarget> targets;

        try
        {
            targets = PipelineJournal.Read(folder);
        }
        catch (IOException exception)
        {
            log.Warn($"[d3d12_xs] journal prebuild off: {exception.Message}");

            return;
        }

        log.Info($"  [d3d12_xs] journal prebuild: {targets.Count} pipelines of earlier boots, on {Jobs} thread(s) below the game's compiles");

        int idle = 0;

        while (targets.Count != 0 && idle < IdlePasses)
        {
            long before = Interlocked.Read(ref _built);
            Interlocked.Exchange(ref _notReady, 0);
            Interlocked.Exchange(ref _givenBack, 0);

            Pass(targets);
            log.Info($"  [d3d12_xs] {Summary}");
            targets = [.. _later];
            _later.Clear();
            idle = Interlocked.Read(ref _built) == before ? idle + 1 : 0;

            if (targets.Count != 0)
            {
                Thread.Sleep(RetryPause);
            }
        }
    }

    /// <summary>One pass over <paramref name="targets"/> on <see cref="Jobs"/> threads; the ones given back wait for the next.</summary>
    private void Pass(List<WarmTarget> targets)
    {
        _next = -1;
        var threads = Enumerable.Range(0, Math.Max(1, Jobs)).Select(_ => new Thread(() => Take(targets)) { IsBackground = true, Priority = ThreadPriority.BelowNormal }).ToList();
        threads.ForEach(thread => thread.Start());
        threads.ForEach(thread => thread.Join());
    }

    private void Take(List<WarmTarget> targets)
    {
        for (int i = Interlocked.Increment(ref _next); i < targets.Count; i = Interlocked.Increment(ref _next))
        {
            var outcome = Build(targets[i]);

            if (outcome == PrebuildOutcome.GivenBack || outcome == PrebuildOutcome.NotReady)
            {
                _later.Enqueue(targets[i]);
            }
        }
    }

    /// <summary>
    /// The recipe of a target; null while a program or its linked producer is not made yet. Programs are not made here:
    /// the boot's prefetch makes the journal's programs on the program workers, behind the game's own.
    /// </summary>
    private PipelineRecipe? Prepare(WarmTarget target)
    {
        try
        {
            if (target.Compute is not null && _factory.Ready(target.Compute))
            {
                return _factory.PrepareCompute(target.Compute);
            }

            if (target.Graphics is not null && _factory.Ready(target.Graphics))
            {
                return _factory.PrepareGraphics(target.Graphics);
            }

            return null;
        }
        catch (Exception exception) when (exception is InvalidDataException or NotSupportedException)
        {
            return null;
        }
    }

    private bool See(string key)
    {
        lock (_seen)
        {
            return _seen.Add(key);
        }
    }

    private void Forget(string key)
    {
        lock (_seen)
        {
            _seen.Remove(key);
        }
    }

    private static PrebuildOutcome Count(PrebuildOutcome outcome, ref long counter)
    {
        Interlocked.Increment(ref counter);

        return outcome;
    }
}
