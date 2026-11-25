// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The production of one program on demand (<see cref="OnDemandPrograms"/>): when it was first asked for, the queue it
/// waits in, the stand-ins waiting for it and its outcome. Prefetched work a command list then waits for is queued
/// again as urgent; whichever entry a worker takes first runs it.
/// </summary>
internal sealed class ProgramJob
{
    private const int NotQueued = 0;
    private const int Prefetch = 1;
    private const int Urgent = 2;

    private readonly TaskCompletionSource<bool> _done = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly List<StandInUse> _users = [];
    private int _queued;
    private int _started;

    public ProgramJob(string name, long requested)
    {
        Name = name;
        Requested = requested;
    }

    public string Name { get; }

    /// <summary>The timestamp of the first request.</summary>
    public long Requested { get; }

    /// <summary>Ends true when the program's DXIL is ready, false when it cannot be made.</summary>
    public Task<bool> Done => _done.Task;

    /// <summary>
    /// The job's place among urgent work: the highest <see cref="StandInUse.Score"/> of the stand-ins waiting for it,
    /// read when a worker picks, so a program the frame skips every present goes before one a loading screen used once.
    /// </summary>
    public long Score
    {
        get
        {
            lock (_users)
            {
                long best = 0;

                foreach (var user in _users)
                {
                    best = Math.Max(best, user.Score);
                }

                return best;
            }
        }
    }

    /// <summary>Counts the stand-in of <paramref name="use"/> among those waiting for the program; null adds none.</summary>
    public void WantedBy(StandInUse? use)
    {
        if (use is null)
        {
            return;
        }

        lock (_users)
        {
            if (!_users.Contains(use))
            {
                _users.Add(use);
            }
        }
    }

    /// <summary>Whether a request needs a queue entry: the first one, or the first urgent one while only prefetched.</summary>
    public bool Queue(bool urgent)
    {
        var level = Prefetch;

        if (urgent)
        {
            level = Urgent;
        }

        while (true)
        {
            var current = Volatile.Read(ref _queued);

            if (current >= level || Volatile.Read(ref _started) != 0)
            {
                return false;
            }

            if (Interlocked.CompareExchange(ref _queued, level, current) == current)
            {
                return true;
            }
        }
    }

    /// <summary>Whether the calling worker is the one that runs the job.</summary>
    public bool Start()
    {
        return Interlocked.Exchange(ref _started, 1) == 0;
    }

    public void Finish(bool ready)
    {
        lock (_users)
        {
            _users.Clear();
        }

        _done.TrySetResult(ready);
    }
}
