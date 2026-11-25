// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;
using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The translated programs, made when a pipeline first needs them, as console emulators do: a program the ahead-of-time
/// translation (--translated-dir, optional) and the program cache lack is translated and compiled in this process
/// (<see cref="ProgramProducer"/>) on the <see cref="ProgramWorkers"/>, and its DXIL is kept in the program cache folder
/// for the next boot. The pipeline gets a stand-in until its programs are ready (<see cref="StandInPipelines.Waiting"/>).
/// </summary>
internal sealed class OnDemandPrograms : IProgramLibrary
{
    private const string CauseSeparator = ":";
    private const int CausesShown = 6;

    private readonly ProgramIndex _index;
    private readonly TranslatedProgramLibrary? _translated;
    private readonly TranslatedProgramLibrary _compiled;
    private readonly ProgramProducer _producer;
    private readonly ProgramWorkers _workers;
    private readonly OnceLog _warnings;
    private readonly Func<string>? _compilerCounts;
    private readonly ConcurrentDictionary<string, ProgramJob> _jobs = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, int> _failures = new(StringComparer.Ordinal);
    private int _made;
    private int _found;
    private int _ready;
    private long _readyTicks;
    private long _slowestTicks;

    /// <param name="index">The extracted programs.</param>
    /// <param name="translated">The ahead-of-time translation, looked at first; null for none.</param>
    /// <param name="producer">Makes a program's DXIL into the program cache folder.</param>
    /// <param name="workers">Runs the production (started by the caller).</param>
    /// <param name="warnings">Where failures are reported, once per program.</param>
    /// <param name="compilerCounts">The compiler's own counts for the summary (which compiler made the programs); null for none.</param>
    public OnDemandPrograms(ProgramIndex index, TranslatedProgramLibrary? translated, ProgramProducer producer, ProgramWorkers workers, OnceLog warnings, Func<string>? compilerCounts)
    {
        _index = index;
        _translated = translated;
        _compiled = new TranslatedProgramLibrary(index, producer.Folder, null);

        _producer = producer;
        _workers = workers;
        _warnings = warnings;
        _compilerCounts = compilerCounts;

        Links = new ProducerLinks(producer, workers);
    }

    /// <summary>Producers linked to the pixel shaders they run with.</summary>
    public ProducerLinks Links { get; }

    public long HeldBytes => (_translated?.HeldBytes ?? 0) + _compiled.HeldBytes;

    /// <summary>Programs made, found made, failed by cause, the time to ready and the work waiting.</summary>
    public string Summary
    {
        get
        {
            var ready = Math.Max(1, Volatile.Read(ref _ready));
            var average = TimeSpan.FromTicks(Interlocked.Read(ref _readyTicks) / ready).TotalSeconds;
            var slowest = TimeSpan.FromTicks(Interlocked.Read(ref _slowestTicks)).TotalSeconds;
            // A snapshot: LINQ over the dictionary itself sizes an array by Count, then copies while workers add causes.
            var failures = _failures.ToArray();
            var failed = failures.Sum(entry => entry.Value);
            var causes = string.Join("; ", failures.OrderByDescending(entry => entry.Value).Take(CausesShown).Select(entry => $"{entry.Value} {entry.Key}"));

            return $"programs on demand: {_jobs.Count} asked for, {Volatile.Read(ref _made)} translated and compiled, {Volatile.Read(ref _found)} found compiled{_compilerCounts?.Invoke()}, ready after {average:F1} s on average and {slowest:F0} s at most, {failed} failed ({causes}), {_workers.Backlog} queued, {_workers.Running} running, {_workers.WaitingForMemory} waiting for memory, {Links.Summary}";
        }
    }

    /// <summary>Drops the DXIL read so far; programs in use stay with their users.</summary>
    public void Forget()
    {
        _translated?.Forget();
        _compiled.Forget();
    }

    public TranslatedProgram? Find(ReadOnlySpan<byte> prefix)
    {
        return _translated?.Find(prefix) ?? _compiled.Find(prefix);
    }

    public IReadOnlyList<string> Names(ReadOnlySpan<byte> prefix)
    {
        return [.. _index.Find(prefix)];
    }

    public TranslatedProgram? Load(string name)
    {
        return _translated?.Load(name) ?? _compiled.Load(name);
    }

    /// <summary>
    /// The extracted programs of <paramref name="prefix"/> to make, in the order a library tries them; null when one of
    /// them is loadable now or none was extracted (the pipeline then fails as without this source).
    /// </summary>
    public IReadOnlyList<string>? Lacking(ReadOnlySpan<byte> prefix)
    {
        var names = _index.Find(prefix);

        if (names.Count == 0 || names.Any(name => (_translated?.Has(name) ?? false) || _compiled.Has(name)))
        {
            return null;
        }

        return names;
    }

    /// <summary>
    /// Makes the first of <paramref name="names"/> that can be made; ends true when one is ready.
    /// <paramref name="urgent"/> when a command list waits for it, so it goes before prefetched work, ordered by how
    /// much the game misses <paramref name="use"/>, the stand-in that waits (none for the warm-up).
    /// </summary>
    public async Task<bool> Request(IReadOnlyList<string> names, bool urgent, StandInUse? use = null)
    {
        foreach (var name in names)
        {
            if (await Job(name, urgent, use).ConfigureAwait(false))
            {
                return true;
            }
        }

        return false;
    }

    /// <summary>
    /// Makes the first of <paramref name="names"/> that can be made on this thread, for a small program a command list
    /// waits for: a worker would take it only after the queue (a minute or more while loading). A job a worker already
    /// runs, or a heavy one, is left to the workers. True when one is ready.
    /// </summary>
    public bool MakeNow(IReadOnlyList<string> names)
    {
        foreach (var name in names)
        {
            var job = _jobs.GetOrAdd(name, static key => new ProgramJob(key, Stopwatch.GetTimestamp()));

            if (!job.Done.IsCompleted)
            {
                Run(job);
            }

            if (job.Done.IsCompletedSuccessfully && job.Done.Result)
            {
                return true;
            }

            if (!job.Done.IsCompleted)
            {
                return false;
            }
        }

        return false;
    }

    /// <summary>Queues the programs of the shader prefixes in their order (the pipeline journal's first-use order).</summary>
    public int Prefetch(IEnumerable<byte[]> prefixes)
    {
        var queued = 0;

        foreach (var prefix in prefixes)
        {
            var names = Lacking(prefix);

            if (names is not null)
            {
                _ = Request(names, urgent: false);
                queued++;
            }
        }

        return queued;
    }

    private Task<bool> Job(string name, bool urgent, StandInUse? use)
    {
        var job = _jobs.GetOrAdd(name, static key => new ProgramJob(key, Stopwatch.GetTimestamp()));
        job.WantedBy(use);

        if (job.Queue(urgent))
        {
            _workers.Queue(() => Run(job), urgent, () => job.Score);
        }

        return job.Done;
    }

    /// <summary>On a worker: translates the program, then compiles it here or on the heavy thread.</summary>
    private void Run(ProgramJob job)
    {
        if (!job.Start())
        {
            return;
        }

        Guarded(job, () =>
        {
            if (_producer.Exists(job.Name))
            {
                Interlocked.Increment(ref _found);
                Finish(job, null);

                return;
            }

            var kept = _producer.KeptFailure(job.Name);

            if (kept is not null)
            {
                Finish(job, kept);

                return;
            }

            var hosted = _producer.MakeInHost(job.Name);

            if (hosted.Hosted && hosted.Dxc is null)
            {
                Made(job, hosted.Failure);

                return;
            }

            var form = hosted.Dxc ?? _producer.Prepare(job.Name);

            if (form.Failure is not null)
            {
                Finish(job, form.Failure);

                return;
            }

            bool dxcOnly = hosted.Dxc is not null || hosted.DxcOnly;

            if (form.Heavy)
            {
                _workers.QueueHeavy(() => Guarded(job, () => Compile(job, form, dxcOnly)));

                return;
            }

            Compile(job, form, dxcOnly);
        });
    }

    /// <summary>A program the compile host made (null) or refused.</summary>
    private void Made(ProgramJob job, string? failure)
    {
        if (failure is null)
        {
            Interlocked.Increment(ref _made);
        }

        Finish(job, failure);
    }

    private void Compile(ProgramJob job, ProgramForm form, bool dxcOnly)
    {
        var failure = _producer.Compile(form, dxcOnly);

        if (failure is null)
        {
            Interlocked.Increment(ref _made);
        }

        Finish(job, failure);
    }

    /// <summary>Runs work for a job; an error ends the job as failed instead of the worker.</summary>
    private void Guarded(ProgramJob job, Action work)
    {
        try
        {
            work();
        }
        catch (Exception exception) when (exception is not OutOfMemoryException)
        {
            Finish(job, $"error: {exception.GetType().Name} {exception.Message}");
        }
    }

    private void Finish(ProgramJob job, string? failure)
    {
        if (failure is null)
        {
            _compiled.Arrived(job.Name);

            // The compile host may have made it: its ray-tracing marker is read again.
            RayTracingPrograms.Refresh(job.Name);
            var took = Stopwatch.GetElapsedTime(job.Requested).Ticks;
            Interlocked.Add(ref _readyTicks, took);
            Interlocked.Increment(ref _ready);
            Slowest(took);
            job.Finish(ready: true);

            return;
        }

        _failures.AddOrUpdate(Cause(failure), 1, static (_, count) => count + 1);
        _warnings.Warn($"on-demand-{job.Name}", $"[d3d12_xs] program {job.Name} not made on demand: {failure}");
        job.Finish(ready: false);
    }

    private void Slowest(long ticks)
    {
        var current = Interlocked.Read(ref _slowestTicks);

        while (ticks > current)
        {
            var seen = Interlocked.CompareExchange(ref _slowestTicks, ticks, current);

            if (seen == current)
            {
                return;
            }

            current = seen;
        }
    }

    /// <summary>The cause of a failure ("unsupported", "dxc-fail", "no form"): its text up to the first colon.</summary>
    private static string Cause(string failure)
    {
        var end = failure.IndexOf(CauseSeparator, StringComparison.Ordinal);

        if (end < 0)
        {
            return failure;
        }

        return failure[..end];
    }
}
