// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.Intrinsics;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// What every lane engine shares: the feeder window, the stall and clock limits of the reference model (bcpack-rs),
/// and the results that end a decode.
/// </summary>
internal static unsafe class LaneClock
{
    /// <summary>At most 8 stream bytes go out per clock, to the requesting lanes in lane order.</summary>
    public const int FeedWindow = 8;

    /// <summary>A decode with no progress for this many clocks fails.</summary>
    public const int StallLimit = 64;

    /// <summary>No lane: the control steps of a clock all succeeded.</summary>
    public const int NoLane = -1;

    private const long CycleSlack = 1000;
    private const int WindowBytes = 16;

    /// <summary>The bytes the feeder can hand out this clock: the window, or what is left of the stream.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static int Available(int position, int length)
    {
        var left = length - position;

        if (left < FeedWindow)
        {
            return left;
        }

        return FeedWindow;
    }

    /// <summary>The clock budget of the reference model.</summary>
    public static long MaxCycles(uint target, int length, int lanes) =>
        (StallLimit * ((lanes * (long)target) + length)) + CycleSlack;

    /// <summary>The result once every lane finished and drained: ok unless an end state is illegal.</summary>
    public static BcPackDecodeResult Finish(LaneRegisters* registers, int position, int length, long cycle)
    {
        var bad = registers->IllegalEndStates();

        if (bad != 0)
        {
            return new BcPackDecodeResult(BcPackDecodeStatus.IllegalEndState, position, length, cycle, 0, bad);
        }

        return new BcPackDecodeResult(BcPackDecodeStatus.Ok, position, length, cycle, 0, 0);
    }

    /// <summary>
    /// The shared control steps of a clock for the vector engines: first the lanes whose fragment input or drain
    /// <paramref name="ends"/>, then the <paramref name="slow"/> lanes in IDLE or a header state. Returns the lane whose
    /// header failed, or <see cref="NoLane"/>.
    /// </summary>
    [HotPath]
    public static int ControlSteps(LaneRegisters* registers, uint ends, uint slow, uint target)
    {
        for (; ends != 0; ends &= ends - 1)
        {
            registers->Transition(BitOperations.TrailingZeroCount(ends));
        }

        for (; slow != 0; slow &= slow - 1)
        {
            var lane = BitOperations.TrailingZeroCount(slow);

            if (!registers->Header(lane, target))
            {
                return lane;
            }
        }

        return NoLane;
    }

    public static BcPackDecodeResult BadHeader(int position, int length, long cycle, int lane) =>
        new(BcPackDecodeStatus.BadHeader, position, length, cycle, 0, lane);

    /// <summary>
    /// A failed header in a vector engine, which grants every lane's byte first. The scalar engine stops inside its
    /// lane loop, so only the grants up to <paramref name="lane"/> count.
    /// </summary>
    public static BcPackDecodeResult BadHeader(int position, uint grant, int length, long cycle, int lane) =>
        BadHeader(position - BitOperations.PopCount(grant >> (lane + 1)), length, cycle, lane);

    public static BcPackDecodeResult NoProgress(int position, int length, long cycle) =>
        new(BcPackDecodeStatus.NoProgress, position, length, cycle, 0, 0);

    public static BcPackDecodeResult CycleLimit(int position, int length, long cycles) =>
        new(BcPackDecodeStatus.CycleLimit, position, length, cycles, 0, 0);

    /// <summary>16 stream bytes from <paramref name="position"/>, zero past the end, for the vector feeders (which use at most 8).</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static Vector128<byte> Window(byte* stream, int position, int length)
    {
        if (length - position >= WindowBytes)
        {
            return Vector128.Load(stream + position);
        }

        var tail = Vector128<byte>.Zero;
        new ReadOnlySpan<byte>(stream + position, length - position).CopyTo(new Span<byte>(&tail, WindowBytes));

        return tail;
    }
}
