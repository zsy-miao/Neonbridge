// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The console's frame events on a timer: device slot 78 sets the interval, slot 79 places an event type at an offset
/// from each origin, slot 80 waits for one. A token counts origins (1 = the first after the interval was set); waiting
/// for the origin returns the next token, a token-in wait for another type returns once that frame's event passed.
/// A presented token is used up: the current token is never one already presented, so the render thread's wait for
/// event 2 of its previous frame (0x142482405) lasts until that frame's origin and frames come one per interval.
/// </summary>
internal sealed unsafe class FramePacer
{
    /// <summary>Event type 0: the frame origin, the only type the game waits for without a token.</summary>
    public const uint OriginEvent = 0;

    /// <summary>Flag 2 with timeout 0 (0x142482440): the current token, no wait (*inferred*).</summary>
    public const uint NoWaitFlag = 2;

    /// <summary>60 Hz, one refresh: the first entry of the game's interval table (0x1461ece60).</summary>
    private const uint DefaultMicroseconds = 16667;

    private const uint DefaultPeriod = 3;

    /// <summary>Event types 0 to 3; the game schedules 0 and 2.</summary>
    private const int EventTypes = 4;

    private const long MicrosecondsPerSecond = 1_000_000;

    private const long MillisecondsPerSecond = 1000;

    private readonly long[] _offsets = new long[EventTypes];
    private readonly bool[] _scheduled = new bool[EventTypes];
    private FrameGrid _grid;

    /// <summary>The last token a present carried on the current grid; 0 for none.</summary>
    private long _presented;

    /// <summary>Stopwatch ticks spent in blocking slot-80 waits.</summary>
    private long _waited;

    public FramePacer()
    {
        _grid = Create(DefaultMicroseconds, DefaultPeriod);
    }

    /// <summary>The current origins.</summary>
    public FrameGrid Grid => Volatile.Read(ref _grid);

    /// <summary>Stopwatch ticks the game spent in blocking slot-80 waits so far.</summary>
    public long WaitedTicks => Interlocked.Read(ref _waited);

    /// <summary>Slot 78: a new interval restarts the origins now; the same interval keeps them.</summary>
    public void SetInterval(uint microseconds, uint period)
    {
        var current = Grid;

        if (microseconds == 0 || (current.Microseconds == microseconds && current.Period == period))
        {
            return;
        }

        Volatile.Write(ref _grid, Create(microseconds, period));
        Volatile.Write(ref _presented, 0);
    }

    /// <summary>Queue slot 32: the plane's token is used up.</summary>
    [HotPath]
    public void Presented(ulong token)
    {
        long value = (long)token;
        long current = Volatile.Read(ref _presented);

        while (value > current)
        {
            long seen = Interlocked.CompareExchange(ref _presented, value, current);

            if (seen == current)
            {
                return;
            }

            current = seen;
        }
    }

    /// <summary>Slot 79: event <paramref name="type"/> falls <paramref name="microseconds"/> after each origin.</summary>
    public void Schedule(uint type, uint microseconds)
    {
        if (type >= EventTypes)
        {
            return;
        }

        Volatile.Write(ref _offsets[type], microseconds * Stopwatch.Frequency / MicrosecondsPerSecond);
        Volatile.Write(ref _scheduled[type], true);
    }

    /// <summary>
    /// Slot 80: (type, timeout in ms, flags, token in/out). S_FALSE when a finite timeout ends first, E_INVALIDARG for
    /// an event type slot 79 never scheduled (the origin always counts as scheduled); the game breaks only on a failing
    /// HRESULT (0x14248463e, 0x14248244e, 0x14248240d).
    /// </summary>
    [HotPath]
    public int Wait(uint type, uint timeout, uint flags, ulong* token)
    {
        if (type >= EventTypes || (type != OriginEvent && !Volatile.Read(ref _scheduled[type])))
        {
            return HResult.InvalidArgument;
        }

        var grid = Grid;
        long offset = Volatile.Read(ref _offsets[type]);

        long now = Stopwatch.GetTimestamp();
        ulong reached = Reached(grid, offset, now);
        ulong target = reached + 1;

        if (type == OriginEvent && ((flags & NoWaitFlag) != 0 || timeout == 0))
        {
            ulong current = (ulong)Volatile.Read(ref _presented) + 1;

            if (current < reached)
            {
                current = reached;
            }

            Store(token, current);

            return HResult.Ok;
        }

        if (type != OriginEvent && token != null && *token != 0)
        {
            target = *token;
        }

        int result = WaitFor(grid, offset, target, now, timeout, token);
        Interlocked.Add(ref _waited, Stopwatch.GetTimestamp() - now);

        return result;
    }

    [HotPath]
    private static int WaitFor(FrameGrid grid, long offset, ulong target, long now, uint timeout, ulong* token)
    {
        long due = grid.Origin + offset + ((long)target - 1) * grid.Interval;

        if (timeout != PresentNative.Infinite)
        {
            long limit = now + timeout * Stopwatch.Frequency / MillisecondsPerSecond;

            if (due > limit)
            {
                PreciseWait.Until(limit);

                return HResult.False;
            }
        }

        PreciseWait.Until(due);
        Store(token, target);

        return HResult.Ok;
    }

    /// <summary>How many events of this type have happened.</summary>
    [HotPath]
    private static ulong Reached(FrameGrid grid, long offset, long now)
    {
        long first = grid.Origin + offset;

        if (now < first)
        {
            return 0;
        }

        return (ulong)((now - first) / grid.Interval) + 1;
    }

    [HotPath]
    private static void Store(ulong* token, ulong value)
    {
        if (token == null)
        {
            return;
        }

        if (value == 0)
        {
            *token = 1;

            return;
        }

        *token = value;
    }

    private static FrameGrid Create(uint microseconds, uint period)
    {
        long interval = microseconds * Stopwatch.Frequency / MicrosecondsPerSecond;

        return new FrameGrid(Stopwatch.GetTimestamp(), interval, microseconds, period);
    }
}
