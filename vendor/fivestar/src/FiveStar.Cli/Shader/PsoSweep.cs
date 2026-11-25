// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using FiveStar.Graphics;
using FiveStar.Shaders.Containers;
using SharpGen.Runtime;

namespace FiveStar.Cli.Shader;

/// <summary>
/// Creates the pipeline state of every target on one device and logs each result as soon as it is known. A pipeline
/// state is released right after creation. The sweep stops when the device is removed. A driver compile cannot be
/// cancelled, so the limits end the process instead: past the time limit no new pipeline starts, and the late ones are
/// logged as timeouts once nothing else runs; over the memory limit the longest-running pipeline is logged as
/// over-memory at once. A new run resumes.
/// </summary>
internal sealed class PsoSweep
{
    /// <summary>The exit code of a run ended by a limit.</summary>
    public const int LimitExitCode = 3;

    private const int ProgressInterval = 500;
    private const string MessageSeparator = " | ";
    private const double BytesPerGigabyte = 1 << 30;
    private static readonly TimeSpan WatchInterval = TimeSpan.FromSeconds(1);

    private readonly GpuDevice _device;
    private readonly ProgramPipelines _pipelines;
    private readonly PsoLog _log;
    private readonly PsoLimits _limits;
    private readonly ConcurrentDictionary<string, (PsoTarget Target, long Start)> _running = new(StringComparer.Ordinal);
    private readonly Lock _ending = new();
    private int _done;
    private long _peakBytes;
    private volatile bool _draining;

    /// <summary>With the debug layer, <see cref="PsoLimits.Jobs"/> must be 1 so the device's messages belong to one pipeline.</summary>
    public PsoSweep(GpuDevice device, ProgramPipelines pipelines, PsoLog log, PsoLimits limits)
    {
        _device = device;
        _pipelines = pipelines;
        _log = log;
        _limits = limits;
    }

    /// <summary>The largest private memory of the process seen while the sweep ran.</summary>
    public long PeakBytes => Interlocked.Read(ref _peakBytes);

    /// <summary>Runs every target; false when the device was removed and the sweep stopped early.</summary>
    public bool Run(IReadOnlyList<PsoTarget> targets)
    {
        var clock = Stopwatch.StartNew();
        var removed = false;
        var options = new ParallelOptions { MaxDegreeOfParallelism = _limits.Jobs };
        using var watchdog = new Timer(_ => Watch(), null, WatchInterval, WatchInterval);

        Parallel.ForEach(targets, options, (target, loop) =>
        {
            if (_draining)
            {
                loop.Stop();

                return;
            }

            _log.MarkStarted(target);
            _running[target.Key] = (target, Stopwatch.GetTimestamp());
            var record = Try(target);
            _running.TryRemove(target.Key, out _);
            _log.Append(record);

            if (record.Result == PsoRecord.Rejected && _device.Removed)
            {
                removed = true;
                loop.Stop();
            }

            Progress(targets.Count, clock);
        });

        return !removed;
    }

    private void Progress(int total, Stopwatch clock)
    {
        var done = Interlocked.Increment(ref _done);

        if (done % ProgressInterval == 0)
        {
            var rate = (done / clock.Elapsed.TotalSeconds).ToString("F1", CultureInfo.InvariantCulture);
            Console.Error.WriteLine($"  {done}/{total}  {rate}/s  peak {Gigabytes(PeakBytes)} GB");
        }
    }

    private void Watch()
    {
        using var process = Process.GetCurrentProcess();
        var bytes = process.PrivateMemorySize64;
        var peak = Interlocked.Read(ref _peakBytes);

        if (bytes > peak)
        {
            Interlocked.Exchange(ref _peakBytes, bytes);
        }

        var running = _running.Values.ToList();
        var late = running.Where(entry => Stopwatch.GetElapsedTime(entry.Start) > _limits.Timeout).ToList();
        _draining |= late.Count > 0;

        if (bytes > _limits.MemoryBytes && running.Count > 0)
        {
            var oldest = running.MinBy(entry => entry.Start);
            End([oldest], running, PsoRecord.OverMemory, $"process over {Gigabytes(_limits.MemoryBytes)} GB of private memory");
        }
        else if (late.Count > 0 && late.Count == running.Count)
        {
            End(late, running, PsoRecord.TimedOut, null);
        }
    }

    /// <summary>
    /// Logs the guilty pipelines with the result, takes back the start of the other running ones (they run again in the
    /// next run) and ends the process, since compiles cannot be stopped.
    /// </summary>
    private void End(List<(PsoTarget Target, long Start)> guilty, List<(PsoTarget Target, long Start)> running, string result, string? reason)
    {
        lock (_ending)
        {
            foreach (var (target, start) in guilty)
            {
                var elapsed = Stopwatch.GetElapsedTime(start);
                var message = reason ?? $"no result after {(int)elapsed.TotalSeconds} s";
                _log.Append(new PsoRecord(target.Name, target.Kind, result, null, message, null, (long)elapsed.TotalMilliseconds));
                Console.Error.WriteLine($"{result}: {target.Name} ({target.Kind}); run again to resume");
            }

            foreach (var (target, _) in running.Where(entry => !guilty.Contains(entry)))
            {
                _log.Unstart(target);
            }

            Console.Error.WriteLine($"peak private memory {Gigabytes(PeakBytes)} GB");
            Environment.Exit(LimitExitCode);
        }
    }

    private static string Gigabytes(long bytes)
    {
        return (bytes / BytesPerGigabyte).ToString("F1", CultureInfo.InvariantCulture);
    }

    private PsoRecord Try(PsoTarget target)
    {
        var clock = Stopwatch.StartNew();

        if (!File.Exists(target.DxilPath))
        {
            return new PsoRecord(target.Name, target.Kind, PsoRecord.SetupFailed, null, "no DXIL file", null, 0);
        }

        var program = File.ReadAllBytes(target.DxilPath);
        var wave = WaveSize(target, program);
        _device.ClearMessages();

        try
        {
            using var state = _pipelines.Create(target, program);

            return new PsoRecord(target.Name, target.Kind, PsoRecord.Accepted, null, null, wave, clock.ElapsedMilliseconds);
        }
        catch (SharpGenException exception)
        {
            var hresult = "0x" + exception.ResultCode.Code.ToString("X8", CultureInfo.InvariantCulture);
            var message = Message(exception);

            return new PsoRecord(target.Name, target.Kind, PsoRecord.Rejected, hresult, message, wave, clock.ElapsedMilliseconds);
        }
        catch (Exception exception) when (exception is PipelineSetupException or InvalidDataException or ArgumentException)
        {
            return new PsoRecord(target.Name, target.Kind, PsoRecord.SetupFailed, null, exception.Message, wave, clock.ElapsedMilliseconds);
        }
    }

    /// <summary>The debug layer's errors when it runs, otherwise the exception's HRESULT text; device removal is named.</summary>
    private string Message(SharpGenException exception)
    {
        var errors = _device.TakeErrors();

        if (errors.Count > 0)
        {
            return string.Join(MessageSeparator, errors);
        }

        var text = exception.Message.Trim();

        if (_device.Removed)
        {
            text += " (device removed)";
        }

        return text;
    }

    /// <summary>The wave size a compute program pins with [WaveSize], or null.</summary>
    private static int? WaveSize(PsoTarget target, byte[] program)
    {
        if (target.Kind != "cs")
        {
            return null;
        }

        PipelineStateInfo info;

        try
        {
            info = PipelineStateParser.Parse(ProgramChunks.Psv(program).Span);
        }
        catch (InvalidDataException)
        {
            return null;
        }

        if (info.MinimumWaveLanes <= 0 || info.MinimumWaveLanes != info.MaximumWaveLanes)
        {
            return null;
        }

        return info.MinimumWaveLanes;
    }
}
