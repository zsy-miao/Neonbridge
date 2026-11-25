// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Pipeline creations the driver takes too long for. The game creates pipelines in deferred jobs and waits for
/// all of them before it finishes a frame (docs/d3d12x-pipelines.md); the console builds them at once, the desktop
/// driver compiles some translated programs for minutes. A creation that has not returned within the budget hands the
/// game a <see cref="StandInPipelineObject"/> and goes on in the background; the stand-in becomes the desktop pipeline
/// when the driver is done. Until then the command lists skip the work it would run, unless the stand-in is named by
/// <c>--wait-pipelines</c> (<see cref="Waits"/>): its first use then waits for the pipeline.
/// </summary>
internal sealed unsafe class StandInPipelines
{
    /// <summary>A build that takes longer than this is named in the log when it ends.</summary>
    private static readonly TimeSpan SlowBuild = TimeSpan.FromMinutes(1);

    /// <summary>How long the first use of a small compute stand-in waits for its pipeline (made on that thread if queued).</summary>
    public static readonly TimeSpan SmallBudget = TimeSpan.FromMilliseconds(250);

    /// <summary>Pipelines whose skipped work is logged by name.</summary>
    private const int SkipsLogged = 64;

    /// <summary>Names listed in the summary's count of work skipped before later work of the same list ran.</summary>
    private const int SkipsShown = 4;

    private readonly ConcurrentDictionary<nint, string> _names = new();
    private readonly ConcurrentDictionary<nint, StandInUse> _uses = new();
    private readonly ConcurrentDictionary<string, long> _skippedByName = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, long> _skippedBefore = new(StringComparer.Ordinal);
    private readonly HashSet<nint> _small = [];
    private int _smallMade;
    private int _smallNotReady;
    private int _smallProgramsInline;
    private int _smallOverBudget;
    private readonly HashSet<nint> _waited = [];
    private int _waitedFor;
    private int _waitsPastLimit;

    private readonly object _gate = new();
    private readonly HashSet<nint> _live = [];
    private readonly Dictionary<nint, Action> _waiting = [];
    private readonly Dictionary<nint, StandInWork> _work = [];
    private readonly TimeSpan _budget;
    private readonly PipelineBuildQueue _builds;
    private readonly PipelineBuildQueue _heavyBuilds;
    private readonly OnceLog _warnings;
    private readonly nint* _vtable;
    private readonly GCHandle _self;
    private int _created;
    private int _ready;
    private int _deferred;
    private int _used;
    private int _onDemand;
    private long _skipped;
    private long _skippedAfterChain;
    private long _recorded;

    /// <param name="budget">How long a creation may take before the game gets a stand-in.</param>
    /// <param name="builds">The threads that call the driver (started by the caller).</param>
    /// <param name="heavyBuilds">The same queue, or the thread for pipelines of <see cref="PipelineFactory.HeavyShaderBytes"/> or more, which wait for more memory without holding up the others.</param>
    /// <param name="warnings">Where stand-ins and late pipelines are reported, once per name.</param>
    public StandInPipelines(TimeSpan budget, PipelineBuildQueue builds, PipelineBuildQueue heavyBuilds, OnceLog warnings)
    {
        _budget = budget;
        _builds = builds;
        _heavyBuilds = heavyBuilds;
        _warnings = warnings;
        _vtable = StandInPipelineMethods.CreateVtable();
        _self = GCHandle.Alloc(this, GCHandleType.Weak);
    }

    /// <summary>The stand-ins a command list waits for at their first use instead of skipping their work; null for none.</summary>
    public PipelineWaits? Waits { get; init; } = PipelineWaits.Current;

    /// <summary>Stand-ins handed out, how many have their pipeline, the work skipped meanwhile and the work recorded.</summary>
    public string Summary => $"{Volatile.Read(ref _created)} stand-in pipelines ({Volatile.Read(ref _deferred)} built on first use, {Volatile.Read(ref _used)} of them used, {Volatile.Read(ref _onDemand)} with programs made on demand, {Volatile.Read(ref _ready)} built since, {Count(queue => queue.Backlog)} queued, {Count(queue => queue.WaitingForMemory)} build threads waiting for memory), {Interlocked.Read(ref _skipped)} draws and dispatches skipped, {Interlocked.Read(ref _skippedAfterChain)} dispatches and ExecuteIndirects skipped after a skipped one of their list,{Interlocked.Read(ref _recorded)} recorded, {SkippedBeforeSummary()}, {Volatile.Read(ref _smallMade)} small compute pipelines made at first use ({Volatile.Read(ref _smallProgramsInline)} with programs made inline or found ready,{Volatile.Read(ref _smallNotReady)} not waited for: programs not made, {Volatile.Read(ref _smallOverBudget)} over the first-use budget), {Volatile.Read(ref _waitedFor)} waited for by name ({Volatile.Read(ref _waitsPastLimit)} past the limit)";

    /// <summary>
    /// Runs <paramref name="create"/> and writes its pipeline to <paramref name="result"/>; when it takes longer than
    /// the budget, or when earlier creations still wait for a thread, writes a stand-in instead and returns S_OK.
    /// <paramref name="built"/> is told the desktop pipeline when there is one; <paramref name="heavy"/> builds on the heavy thread.
    /// </summary>
    public int Create(Func<PipelineResult> create, nint* result, string what, Action<nint>? built, bool heavy = false, bool small = false)
    {
        var budget = _budget;
        var queue = _builds;

        if (heavy)
        {
            queue = _heavyBuilds;
        }

        if (queue.Backlog > 0)
        {
            budget = TimeSpan.Zero;
        }

        long started = Stopwatch.GetTimestamp();
        var queued = queue.Queue(create, urgent: false);
        var build = queued.Done.Task;
        bool done;

        using (BlockingWaits.Enter("pipeline creation budget"))
        {
            done = build.Wait(budget);
        }

        if (done)
        {
            *result = build.Result.Pipeline;

            if (build.Result.Pipeline != 0)
            {
                built?.Invoke(build.Result.Pipeline);
            }

            return build.Result.Hr;
        }

        nint standIn = NewStandIn(what, small);
        queued.Use = Use(standIn);

        Track(standIn, new StandInWork { Creation = (queued, queue) });
        build.ContinueWith(task => Complete(standIn, task.Result, what, started, built), CancellationToken.None, TaskContinuationOptions.None, TaskScheduler.Default);
        Interlocked.Increment(ref _created);
        _warnings.Warn("standin", $"[d3d12_xs] pipeline {what} not built in time: the game gets a stand-in, its draws and dispatches are skipped until the driver is done (counted in the sample summary)");
        *result = standIn;

        return HResult.Ok;
    }

    /// <summary>
    /// Writes a stand-in whose build starts only when a command list first sets it (<see cref="Desktop"/>): the game
    /// creates tens of thousands of pipelines while loading and draws with a small part of them. <paramref name="used"/>
    /// runs then, before the build is queued.
    /// </summary>
    public int Deferred(Func<PipelineResult> create, nint* result, string what, Action<nint>? built, bool heavy, Action used, bool small = false)
    {
        nint standIn = NewStandIn(what, small);
        ((StandInPipelineObject*)standIn)->Deferred = 1;

        lock (_gate)
        {
            _waiting[standIn] = () =>
            {
                used();
                var queue = heavy ? _heavyBuilds : _builds;
                long started = Stopwatch.GetTimestamp();
                var queued = queue.Queue(create, urgent: true, Use(standIn));
                Track(standIn, new StandInWork { Creation = (queued, queue) });
                queued.Done.Task.ContinueWith(task => Complete(standIn, task.Result, what, started, built), CancellationToken.None, TaskContinuationOptions.None, TaskScheduler.Default);
            };
        }

        Interlocked.Increment(ref _created);
        Interlocked.Increment(ref _deferred);
        *result = standIn;

        return HResult.Ok;
    }

    /// <summary>
    /// Writes a stand-in whose programs are made on demand (<see cref="OnDemandPrograms"/>): <paramref name="programs"/>
    /// starts making them (given the stand-in's use when a command list waits, null for a prefetch) and, once they are ready, <paramref name="prepare"/> gives
    /// the build, queued as in <see cref="Create"/>; null leaves the stand-in unbuilt. With <paramref name="used"/> this
    /// starts when a command list first sets the stand-in (as <see cref="Deferred"/>), else now. A small compute
    /// stand-in has <paramref name="programsNow"/>, which makes its programs on the thread of its first use.
    /// </summary>
    public int Waiting(Func<StandInUse?, Task> programs, Func<PendingBuild?> prepare, nint* result, Action? used, string what = "", Func<bool>? programsNow = null)
    {
        nint standIn = NewStandIn(what, programsNow is not null);
        Track(standIn, new StandInWork { Programs = programs, ProgramsNow = programsNow });
        Interlocked.Increment(ref _created);
        Interlocked.Increment(ref _onDemand);

        void Start(bool firstUse)
        {
            long started = Stopwatch.GetTimestamp();
            StandInUse? use = null;

            if (firstUse)
            {
                use = Use(standIn);
            }

            programs(use).ContinueWith(_ => Queue(standIn, prepare, started), CancellationToken.None, TaskContinuationOptions.None, TaskScheduler.Default);
        }

        if (used is null)
        {
            Start(firstUse: false);
        }
        else
        {
            ((StandInPipelineObject*)standIn)->Deferred = 1;
            Interlocked.Increment(ref _deferred);

            lock (_gate)
            {
                _waiting[standIn] = () =>
                {
                    used();
                    Start(firstUse: true);
                };
            }
        }

        *result = standIn;

        return HResult.Ok;
    }

    /// <summary>Queues the build of a stand-in whose programs are ready; logs a build that cannot be made.</summary>
    private void Queue(nint standIn, Func<PendingBuild?> prepare, long started)
    {
        PendingBuild? build;

        try
        {
            build = prepare();
        }
        catch (Exception exception) when (exception is InvalidDataException or NotSupportedException)
        {
            _warnings.Warn($"prepare-{exception.Message}", $"[d3d12_xs] pipeline with programs made on demand not built: {exception.Message}");

            return;
        }

        if (build is null)
        {
            return;
        }

        var queue = build.Heavy ? _heavyBuilds : _builds;
        PipelineBuild queued;

        lock (_gate)
        {
            // A stand-in a command list set already goes first; one set later is promoted then (Promote).
            queued = queue.Queue(build.Create, Volatile.Read(ref ((StandInPipelineObject*)standIn)->Used) == 1, Use(standIn));

            if (_work.TryGetValue(standIn, out var work))
            {
                work.Creation = (queued, queue);
            }
        }

        queued.Done.Task.ContinueWith(task => Complete(standIn, task.Result, build.What, started, build.Built), CancellationToken.None, TaskContinuationOptions.None, TaskScheduler.Default);
    }

    private void Track(nint standIn, StandInWork work)
    {
        lock (_gate)
        {
            _work[standIn] = work;
        }
    }

    /// <summary>
    /// A command list set <paramref name="standIn"/> for the first time: its programs and its creation go before all
    /// speculative work (pipelines built at creation from the journal, prefetched programs).
    /// </summary>
    private void Promote(nint standIn)
    {
        StandInWork? work;

        lock (_gate)
        {
            _work.TryGetValue(standIn, out work);
        }

        if (work is null)
        {
            return;
        }

        if (work.Programs is not null)
        {
            _ = work.Programs(Use(standIn));
        }

        lock (_gate)
        {
            var creation = work.Creation;

            if (creation.HasValue)
            {
                creation.Value.Queue.Promote(creation.Value.Build);
            }
        }
    }

    /// <summary>
    /// The desktop pipeline behind <paramref name="pipeline"/>: the built pipeline of a stand-in (0 and
    /// <paramref name="pending"/> while the driver works), any other pointer as it is.
    /// </summary>
    [HotPath]
    public nint Desktop(nint pipeline, out bool pending)
    {
        pending = false;

        if (pipeline == 0 || *(nint*)pipeline != (nint)_vtable)
        {
            return pipeline;
        }

        var standIn = (StandInPipelineObject*)pipeline;
        nint desktop = Volatile.Read(ref standIn->Desktop);
        pending = desktop == 0;

        // Used first, so a deferred stand-in started below queues its creation as urgent.
        bool firstUse = pending && Volatile.Read(ref standIn->Used) == 0 && Interlocked.Exchange(ref standIn->Used, 1) == 0;

        if (firstUse)
        {
            Promote(pipeline);
        }

        if (pending && Volatile.Read(ref standIn->Deferred) == 1 && Interlocked.Exchange(ref standIn->Deferred, 0) == 1)
        {
            Start(pipeline);
        }

        if (firstUse && IsSmall(pipeline))
        {
            desktop = MakeWithinBudget(pipeline);
            pending = desktop == 0;
        }

        if (pending && Waits is not null)
        {
            desktop = WaitIfNamed(pipeline, Waits);
            pending = desktop == 0;
        }

        return desktop;
    }

    /// <summary>
    /// The first use of a stand-in <paramref name="waits"/> names: this thread waits for its pipeline (made first, as the
    /// first use promoted it) up to the limit, the game's hang watchdog paused meanwhile. Past the limit the stand-in
    /// stays and is not waited for again; its work is skipped as without the option. A heavy mesh pipeline's compile can
    /// take minutes and several gigabytes, so the limit bounds the frame it holds.
    /// </summary>
    private nint WaitIfNamed(nint pipeline, PipelineWaits waits)
    {
        string name = Name(pipeline);

        if (!waits.Matches(name) || !FirstWait(pipeline))
        {
            return 0;
        }

        var standIn = (StandInPipelineObject*)pipeline;
        long started = Stopwatch.GetTimestamp();
        long deadline = started + (long)(waits.Limit.TotalSeconds * Stopwatch.Frequency);

        using (WatchdogPause.Enter())
        using (BlockingWaits.Enter("a pipeline named by --wait-pipelines"))
        {
            while (Volatile.Read(ref standIn->Desktop) == 0 && Stopwatch.GetTimestamp() < deadline)
            {
                Thread.Sleep(1);
            }
        }

        nint desktop = Volatile.Read(ref standIn->Desktop);
        Waited(name, desktop != 0, Stopwatch.GetElapsedTime(started));

        return desktop;
    }

    private bool FirstWait(nint pipeline)
    {
        lock (_gate)
        {
            return _waited.Add(pipeline);
        }
    }

    private void Waited(string name, bool built, TimeSpan took)
    {
        Interlocked.Increment(ref _waitedFor);

        if (built)
        {
            _warnings.Warn($"waited-{name}", $"[d3d12_xs] waited {took.TotalSeconds:F1} s for pipeline {name} at its first use (--wait-pipelines)");

            return;
        }

        Interlocked.Increment(ref _waitsPastLimit);
        _warnings.Warn($"wait-limit-{name}", $"[d3d12_xs] pipeline {name} not built within {took.TotalSeconds:F0} s of its first use (--wait-pipelines): its work is skipped until it is");
    }

    /// <summary>
    /// <see cref="MakeNow"/> while the game's threads have first-use time left (<see cref="FirstUseBudget"/>), the time
    /// it took spent from it; otherwise the stand-in stays.
    /// </summary>
    private nint MakeWithinBudget(nint pipeline)
    {
        if (!FirstUseBudget.Available())
        {
            Interlocked.Increment(ref _smallOverBudget);

            return 0;
        }

        long started = Stopwatch.GetTimestamp();
        nint desktop = MakeNow(pipeline);
        FirstUseBudget.Spend(Stopwatch.GetTimestamp() - started);

        return desktop;
    }

    /// <summary>Queues the build of a deferred stand-in on its first use.</summary>
    private void Start(nint standIn)
    {
        Action? start;

        lock (_gate)
        {
            _waiting.Remove(standIn, out start);
        }

        Interlocked.Increment(ref _used);
        start?.Invoke();
    }

    /// <summary>Counts a draw or dispatch a command list recorded (draws per frame: its growth over the presents).</summary>
    [HotPath]
    public void Recorded(nint skippedBefore)
    {
        Interlocked.Increment(ref _recorded);

        if (skippedBefore != 0)
        {
            CountSkippedBefore(skippedBefore);
        }
    }

    /// <summary>Counts a dispatch or ExecuteIndirect dropped because an earlier one of its recording was.</summary>
    [HotPath]
    public void SkippedAfterChain()
    {
        Interlocked.Increment(ref _skippedAfterChain);
    }

    /// <summary>Counts a dispatch a command list dropped because its pipeline is still a stand-in.</summary>
    [HotPath]
    public void Skipped(nint standIn)
    {
        Interlocked.Increment(ref _skipped);
        CountSkipped(standIn);

        if (_uses.TryGetValue(standIn, out var use))
        {
            use.Skipped(StandInUse.Frame);
        }
    }

    /// <summary>How much the game misses <paramref name="standIn"/>, kept while it lives.</summary>
    private StandInUse Use(nint standIn)
    {
        return _uses.GetOrAdd(standIn, static _ => new StandInUse());
    }

    /// <summary>The last reference went: releases the desktop pipeline (or lets the build release it) and frees the stand-in.</summary>
    public void Retire(StandInPipelineObject* standIn)
    {
        nint desktop;

        lock (_gate)
        {
            _live.Remove((nint)standIn);
            _waiting.Remove((nint)standIn);
            _work.Remove((nint)standIn);
            _small.Remove((nint)standIn);
            _waited.Remove((nint)standIn);
            desktop = standIn->Desktop;
        }

        _names.TryRemove((nint)standIn, out _);
        _uses.TryRemove((nint)standIn, out _);
        ComObject.Release(desktop);
        NativeMemory.Free(standIn);
    }

    /// <summary>
    /// A stand-in that never gets a pipeline, for a creation that failed: the console never fails one and the game
    /// reads a compute shader's pipeline without a null check (0x14248c30d, the shader object's +0x120).
    /// </summary>
    public nint Unbuilt()
    {
        Interlocked.Increment(ref _created);

        return NewStandIn();
    }

    /// <summary>Counts skipped work by pipeline name; the first <see cref="SkipsLogged"/> names are logged.</summary>
    private void CountSkipped(nint standIn)
    {
        var name = Name(standIn);
        var count = _skippedByName.AddOrUpdate(name, 1, static (_, value) => value + 1);

        if (count == 1 && _skippedByName.Count <= SkipsLogged)
        {
            _warnings.Warn($"skipped-{name}", $"[d3d12_xs] work of pipeline {name} skipped: it is a stand-in until the driver is done");
        }
    }

    /// <summary>A list ran work after skipping the stand-in's: its consumers read what the skipped work did not write.</summary>
    private void CountSkippedBefore(nint standIn)
    {
        _skippedBefore.AddOrUpdate(Name(standIn), 1, static (_, value) => value + 1);
    }

    private string SkippedBeforeSummary()
    {
        // A snapshot: LINQ over the dictionary itself sizes an array by Count, then copies while lists add entries
        // (ArgumentException on the sampler thread, which ended a boot at 4,200 presents).
        var entries = _skippedBefore.ToArray();
        var total = entries.Sum(entry => entry.Value);
        var top = string.Join(", ", entries.OrderByDescending(entry => entry.Value).Take(SkipsShown).Select(entry => $"{entry.Key} x{entry.Value}"));

        return $"{total} lists ran work after a skipped one ({top})";
    }

    /// <summary>Where a stand-in's build is, for the census: built since, waiting for its programs, still building, or released.</summary>
    public string State(nint standIn)
    {
        lock (_gate)
        {
            if (!_live.Contains(standIn))
            {
                return "released";
            }

            if (Volatile.Read(ref ((StandInPipelineObject*)standIn)->Desktop) != 0)
            {
                return "built since";
            }

            if (_waiting.ContainsKey(standIn))
            {
                return "waiting for its programs";
            }

            return "still building";
        }
    }

    /// <summary>The programs of a stand-in, or "unnamed".</summary>
    public string Name(nint standIn)
    {
        return _names.GetValueOrDefault(standIn, "unnamed");
    }

    private bool IsSmall(nint standIn)
    {
        lock (_gate)
        {
            return _small.Contains(standIn);
        }
    }

    /// <summary>
    /// The first use of a small compute stand-in: its programs, when not ready, are made on this thread, and its
    /// creation runs on this thread (claimed from the queue) within <see cref="SmallBudget"/>; the desktop pipeline, or
    /// 0 when the programs cannot be made here or the budget ran out, and the stand-in stays as before.
    /// </summary>
    private nint MakeNow(nint pipeline)
    {
        var standIn = (StandInPipelineObject*)pipeline;
        var deadline = Stopwatch.GetTimestamp() + (long)(SmallBudget.TotalSeconds * Stopwatch.Frequency);
        bool madeHere = false;
        using var wait = BlockingWaits.Enter("small compute pipeline at first use");

        while (Stopwatch.GetTimestamp() < deadline)
        {
            nint desktop = Volatile.Read(ref standIn->Desktop);

            if (desktop != 0)
            {
                Interlocked.Increment(ref _smallMade);

                return desktop;
            }

            StandInWork? work;

            lock (_gate)
            {
                _work.TryGetValue(pipeline, out work);
            }

            var creation = work?.Creation;

            // Programs not ready: made here once (a small program takes milliseconds, the workers' queue a minute or
            // more while loading), then the creation is queued within moments. Never slept on: the Subrender workers
            // of a loading-screen frame meeting hundreds of such stand-ins slept 250 ms on each, and the game's hang
            // watchdog ended the story load (docs/debugging.md#hang-watchdog).
            if (!creation.HasValue)
            {
                if (!madeHere && !MakePrograms(work, ref deadline))
                {
                    break;
                }

                madeHere = true;
                Thread.Sleep(1);

                continue;
            }

            // Never on this thread while the build would wait for memory (Build waits without a limit), nor while other
            // compiles wait in the compile budget: the game's thread would queue behind them for seconds.
            if (CompileBudget.Waiting == 0 && WarmMemoryGuard.RoomForBuild(0))
            {
                creation.Value.Queue.RunNow(creation.Value.Build);
            }

            Thread.Sleep(1);
        }

        return Volatile.Read(ref standIn->Desktop);
    }

    /// <summary>
    /// Makes a small stand-in's programs on this thread; true when they are ready, and the budget for its creation
    /// starts again (<paramref name="deadline"/>).
    /// </summary>
    private bool MakePrograms(StandInWork? work, ref long deadline)
    {
        if (work?.ProgramsNow is null)
        {
            Interlocked.Increment(ref _smallNotReady);

            return false;
        }

        bool ready;

        using (BlockingWaits.Enter("small compute programs made inline"))
        {
            ready = work.ProgramsNow();
        }

        if (!ready)
        {
            Interlocked.Increment(ref _smallNotReady);

            return false;
        }

        Interlocked.Increment(ref _smallProgramsInline);
        deadline = Stopwatch.GetTimestamp() + (long)(SmallBudget.TotalSeconds * Stopwatch.Frequency);

        return true;
    }

    /// <summary>A count over the build queues (one when the heavy pipelines share the others' threads).</summary>
    private int Count(Func<PipelineBuildQueue, int> count)
    {
        if (ReferenceEquals(_builds, _heavyBuilds))
        {
            return count(_builds);
        }

        return count(_builds) + count(_heavyBuilds);
    }

    private nint NewStandIn(string what = "", bool small = false)
    {
        var standIn = (StandInPipelineObject*)NativeMemory.AllocZeroed((nuint)sizeof(StandInPipelineObject));
        standIn->Vtable = (nint)_vtable;
        standIn->References = 1;
        standIn->Owner = GCHandle.ToIntPtr(_self);
        _uses[(nint)standIn] = new StandInUse();

        lock (_gate)
        {
            _live.Add((nint)standIn);

            if (small)
            {
                _small.Add((nint)standIn);
            }
        }

        if (what.Length > 0)
        {
            _names[(nint)standIn] = what;
        }

        return (nint)standIn;
    }

    private void Complete(nint standIn, PipelineResult built, string what, long started, Action<nint>? notify)
    {
        var took = Stopwatch.GetElapsedTime(started);

        if (HResult.Failed(built.Hr) || built.Pipeline == 0)
        {
            lock (_gate)
            {
                _work.Remove(standIn);
            }

            _warnings.Warn($"late-{what}", $"[d3d12_xs] pipeline {what} failed after {took.TotalSeconds:F0} s: {HResult.Format(built.Hr)}; its draws and dispatches stay skipped");

            return;
        }

        notify?.Invoke(built.Pipeline);

        lock (_gate)
        {
            _work.Remove(standIn);

            if (_live.Contains(standIn))
            {
                Volatile.Write(ref ((StandInPipelineObject*)standIn)->Desktop, built.Pipeline);
                Interlocked.Increment(ref _ready);

                if (took > SlowBuild)
                {
                    _warnings.Warn($"late-{what}", $"[d3d12_xs] pipeline {what} built after {took.TotalSeconds:F0} s");
                }

                return;
            }
        }

        ComObject.Release(built.Pipeline);
    }
}
