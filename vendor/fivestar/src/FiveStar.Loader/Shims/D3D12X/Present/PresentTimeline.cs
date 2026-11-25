// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// Where a slow present spent its time: the game's own time since the previous present returned, then the swap
/// chain's latency wait, the back buffer's retire wait, the copy submission and IDXGISwapChain::Present. A present
/// whose gap passes <see cref="SlowMilliseconds"/> is logged (the first <see cref="Lines"/>), with the runtime's GC
/// pause time over the gap, so a stall can be placed in the game, the GPU or the runtime.
/// </summary>
internal sealed class PresentTimeline
{
    /// <summary>Phases measured inside the present, in order.</summary>
    public const int LatencyWait = 0;
    public const int RetireWait = 1;
    public const int Submit = 2;
    public const int Present = 3;
    private const int Phases = 4;

    /// <summary>A quarter second: several frames at any interval the game asks for.</summary>
    private const double SlowMilliseconds = 250.0;

    private const int Lines = 64;
    private const double MillisecondsPerSecond = 1000.0;

    private readonly LoaderLog _log;
    private readonly long[] _marks = new long[Phases];
    private long _entered;
    private long _phaseStart;
    private long _previousExit;
    private long _first;
    private TimeSpan _previousPause;
    private long _previousWaited;
    private int _lines;

    public PresentTimeline(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>The present handler started.</summary>
    [HotPath]
    public void Enter()
    {
        _entered = Stopwatch.GetTimestamp();
        _phaseStart = _entered;
    }

    /// <summary>Phase <paramref name="phase"/> ended now.</summary>
    [HotPath]
    public void Mark(int phase)
    {
        long now = Stopwatch.GetTimestamp();
        _marks[phase] = now - _phaseStart;
        _phaseStart = now;
    }

    /// <summary>The present returned; logs it when the frame was slow.</summary>
    [HotPath]
    public void Exit(long presents)
    {
        long now = Stopwatch.GetTimestamp();
        var pause = GC.GetTotalPauseDuration();

        if (_previousExit == 0)
        {
            _first = now;
        }
        else if (_lines < Lines && Milliseconds(now - _previousExit) >= SlowMilliseconds)
        {
            Report(presents, now, pause);
        }

        _previousExit = now;
        _previousPause = pause;
        _previousWaited = ShimContext.Runtime.D3D12X.Pacer.WaitedTicks;
    }

    private void Report(long presents, long now, TimeSpan pause)
    {
        _lines++;
        double game = Milliseconds(_entered - _previousExit);
        double gc = (pause - _previousPause).TotalMilliseconds;

        var d3d12x = ShimContext.Runtime.D3D12X;
        long waited = d3d12x.Pacer.WaitedTicks;
        double frameEvents = Milliseconds(waited - _previousWaited);
        _log.Info($"  [present] slow present {presents} at {Milliseconds(now - _first) / MillisecondsPerSecond:F1} s: {Milliseconds(now - _previousExit):F0} ms = game {game:F0} (frame-event waits {frameEvents:F0}) + latency wait {Milliseconds(_marks[LatencyWait]):F0} + retire wait {Milliseconds(_marks[RetireWait]):F0} + submit {Milliseconds(_marks[Submit]):F0} + present {Milliseconds(_marks[Present]):F0}; GC pauses {gc:F0} ms; longest Signal-to-cell {Milliseconds(d3d12x.Fences.TakeLongestLatency()):F0} ms; fences (cell/completed){d3d12x.Fences.Describe()}");
    }

    private static double Milliseconds(long ticks)
    {
        return ticks * MillisecondsPerSecond / Stopwatch.Frequency;
    }
}
