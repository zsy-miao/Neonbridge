// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The background threads that translate and compile programs on demand (<see cref="OnDemandPrograms"/>), off the
/// game's threads. Work a command list waits for goes before prefetched work, and only half the threads take prefetched
/// work at all; among that urgent work the one with the highest score goes first (<see cref="ProgramJob.Score"/>: the
/// stand-ins that wait for it skipped in the most frames), so the lighting and composite passes the frame skips every
/// present are not queued behind hundreds of programs used once (boot w36: the tiled-lighting programs waited about
/// 300 s for 3 s of work). Compiles of huge sources run on a thread of their own, so one compile that takes minutes
/// holds up nothing else. Like pipeline builds, work starts only while the machine has the memory for it
/// (<see cref="WarmMemoryGuard.RoomForPrograms"/>).
/// </summary>
internal sealed class ProgramWorkers
{
    private const int PauseMilliseconds = 2000;

    // One thread in this many also takes prefetched work.
    private const int PrefetchShare = 2;

    private readonly object _gate = new();
    private readonly List<(Action Work, Func<long>? Score)> _urgent = [];
    private readonly Queue<Action> _prefetch = [];
    private readonly BlockingCollection<Action> _heavy = [];
    private readonly Func<int, bool> _room;
    private readonly Func<int, bool> _heavyRoom;
    private int _running;
    private int _waiting;

    /// <param name="workers">Threads for ordinary work; at least one. One more thread runs heavy work.</param>
    /// <param name="room">Whether ordinary work may start, given the work running.</param>
    /// <param name="heavyRoom">Whether heavy work may start, given the work running.</param>
    public ProgramWorkers(int workers, Func<int, bool> room, Func<int, bool> heavyRoom)
    {
        Workers = Math.Max(1, workers);
        _room = room;
        _heavyRoom = heavyRoom;
    }

    /// <summary>The threads for ordinary work.</summary>
    public int Workers { get; }

    /// <summary>
    /// The threads that also take prefetched work, half of them (at least one): at a warm boot the journal queues a
    /// thousand programs or more at once, and the others stay free for the work the game waits for.
    /// </summary>
    public int PrefetchWorkers => Math.Max(1, Workers / PrefetchShare);

    /// <summary>Work queued and not started.</summary>
    public int Backlog
    {
        get
        {
            lock (_gate)
            {
                return _urgent.Count + _prefetch.Count + _heavy.Count;
            }
        }
    }

    /// <summary>Work running now.</summary>
    public int Running => Volatile.Read(ref _running);

    /// <summary>Threads waiting for memory before their next work.</summary>
    public int WaitingForMemory => Volatile.Read(ref _waiting);

    /// <summary>Starts the threads; call once.</summary>
    public void Start()
    {
        for (int i = 0; i < Workers; i++)
        {
            bool prefetch = i < PrefetchWorkers;
            new Thread(() => RunOrdinary(prefetch)) { IsBackground = true, Name = "fivestar program compile" }.Start();
        }

        new Thread(RunHeavy) { IsBackground = true, Name = "fivestar heavy program compile" }.Start();
    }

    /// <summary>
    /// Queues ordinary work; <paramref name="urgent"/> for work a command list waits for, ordered among the urgent by
    /// <paramref name="score"/> (higher first, read when a worker picks; none counts as 0, ties in queue order).
    /// </summary>
    public void Queue(Action work, bool urgent, Func<long>? score = null)
    {
        lock (_gate)
        {
            if (urgent)
            {
                _urgent.Add((work, score));
            }
            else
            {
                _prefetch.Enqueue(work);
            }

            Monitor.PulseAll(_gate);
        }
    }

    /// <summary>Queues work for the heavy thread.</summary>
    public void QueueHeavy(Action work)
    {
        _heavy.Add(work);
    }

    /// <summary>The index of the highest score; the earliest of equal ones.</summary>
    public static int Best(IReadOnlyList<long> scores)
    {
        int best = 0;

        for (int index = 1; index < scores.Count; index++)
        {
            if (scores[index] > scores[best])
            {
                best = index;
            }
        }

        return best;
    }

    /// <summary>The next work for a thread: the urgent work scored highest, else (<paramref name="prefetch"/>) the oldest prefetched.</summary>
    private Action Take(bool prefetch)
    {
        lock (_gate)
        {
            while (true)
            {
                if (_urgent.Count > 0)
                {
                    var best = Best(_urgent.ConvertAll(entry => entry.Score?.Invoke() ?? 0));
                    var work = _urgent[best].Work;
                    _urgent.RemoveAt(best);

                    return work;
                }

                if (prefetch && _prefetch.Count > 0)
                {
                    return _prefetch.Dequeue();
                }

                Monitor.Wait(_gate);
            }
        }
    }

    private void RunOrdinary(bool prefetch)
    {
        while (true)
        {
            // Memory first, then the work: a thread waiting for memory holds no work, so urgent work queued meanwhile
            // is the next taken.
            WaitForRoom(_room);
            Run(Take(prefetch));
        }
    }

    private void RunHeavy()
    {
        while (true)
        {
            WaitForRoom(_heavyRoom);
            Run(_heavy.Take());
        }
    }

    private void WaitForRoom(Func<int, bool> room)
    {
        if (room(Running))
        {
            return;
        }

        Interlocked.Increment(ref _waiting);

        while (!room(Running))
        {
            Thread.Sleep(PauseMilliseconds);
        }

        Interlocked.Decrement(ref _waiting);
    }

    private void Run(Action work)
    {
        Interlocked.Increment(ref _running);

        try
        {
            work();
        }
        finally
        {
            Interlocked.Decrement(ref _running);
        }
    }
}
