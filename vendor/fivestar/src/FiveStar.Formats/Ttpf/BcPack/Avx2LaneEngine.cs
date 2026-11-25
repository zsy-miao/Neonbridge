// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The same clocked lane model as ScalarLaneEngine (after bcpack-rs and Microsoft silicon-rans, both MIT; see
// THIRD_PARTY_NOTICES.md), evaluated for eight lanes at once.

using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.Intrinsics;
using System.Runtime.Intrinsics.X86;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The AVX2 lane engine: one clock of all 12 or 16 lanes is two 8-lane vector steps. The feeder grant is one PDEP
/// (<see cref="ClockMasks"/>), and a PSHUFB with a table per grant mask moves stream byte k to the k-th granted lane.
/// Lanes in PROCESSING or DRAINING, almost every lane on almost every clock, run as vectors whose result is written only
/// for lanes that do not stall; lanes in IDLE, a header state or at the end of a fragment take the shared control steps.
/// </summary>
internal static unsafe class Avx2LaneEngine
{
    private const int Half = 8;
    private const uint HalfMask = 0xFF;
    private const int ControlBytes = 16;
    private const int ExpandEntries = 1 << Half;
    private const int Alignment = 64;
    private const byte GatherScale = sizeof(uint);

    // A shuffle control byte with the top bit set reads as zero.
    private const byte ZeroByte = 0x80;

    // A state whose top byte is zero is below L and takes one byte.
    private const int TopByteShift = 24;

    /// <summary><c>[mask]</c>: the PSHUFB control that puts the k-th stream byte into the lane of the k-th set bit.</summary>
    private static readonly Vector128<byte>* Expand = BuildExpand();

    private static readonly Vector256<uint> LaneBits = Vector256.Create(1u, 2, 4, 8, 16, 32, 64, 128);

    public static bool IsSupported => Avx2.IsSupported && Bmi2.IsSupported && Popcnt.IsSupported;

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

    // Optimised on the first call: a decode is one long loop, and tier-0 code would run the first pages slowly.
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
            var masks = Masks(r, slots, LaneClock.Available(position, length));
            var source = LaneClock.Window(stream, position, length);
            var processing = r->ProcessingLanes;
            var ends = Step(r, 0, source, masks, processing) | (Step(r, Half, source, masks, processing) << Half);

            position += BitOperations.PopCount(masks.Grant);
            slots |= masks.Push;
            symbols += BitOperations.PopCount(masks.Push);

            var bad = LaneClock.ControlSteps(r, ends, masks.Slow, target);

            if (bad != LaneClock.NoLane)
            {
                return LaneClock.BadHeader(position, masks.Grant, length, cycle + 1, bad);
            }

            // When every slot is full the vector goes out (see ScalarLaneEngine for why this is the reference rule).
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

    /// <summary>The feeder's masks: lanes with no input byte that are processing with input left or reading a header.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static ClockMasks Masks(LaneRegisters* r, uint slots, int available)
    {
        var inputFull = Bits(r->InputFull);
        var symbolFull = Bits(r->SymbolFull);
        var noInputLeft = Bits(r->InputLeft, Vector256<uint>.Zero);
        var need = ~inputFull & ((r->ProcessingLanes & ~noInputLeft) | r->HeaderLanes);

        return ClockMasks.Compute(need, symbolFull, slots, available, r->ProcessingLanes | r->DrainingLanes, r->AllLanes);
    }

    /// <summary>
    /// Lanes <paramref name="first"/> to first + 7: byte delivery and symbol hand-off for every lane, then phase 1 of
    /// PROCESSING and DRAINING written where the lane commits. Returns the lanes (bit 0 = lane <paramref name="first"/>)
    /// whose fragment input or drain ended.
    /// </summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static uint Step(LaneRegisters* r, int first, Vector128<byte> source, ClockMasks masks, uint processing)
    {
        var grant = (masks.Grant >> first) & HalfMask;
        var control = Expand[grant];

        // The upper half skips the bytes the lower half took; zero entries stay at 0x80 or above and still read as zero.
        if (first != 0)
        {
            control += Vector128.Create((byte)BitOperations.PopCount(masks.Grant & HalfMask));
        }

        var bytes = Avx2.ConvertToVector256Int32(Ssse3.Shuffle(source, control)).AsUInt32();
        var state = HandOver(r, first, bytes, Mask(grant), Mask((masks.Push >> first) & HalfMask));

        return Process(r, first, state, Mask((masks.Commit >> first) & HalfMask), Mask((processing >> first) & HalfMask));
    }

    /// <summary>Phase 0: the state update from the held symbol, the byte in, the hand-off; returns the phase-0 state.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static Vector256<uint> HandOver(LaneRegisters* r, int first, Vector256<uint> bytes, Vector256<uint> grant, Vector256<uint> push)
    {
        var state = Vector256.Load(r->RansState + first);
        var symbol = Vector256.Load(r->Symbol + first);
        var symbolFull = Vector256.Load(r->SymbolFull + first);
        var index = (Vector256.Load(r->SymbolBase + first) + symbol).AsInt32();
        var forward = Avx2.GatherVector256(RansModels.Forward, index, GatherScale);
        var low16 = Vector256.Create(RansModels.Low16);

        var advanced = Avx2.MultiplyLow(forward & low16, state >> RansModels.HalfShift) + (state & low16) - (forward >> RansModels.HalfShift);
        Vector256.ConditionalSelect(push, symbol, Vector256.Load(r->Released + first)).Store(r->Released + first);
        Vector256.AndNot(symbolFull, push).Store(r->SymbolFull + first);
        Vector256.ConditionalSelect(grant, bytes, Vector256.Load(r->InputByte + first)).Store(r->InputByte + first);
        (Vector256.Load(r->InputFull + first) | grant).Store(r->InputFull + first);

        return Vector256.ConditionalSelect(symbolFull, advanced, state);
    }

    /// <summary>Phase 1: renormalise one byte and decode, written for the <paramref name="commit"/> lanes.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static uint Process(LaneRegisters* r, int first, Vector256<uint> state, Vector256<uint> commit, Vector256<uint> processing)
    {
        var inputFull = Vector256.Load(r->InputFull + first);
        var inputLeft = Vector256.Load(r->InputLeft + first);
        var symbolsLeft = Vector256.Load(r->SymbolsLeft + first);
        var top = Vector256.Equals(state >> TopByteShift, Vector256<uint>.Zero);
        var renormalise = processing & commit & inputFull & top & ~Vector256.Equals(inputLeft, Vector256<uint>.Zero);
        var x = Vector256.ConditionalSelect(renormalise, (state << LaneRegisters.ByteShift) | Vector256.Load(r->InputByte + first), state);

        var above = Vector256.GreaterThan(x, Vector256.Create(LaneRegisters.LowerBound));
        var decodes = above & ~Vector256.Equals(symbolsLeft, Vector256<uint>.Zero) & commit;
        var symbol = Inverse(r, first, x & Vector256.Create(RansModels.Low16));

        Vector256.ConditionalSelect(commit, x, Vector256.Load(r->RansState + first)).Store(r->RansState + first);
        Vector256.ConditionalSelect(commit, symbol, Vector256.Load(r->Symbol + first)).Store(r->Symbol + first);
        Vector256.ConditionalSelect(commit, decodes, Vector256.Load(r->SymbolFull + first)).Store(r->SymbolFull + first);

        // Adding an all-ones mask subtracts one.
        (symbolsLeft + decodes).Store(r->SymbolsLeft + first);
        inputLeft += renormalise;
        inputLeft.Store(r->InputLeft + first);
        Vector256.AndNot(inputFull, renormalise).Store(r->InputFull + first);

        var end = Vector256.ConditionalSelect(processing, Vector256.Equals(inputLeft, Vector256<uint>.Zero), ~above);

        return Vector256.ExtractMostSignificantBits(end & commit);
    }

    /// <summary>The reciprocal-divide inverse for eight lanes, each with its own model.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static Vector256<uint> Inverse(LaneRegisters* r, int first, Vector256<uint> cf)
    {
        // segment = thresholds reached = 8 - thresholds above cf (a true compare is -1).
        var value = cf.AsInt32();
        var above = Vector256<int>.Zero;

        for (var k = 0; k < RansModels.Bc1Thresholds; k++)
        {
            above += Avx2.CompareGreaterThan(Vector256.Load(r->ThresholdRows + (k * LaneRegisters.Width) + first).AsInt32(), value);
        }

        var index = Vector256.Load(r->SegmentBase + first).AsInt32() + Vector256.Create(RansModels.Bc1Thresholds) + above;
        var a = Avx2.GatherVector256(RansModels.SegA, index, GatherScale);
        var b = Avx2.GatherVector256(RansModels.SegB, index, GatherScale);
        var low16 = Vector256.Create(RansModels.Low16);
        var symbolMask = Vector256.Create(RansModels.SymbolMask);
        var steps = Avx2.ShiftRightLogicalVariable(Avx2.MultiplyLow((cf + a) & low16, a >> RansModels.HalfShift), b >> RansModels.HalfShift);

        return ((b & low16) + (steps & symbolMask)) & symbolMask;
    }

    /// <summary>A bit mask of lanes 0-7 as all ones or zero per lane.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static Vector256<uint> Mask(uint bits)
    {
        return Vector256.Equals(Vector256.Create(bits) & LaneBits, LaneBits);
    }

    /// <summary>The top bit of each of the 16 lane values (zero in inert lanes).</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static uint Bits(uint* field)
    {
        var low = Vector256.ExtractMostSignificantBits(Vector256.Load(field));
        var high = Vector256.ExtractMostSignificantBits(Vector256.Load(field + Half));

        return low | (high << Half);
    }

    /// <summary>The lanes whose value equals <paramref name="value"/>; the caller masks out inert lanes.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static uint Bits(uint* field, Vector256<uint> value)
    {
        var low = Vector256.ExtractMostSignificantBits(Vector256.Equals(Vector256.Load(field), value));
        var high = Vector256.ExtractMostSignificantBits(Vector256.Equals(Vector256.Load(field + Half), value));

        return low | (high << Half);
    }

    private static Vector128<byte>* BuildExpand()
    {
        var table = (Vector128<byte>*)NativeMemory.AlignedAlloc((nuint)(ExpandEntries * sizeof(Vector128<byte>)), Alignment);

        for (var mask = 0; mask < ExpandEntries; mask++)
        {
            var entry = (byte*)(table + mask);
            byte rank = 0;

            for (var index = 0; index < ControlBytes; index++)
            {
                entry[index] = ZeroByte;

                if (index < Half && (mask & (1 << index)) != 0)
                {
                    entry[index] = rank;
                    rank++;
                }
            }
        }

        return table;
    }
}
