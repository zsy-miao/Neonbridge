// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Text;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// With <c>--watch-transient</c> and <c>--d3d12-breadcrumbs</c>: a lifetime check of the transient pool at full speed.
/// Each recording whose operations reach a block (<see cref="TransientPoolHits"/>: root addresses, user-data V#s, table
/// UAVs) is kept per block, and a range over more than <see cref="WideBlocks"/> blocks (the game's whole-pool raw view)
/// in a separate ring. When the watch sees a block's retire token change, the game has taken the block again; every
/// kept recording of that block whose submission has not completed on its queue's shim fence, or that was never
/// submitted, is a violation: GPU work that can still write memory the game reuses. The first
/// <see cref="Reported"/> are logged with their queue, how far behind the fence is and their pipelines; then counts.
/// </summary>
internal static unsafe class TransientPoolLifetime
{
    private const int PerBlock = 16;
    private const int WideRing = 128;
    private const int WideBlocks = 8;
    private const int Reported = 24;
    private const int TokenOffset = 0x18;
    private const int GetCompletedValueSlot = 8;
    private static readonly TimeSpan SummaryEvery = TimeSpan.FromSeconds(10);

    private static ulong _start;
    private static uint _size;
    private static uint _blocks;
    private static uint[]? _serials;
    private static int[] _next = [];
    private static ulong[] _tokens = [];
    private static readonly uint[] Wide = new uint[WideRing];
    private static long _wideNext;
    private static LoaderLog? _log;
    private static long _recycles;
    private static long _violations;
    private static long _wideViolations;
    private static long _unsubmitted;
    private static long _reported;
    private static long _summaryAt;

    /// <summary>Starts the check for a pool of <paramref name="blocks"/> blocks of <paramref name="size"/> bytes.</summary>
    public static void Setup(ulong start, uint blocks, uint size, LoaderLog log)
    {
        _start = start;
        _size = size;
        _blocks = blocks;
        _next = new int[blocks];
        _tokens = new ulong[blocks];

        _log = log;
        _summaryAt = Stopwatch.GetTimestamp();
        Volatile.Write(ref _serials, new uint[blocks * PerBlock]);
    }

    /// <summary>Recording <paramref name="serial"/> reaches [<paramref name="start"/>, +<paramref name="bytes"/>).</summary>
    [HotPath]
    public static void Touch(uint serial, ulong start, ulong bytes)
    {
        var serials = Volatile.Read(ref _serials);

        if (serials == null || _size == 0)
        {
            return;
        }

        ulong end = _start + ((ulong)_blocks * _size);
        ulong first = start < _start ? 0 : (start - _start) / _size;
        ulong lastAddress = start;

        if (bytes > 1)
        {
            lastAddress = start + bytes - 1;
        }

        ulong last = lastAddress >= end ? _blocks - 1 : (lastAddress - _start) / _size;

        if (first >= _blocks || last < first)
        {
            return;
        }

        if (last - first + 1 > WideBlocks)
        {
            long at = Interlocked.Increment(ref _wideNext) - 1;

            if (Wide[(at + WideRing - 1) % WideRing] != serial)
            {
                Wide[at % WideRing] = serial;
            }

            return;
        }

        for (ulong block = first; block <= last; block++)
        {
            int next = _next[block];
            int previous = (next + PerBlock - 1) % PerBlock;

            if (serials[(block * PerBlock) + (ulong)previous] == serial)
            {
                continue;
            }

            serials[(block * PerBlock) + (ulong)next] = serial;
            _next[block] = (next + 1) % PerBlock;
        }
    }

    /// <summary>From the watch, per block record each poll: a changed retire token means the block was taken again.</summary>
    public static void Check(uint block, byte* record)
    {
        var serials = Volatile.Read(ref _serials);

        if (serials == null || block >= _blocks)
        {
            return;
        }

        ulong token = *(ulong*)(record + TokenOffset);
        ulong previous = _tokens[block];

        if (previous == token)
        {
            return;
        }

        _tokens[block] = token;

        if (previous == 0)
        {
            return;
        }

        _recycles++;

        for (int i = 0; i < PerBlock; i++)
        {
            Examine(serials[(block * PerBlock) + (ulong)i], block, previous, token, wide: false);
        }

        for (int i = 0; i < WideRing; i++)
        {
            Examine(Wide[i], block, previous, token, wide: true);
        }

        Summary();
    }

    private static void Examine(uint serial, uint block, ulong previous, ulong token, bool wide)
    {
        if (!GpuBreadcrumbs.TryRecording(serial, out nint fence, out ulong value))
        {
            return;
        }

        if (fence == 0)
        {
            // Usually the recording that takes the block now: counted, not reported.
            _unsubmitted++;

            return;
        }

        ulong completed = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(fence, GetCompletedValueSlot))(fence);

        if (completed >= value)
        {
            return;
        }

        if (wide)
        {
            _wideViolations++;
        }
        else
        {
            _violations++;
        }

        Report(serial, block, previous, token, wide, $"queue #{RetireWaits.QueueOf(fence)}", value, completed);
    }

    private static void Report(uint serial, uint block, ulong previous, ulong token, bool wide, string where, ulong value, ulong completed)
    {
        if (++_reported > Reported)
        {
            return;
        }

        var (retire, retireAt) = RetireWaits.LastWrite;
        string reach = wide ? "the whole pool (a view over more than 8 blocks)" : $"block {block}";
        var text = new StringBuilder();
        text.Append($"[lifetime] block {block} (0x{_start + ((ulong)block * _size):x}) taken again (token 0x{previous:x} -> 0x{token:x}) while recording {serial}, which reaches {reach}, is unfinished: {where}, at {completed} of {value} ({value - completed} submissions behind); last slot-74 wait for 0x{retireAt:x} to reach 0x{retire:x}; its pipelines:");
        TransientPoolHits.DescribeRecording(text, serial, QueueProgress.PipelineName);
        _log?.Warn(text.ToString());
    }

    private static void Summary()
    {
        if (Stopwatch.GetElapsedTime(_summaryAt) < SummaryEvery)
        {
            return;
        }

        _summaryAt = Stopwatch.GetTimestamp();
        _log?.Info($"  [lifetime] {_recycles} blocks taken again; unfinished work reaching them: {_violations} narrow, {_wideViolations} through a whole-pool view, {_unsubmitted} not submitted yet (the recordings taking them)");
    }
}
