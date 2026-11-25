// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The clocked lane model follows bcpack-rs src/rans.rs (MIT); each lane is the decoder module of Microsoft
// silicon-rans mdu_minirans.v (MIT). See THIRD_PARTY_NOTICES.md.

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The portable lane engine: a clock-exact simulation of a codec's lanes sharing one byte stream, one lane at a time.
/// Per clock and lane, phase 0 (state update from the held symbol, the feeder byte, the symbol hand-off) and phase 1
/// (header control, renormalise, decode). The two phases of a lane can run together because phase 0 of later lanes
/// reads only the feeder counters and the slot mask, which phase 1 never writes.
/// </summary>
internal static unsafe class ScalarLaneEngine
{
    [HotPath]
    [SkipLocalsInit]
    public static BcPackDecodeResult DecodeLanes<TSink>(ReadOnlySpan<byte> data, int target, ReadOnlySpan<byte> widths, ref TSink sink)
        where TSink : struct, ISymbolVectorSink
    {
        Unsafe.SkipInit(out LaneRegisters registers);
        registers.Reset(widths);

        if (data.IsEmpty)
        {
            return LaneClock.NoProgress(0, 0, 0);
        }

        fixed (byte* stream = data)
        {
            return Clock(stream, data.Length, (uint)target, &registers, ref sink);
        }
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveOptimization)]
    private static BcPackDecodeResult Clock<TSink>(byte* stream, int length, uint target, LaneRegisters* r, ref TSink sink)
        where TSink : struct, ISymbolVectorSink
    {
        var maxCycles = LaneClock.MaxCycles(target, length, r->Lanes);
        var watch = ProgressWatch.Start();
        var position = 0;
        var released = 0;
        long symbols = 0;
        uint slots = 0;

        for (long cycle = 0; cycle < maxCycles;)
        {
            var bad = ClockLanes(stream, length, target, r, ref position, ref slots, ref symbols);

            if (bad != LaneClock.NoLane)
            {
                return LaneClock.BadHeader(position, length, cycle + 1, bad);
            }

            // The reference releases a vector when every lane still active holds a symbol. All lanes code the same
            // number of symbols, so between releases each lane pushed k or k + 1 of them, and a drained lane leaves no
            // lane able to push: the rule is exactly "every slot full".
            if (slots == r->AllLanes)
            {
                sink.Emit(released, r->Released);
                released++;
                slots = 0;
            }

            cycle++;

            if (r->Finished())
            {
                return LaneClock.Finish(r, position, length, cycle);
            }

            if (watch.Stalled(position, symbols, r->ControlSum, cycle))
            {
                return LaneClock.NoProgress(position, length, cycle);
            }
        }

        return LaneClock.CycleLimit(position, length, maxCycles);
    }

    /// <summary>One clock of every lane; returns the lane whose header failed, or <see cref="LaneClock.NoLane"/>.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int ClockLanes(byte* stream, int length, uint target, LaneRegisters* r, ref int position, ref uint slots, ref long symbols)
    {
        var served = 0;

        for (var lane = 0; lane < r->Lanes; lane++)
        {
            var control = r->Control[lane];
            var blocked = r->SymbolFull[lane] != 0 && ((slots >> lane) & 1) != 0;

            if (NeedsByte(r, lane, control))
            {
                // A lane refused its byte stalls: neither phase runs this clock.
                if (blocked || served >= LaneClock.FeedWindow || position >= length)
                {
                    continue;
                }

                r->InputByte[lane] = stream[position];
                r->InputFull[lane] = uint.MaxValue;
                position++;
                served++;
            }
            else if (blocked)
            {
                continue;
            }

            var state = HandOver(r, lane, control, ref slots, ref symbols);

            if (control >= LaneRegisters.Processing)
            {
                Process(r, lane, control, state);
            }
            else if (!r->Header(lane, target))
            {
                return lane;
            }
        }

        return LaneClock.NoLane;
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static bool NeedsByte(LaneRegisters* r, int lane, uint control)
    {
        if (r->InputFull[lane] != 0)
        {
            return false;
        }

        if (control == LaneRegisters.Processing)
        {
            return r->InputLeft[lane] != 0;
        }

        return control >= LaneRegisters.Header0 && control <= LaneRegisters.Header2;
    }

    /// <summary>Phase 0: the state update from the held symbol, and its hand-off to the symbol vector.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static uint HandOver(LaneRegisters* r, int lane, uint control, ref uint slots, ref long symbols)
    {
        var state = r->RansState[lane];

        if (r->SymbolFull[lane] == 0)
        {
            return state;
        }

        if (control >= LaneRegisters.Processing)
        {
            state = RansModels.Advance(r->SymbolBase[lane], state, r->Symbol[lane]);
        }

        r->Released[lane] = r->Symbol[lane];
        r->SymbolFull[lane] = 0;
        slots |= 1u << lane;
        symbols++;

        return state;
    }

    /// <summary>Phase 1 in PROCESSING or DRAINING, from the phase-0 state: renormalise one byte, decode a symbol.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static void Process(LaneRegisters* r, int lane, uint control, uint state)
    {
        var processing = control == LaneRegisters.Processing;
        var renormalise = processing && state < LaneRegisters.LowerBound && r->InputLeft[lane] != 0 && r->InputFull[lane] != 0;
        var x = state;

        if (renormalise)
        {
            x = (state << LaneRegisters.ByteShift) | r->InputByte[lane];
            r->InputLeft[lane]--;
            r->InputFull[lane] = 0;
        }

        var decodes = x > LaneRegisters.LowerBound && r->SymbolsLeft[lane] != 0;
        r->RansState[lane] = x;
        r->SymbolFull[lane] = decodes ? uint.MaxValue : 0;

        if (decodes)
        {
            r->Symbol[lane] = RansModels.Inverse(r->SegmentBase[lane], x & RansModels.Low16);
            r->SymbolsLeft[lane]--;
        }

        var ended = processing ? r->InputLeft[lane] == 0 : x <= LaneRegisters.LowerBound;

        if (ended)
        {
            r->Transition(lane);
        }
    }
}
