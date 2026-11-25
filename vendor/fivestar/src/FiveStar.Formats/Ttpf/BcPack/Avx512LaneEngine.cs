// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The same clocked lane model as ScalarLaneEngine (after bcpack-rs and Microsoft silicon-rans, both MIT; see
// THIRD_PARTY_NOTICES.md), for all lanes at once.

using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.Intrinsics;
using System.Runtime.Intrinsics.X86;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The AVX-512 lane engine: one clock of all 12 or 16 lanes is one 16-lane vector step on registers kept in
/// <see cref="Avx512Lanes"/>. The feeder grant is one PDEP (<see cref="ClockMasks"/>) and the granted lanes take their
/// bytes with one VPEXPANDB. There are no gathers: masked selects over each lane's model rows give the inverse's
/// constants and the decoded symbol's forward constants for the next clock.
/// </summary>
internal static unsafe class Avx512LaneEngine
{
    private static readonly Vector128<byte> ByteBits = Vector128.Create((byte)1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128);

    private static readonly Vector128<byte> ByteSpread = Vector128.Create((byte)0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1);

    public static bool IsSupported =>
        Avx512F.IsSupported && Avx512BW.IsSupported && Avx512Vbmi2.VL.IsSupported && Bmi2.IsSupported && Popcnt.IsSupported;

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
        var lanes = Avx512Lanes.Load(r);

        var position = 0;
        var released = 0;
        long symbols = 0;
        uint slots = 0;

        for (long cycle = 0; cycle < maxCycles;)
        {
            var masks = lanes.Masks(r, slots, LaneClock.Available(position, length));
            var ends = lanes.Step(r, Granted(stream, position, length, masks.Grant), masks);

            position += BitOperations.PopCount(masks.Grant);
            slots |= masks.Push;
            symbols += BitOperations.PopCount(masks.Push);

            if ((ends | masks.Slow) != 0)
            {
                lanes.Spill(r);
                var bad = LaneClock.ControlSteps(r, ends, masks.Slow, target);

                if (bad != LaneClock.NoLane)
                {
                    return LaneClock.BadHeader(position, masks.Grant, length, cycle + 1, bad);
                }

                lanes.Reload(r);
            }

            // When every slot is full the vector goes out (see ScalarLaneEngine for why this is the reference rule).
            if (slots == r->AllLanes)
            {
                lanes.Release(r);
                sink.Emit(released, r->Released);
                released++;
                slots = 0;
            }

            cycle++;

            if (r->FinishedLanes == r->Lanes && lanes.SymbolFullLanes == 0)
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

    /// <summary>The next stream bytes moved to the granted lanes: stream byte k to the k-th granted lane.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static Vector128<byte> Granted(byte* stream, int position, int length, uint grant)
    {
        return Avx512Vbmi2.VL.Expand(Vector128<byte>.Zero, ByteMask(grant), LaneClock.Window(stream, position, length));
    }

    /// <summary>A lane bit mask as all ones or zero per byte.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static Vector128<byte> ByteMask(uint bits)
    {
        var spread = Ssse3.Shuffle(Vector128.CreateScalarUnsafe(bits).AsByte(), ByteSpread);

        return Vector128.Equals(spread & ByteBits, ByteBits);
    }
}
