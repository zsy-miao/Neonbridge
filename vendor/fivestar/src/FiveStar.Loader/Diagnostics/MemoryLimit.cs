// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// <c>--min-free-memory &lt;GB&gt;</c> and <c>--max-commit &lt;GB&gt;</c>: ends the process, with a last memory map,
/// as soon as the machine's free physical memory drops below the floor or its commit charge passes the ceiling, so a
/// run that runs away with memory cannot take the machine down (exit code <see cref="ExitCode"/>).
/// </summary>
internal sealed unsafe class MemoryLimit
{
    /// <summary>The process's exit code when a limit ended it.</summary>
    public const int ExitCode = 5;

    private const int PollMilliseconds = 250;
    private const double BytesPerGigabyte = 1L << 30;

    private readonly ulong _minimumFree;
    private readonly ulong _maximumCommit;
    private readonly MemoryMap? _map;
    private readonly LoaderLog _log;

    /// <param name="minimumFree">Free physical bytes below which the process ends; 0 for no floor.</param>
    /// <param name="maximumCommit">System commit bytes above which the process ends; 0 for no ceiling.</param>
    /// <param name="map">Logs the last memory map; null where no game runs (<c>--warm-pipelines</c>).</param>
    /// <param name="log">Where the end is logged.</param>
    public MemoryLimit(ulong minimumFree, ulong maximumCommit, MemoryMap? map, LoaderLog log)
    {
        _minimumFree = minimumFree;
        _maximumCommit = maximumCommit;
        _map = map;
        _log = log;
    }

    public void Start()
    {
        var thread = new Thread(Run) { IsBackground = true, Name = "memory limit", Priority = ThreadPriority.Highest };
        thread.Start();
        _log.Info($"  [memory] limit: the process ends below {Gigabytes(_minimumFree)} free or above {Gigabytes(_maximumCommit)} committed (0 = none)");
    }

    private void Run()
    {
        var clock = Stopwatch.StartNew();

        while (true)
        {
            Thread.Sleep(PollMilliseconds);
            string? reason = Exceeded();

            if (reason is not null)
            {
                End(reason, clock.Elapsed.TotalSeconds);
            }
        }
    }

    /// <summary>What passed its limit, or null.</summary>
    private string? Exceeded()
    {
        MemoryStatusEx status = default;
        status.Length = (uint)sizeof(MemoryStatusEx);

        if (KernelBase.GlobalMemoryStatusEx(&status) == 0)
        {
            return null;
        }

        ulong committed = status.TotalPageFile - status.AvailablePageFile;

        if (_minimumFree != 0 && status.AvailablePhysical < _minimumFree)
        {
            return $"free physical memory {Gigabytes(status.AvailablePhysical)} is below {Gigabytes(_minimumFree)} (commit {Gigabytes(committed)})";
        }

        if (_maximumCommit != 0 && committed > _maximumCommit)
        {
            return $"commit {Gigabytes(committed)} is above {Gigabytes(_maximumCommit)} (free physical {Gigabytes(status.AvailablePhysical)})";
        }

        return null;
    }

    private void End(string reason, double seconds)
    {
        _log.Error($"[memory] limit at {seconds:F1} s: {reason}; {MemorySnapshot.Summary()}; ending the process (exit code {ExitCode})");

        try
        {
            _map?.Log(0, seconds);
        }
        finally
        {
            Console.Out.Flush();
            _ = Kernel32.TerminateProcess(Kernel32.GetCurrentProcess(), ExitCode);
        }
    }

    private static string Gigabytes(ulong bytes)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{bytes / BytesPerGigabyte:F1} GB");
    }
}
