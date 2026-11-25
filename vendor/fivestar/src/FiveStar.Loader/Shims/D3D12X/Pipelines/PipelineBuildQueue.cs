// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// A fixed set of background threads that run desktop pipeline creations in order, so creations the game no longer
/// waits for (<see cref="StandInPipelines"/>) never take more driver compiler threads than these. A thread starts a
/// creation only while the machine has the memory for it: the driver's compiles of a story load's backlog took a
/// 64 GB machine to its commit limit, which ended the process (docs/d3d12x-pipelines.md#stand-ins). Urgent creations,
/// those of pipelines a command list has set, start before all others; a queued creation can be made urgent later.
/// Only half the threads take the others, and those wait in the driver compile budget while an urgent one does: at a
/// warm boot the journal's creations held the budget the game's first uses needed. Among urgent creations, the one
/// whose stand-in the game misses most goes first (<see cref="StandInUse.Score"/>: skipped in the most frames, then
/// the most draws), so what is drawn every frame is built before what was drawn once; a cold boot queued about 1,500.
/// </summary>
internal sealed class PipelineBuildQueue
{
    private const int PauseMilliseconds = 2000;
    private const int GiveBackPauseMilliseconds = 250;

    // One thread in this many also takes creations no command list waits for.
    private const int SpeculativeShare = 2;

    /// <summary>For <see cref="PipelineBuildQueue(int, Func{int, bool})"/>: creations start whenever a thread is free.</summary>
    public static readonly Func<int, bool> AnyTime = _ => true;

    private readonly object _gate = new();
    private readonly List<PipelineBuild> _urgent = [];
    private readonly Queue<PipelineBuild> _builds = [];
    private readonly Func<int, bool> _room;
    private int _waiting;
    private int _running;

    /// <param name="workers">Threads that call the driver; at least one.</param>
    /// <param name="room">Whether a creation may start now, given the creations running; polled until it says so.</param>
    public PipelineBuildQueue(int workers, Func<int, bool> room)
    {
        Workers = Math.Max(1, workers);
        _room = room;
    }

    /// <summary>The number of threads that run creations.</summary>
    public int Workers { get; }

    /// <summary>Creations queued and not yet started.</summary>
    public int Backlog
    {
        get
        {
            lock (_gate)
            {
                return _urgent.Count + _builds.Count;
            }
        }
    }

    /// <summary>Threads waiting for memory before their next creation.</summary>
    public int WaitingForMemory => Volatile.Read(ref _waiting);

    /// <summary>Starts the threads; call once.</summary>
    public void Start()
    {
        for (int i = 0; i < Workers; i++)
        {
            bool speculative = i < SpeculativeWorkers;
            var thread = new Thread(() => Run(speculative)) { IsBackground = true, Name = "fivestar pipeline build" };
            thread.Start();
        }
    }

    /// <summary>
    /// The threads that also take creations no command list waits for yet (the journal's at a warm boot), half of them
    /// (at least one); the others are kept for urgent ones.
    /// </summary>
    public int SpeculativeWorkers => Math.Max(1, Workers / SpeculativeShare);

    /// <summary>Queues <paramref name="create"/>; the task ends with its result.</summary>
    public Task<PipelineResult> Enqueue(Func<PipelineResult> create)
    {
        return Queue(create, urgent: false).Done.Task;
    }

    /// <summary>Queues <paramref name="create"/>, before every creation that is not urgent when <paramref name="urgent"/>.</summary>
    public PipelineBuild Queue(Func<PipelineResult> create, bool urgent)
    {
        return Queue(create, urgent, null);
    }

    /// <summary>
    /// Queues <paramref name="create"/> as <see cref="Queue(Func{PipelineResult}, bool)"/>, ordered among urgent ones by
    /// <paramref name="use"/>, the stand-in it replaces.
    /// </summary>
    public PipelineBuild Queue(Func<PipelineResult> create, bool urgent, StandInUse? use)
    {
        var build = new PipelineBuild(create, urgent, use);
        Add(build, urgent);

        return build;
    }

    /// <summary>Moves a queued <paramref name="build"/> before every creation that is not urgent.</summary>
    public void Promote(PipelineBuild build)
    {
        if (build.MakeUrgent())
        {
            Add(build, urgent: true);
        }
    }

    /// <summary>The next creation for a thread: the urgent one the game misses most, else (<paramref name="speculative"/>) the oldest other.</summary>
    public PipelineBuild Take(bool speculative)
    {
        lock (_gate)
        {
            while (true)
            {
                if (_urgent.Count > 0)
                {
                    var best = Best(_urgent);
                    var build = _urgent[best];
                    _urgent.RemoveAt(best);

                    return build;
                }

                if (speculative && _builds.Count > 0)
                {
                    return _builds.Dequeue();
                }

                Monitor.Wait(_gate);
            }
        }
    }

    /// <summary>The index of the highest score; the earliest of equal ones.</summary>
    public static int Best(IReadOnlyList<PipelineBuild> builds)
    {
        int best = 0;
        long score = builds[0].Score;

        for (int index = 1; index < builds.Count; index++)
        {
            long candidate = builds[index].Score;

            if (candidate > score)
            {
                (best, score) = (index, candidate);
            }
        }

        return best;
    }

    private void Add(PipelineBuild build, bool urgent)
    {
        lock (_gate)
        {
            if (urgent)
            {
                _urgent.Add(build);
            }
            else
            {
                _builds.Enqueue(build);
            }

            Monitor.PulseAll(_gate);
        }
    }

    private void Run(bool speculative)
    {
        while (true)
        {
            // Memory first, then the work: a thread waiting for memory holds no creation, so an urgent one queued
            // meanwhile is the next taken.
            WaitForRoom();
            var build = Take(speculative);

            // A creation given back waits its turn again; a short pause keeps a thread with only those from spinning.
            if (build.Claim() && Execute(build, build.Urgent))
            {
                Thread.Sleep(GiveBackPauseMilliseconds);
            }
        }
    }

    /// <summary>Runs a queued <paramref name="build"/> on this thread (a game thread that waits for it) unless a build thread took it; true when this ran it.</summary>
    public bool RunNow(PipelineBuild build)
    {
        if (!build.Claim())
        {
            return false;
        }

        // A game thread never waits in the compile budget: a compile it would wait for goes back to the build threads,
        // urgent, and the game keeps the stand-in (boot w12b: a game thread waited there until the hang watchdog).
        Interlocked.Increment(ref _running);
        var (urgent, noWait) = (CompileBudget.Urgent, CompileBudget.NoWait);
        CompileBudget.Urgent = true;
        CompileBudget.NoWait = true;

        try
        {
            var result = build.Create();

            if (result.Hr == HResult.Pending)
            {
                build.Release();
                Add(build, urgent: true);

                return false;
            }

            build.Done.SetResult(result);

            return true;
        }
        finally
        {
            (CompileBudget.Urgent, CompileBudget.NoWait) = (urgent, noWait);
            Interlocked.Decrement(ref _running);
        }
    }

    /// <summary>
    /// Runs a build; an urgent one goes before the others in the driver compile budget (<see cref="CompileBudget"/>). One
    /// no command list waits for does not wait there: it goes back to the queue (<see cref="CompileBudget.GiveBack"/>),
    /// so the thread is free for urgent ones; true when it went back.
    /// </summary>
    private bool Execute(PipelineBuild build, bool urgent)
    {
        Interlocked.Increment(ref _running);
        var (previous, giveBack) = (CompileBudget.Urgent, CompileBudget.GiveBack);
        CompileBudget.Urgent = urgent;
        CompileBudget.GiveBack = !urgent;

        try
        {
            var result = build.Create();

            if (result.Hr == HResult.Pending)
            {
                build.Release();
                Add(build, build.Urgent);

                return true;
            }

            build.Done.SetResult(result);

            return false;
        }
        finally
        {
            (CompileBudget.Urgent, CompileBudget.GiveBack) = (previous, giveBack);
            Interlocked.Decrement(ref _running);
        }
    }

    private void WaitForRoom()
    {
        if (_room(Volatile.Read(ref _running)))
        {
            return;
        }

        Interlocked.Increment(ref _waiting);

        while (!_room(Volatile.Read(ref _running)))
        {
            Thread.Sleep(PauseMilliseconds);
        }

        Interlocked.Decrement(ref _waiting);
    }
}
