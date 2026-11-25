// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The same clocked lane model as ScalarLaneEngine (after bcpack-rs and Microsoft silicon-rans, both MIT; see
// THIRD_PARTY_NOTICES.md), for all lanes at once.

using System.Runtime.CompilerServices;
using System.Runtime.Intrinsics;
using System.Runtime.Intrinsics.X86;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The lane registers the AVX-512 engine keeps in vector registers across clocks, one 16-lane vector per field (slots
/// past the codec's lanes are never granted, pushed or written, so they stay inert). They go through <see cref="LaneRegisters"/> only
/// on clocks where a lane takes a control step, and when a symbol vector is released.
/// </summary>
internal unsafe struct Avx512Lanes
{
    // A state whose top byte is zero is below L and takes one byte.
    private const int TopByteShift = 24;

    private static readonly Vector512<uint> LaneBits =
        Vector512.Create(1u, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768);

    private static readonly Vector512<uint> Low16 = Vector512.Create(RansModels.Low16);

    private static readonly Vector512<uint> SymbolMask = Vector512.Create(RansModels.SymbolMask);

    private static readonly Vector512<uint> LowerBound = Vector512.Create(LaneRegisters.LowerBound);

    private Vector512<uint> _state;
    private Vector512<uint> _symbol;
    private Vector512<uint> _forward;
    private Vector512<uint> _symbolFull;
    private Vector512<uint> _inputByte;
    private Vector512<uint> _inputFull;
    private Vector512<uint> _inputLeft;
    private Vector512<uint> _symbolsLeft;
    private Vector512<uint> _released;

    /// <summary>Gets the lanes holding a symbol.</summary>
    public readonly uint SymbolFullLanes => Bits(_symbolFull);

    /// <summary>The registers as a reset decoder holds them; no lane holds a symbol, so the forward constants are unused.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static Avx512Lanes Load(LaneRegisters* r)
    {
        var lanes = default(Avx512Lanes);
        lanes._symbol = Vector512.Load(r->Symbol);
        lanes._symbolFull = Vector512.Load(r->SymbolFull);
        lanes._released = Vector512.Load(r->Released);
        lanes.Reload(r);

        return lanes;
    }

    /// <summary>The feeder's masks: lanes with no input byte that are processing with input left or reading a header need one.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public readonly ClockMasks Masks(LaneRegisters* r, uint slots, int available)
    {
        var noInputLeft = Bits(Vector512.Equals(_inputLeft, Vector512<uint>.Zero));
        var need = ~Bits(_inputFull) & ((r->ProcessingLanes & ~noInputLeft) | r->HeaderLanes);

        return ClockMasks.Compute(need, SymbolFullLanes, slots, available, r->ProcessingLanes | r->DrainingLanes, r->AllLanes);
    }

    /// <summary>
    /// One clock of every lane in PROCESSING or DRAINING: phase 0 (state update from the held symbol, byte in,
    /// hand-off) and phase 1 (renormalise, decode), written for the lanes in <see cref="ClockMasks.Commit"/>. Returns the
    /// lanes whose fragment input or drain ended.
    /// </summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public uint Step(LaneRegisters* r, Vector128<byte> bytes, ClockMasks masks)
    {
        var grant = Mask(masks.Grant);
        var push = Mask(masks.Push);
        var commit = Mask(masks.Commit);
        var processing = Mask(r->ProcessingLanes);

        var advanced = ((_forward & Low16) * (_state >> RansModels.HalfShift)) + (_state & Low16) - (_forward >> RansModels.HalfShift);
        var state = Vector512.ConditionalSelect(_symbolFull, advanced, _state);
        _released = Vector512.ConditionalSelect(push, _symbol, _released);
        _symbolFull = Vector512.AndNot(_symbolFull, push);
        _inputByte = Vector512.ConditionalSelect(grant, Avx512F.ConvertToVector512UInt32(bytes), _inputByte);
        _inputFull |= grant;

        var top = Vector512.Equals(state >> TopByteShift, Vector512<uint>.Zero);
        var renormalise = processing & commit & _inputFull & top & ~Vector512.Equals(_inputLeft, Vector512<uint>.Zero);
        var x = Vector512.ConditionalSelect(renormalise, (state << LaneRegisters.ByteShift) | _inputByte, state);
        var above = Vector512.GreaterThan(x, LowerBound);
        var decodes = above & ~Vector512.Equals(_symbolsLeft, Vector512<uint>.Zero) & commit;
        var (symbol, forward) = Inverse(r, x & Low16);

        _symbol = Vector512.ConditionalSelect(commit, symbol, _symbol);
        _forward = Vector512.ConditionalSelect(commit, forward, _forward);
        _state = Vector512.ConditionalSelect(commit, x, _state);
        _symbolFull = Vector512.ConditionalSelect(commit, decodes, _symbolFull);

        // Adding an all-ones mask subtracts one.
        _symbolsLeft += decodes;
        _inputLeft += renormalise;
        _inputFull = Vector512.AndNot(_inputFull, renormalise);
        var ended = Vector512.ConditionalSelect(processing, Vector512.Equals(_inputLeft, Vector512<uint>.Zero), ~above);

        return Bits(ended & commit);
    }

    /// <summary>Writes the registers the control steps read and write.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public readonly void Spill(LaneRegisters* r)
    {
        _state.Store(r->RansState);
        _symbol.Store(r->Symbol);
        _symbolFull.Store(r->SymbolFull);

        _inputByte.Store(r->InputByte);
        _inputFull.Store(r->InputFull);
        _inputLeft.Store(r->InputLeft);
        _symbolsLeft.Store(r->SymbolsLeft);
    }

    /// <summary>Reads back the registers the control steps may have changed.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public void Reload(LaneRegisters* r)
    {
        _state = Vector512.Load(r->RansState);
        _inputByte = Vector512.Load(r->InputByte);
        _inputFull = Vector512.Load(r->InputFull);
        _inputLeft = Vector512.Load(r->InputLeft);
        _symbolsLeft = Vector512.Load(r->SymbolsLeft);
    }

    /// <summary>Writes the symbol vector for release.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public readonly void Release(LaneRegisters* r)
    {
        _released.Store(r->Released);
    }

    /// <summary>
    /// The reciprocal-divide inverse, and the decoded symbol's forward constants (as in <see cref="RansModels.Forward"/>)
    /// for the next clock's state update. The segment is the number of thresholds reached and the thresholds never
    /// decrease, so taking row k + 1 wherever threshold k is reached leaves the segment's row. The decoded symbol always
    /// lies in that segment (the tests check every model and input), so its (freq, base) row is the same.
    /// </summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static (Vector512<uint> Symbol, Vector512<uint> Forward) Inverse(LaneRegisters* r, Vector512<uint> cf)
    {
        var a = Vector512.Load(r->SegARows);
        var b = Vector512.Load(r->SegBRows);
        var row = Vector512.Load(r->ForwardRows);

        for (var k = 0; k < RansModels.Bc1Thresholds; k++)
        {
            var reached = Vector512.GreaterThanOrEqual(cf, Vector512.Load(r->ThresholdRows + (k * LaneRegisters.Width)));
            var next = (k + 1) * LaneRegisters.Width;
            a = Vector512.ConditionalSelect(reached, Vector512.Load(r->SegARows + next), a);
            b = Vector512.ConditionalSelect(reached, Vector512.Load(r->SegBRows + next), b);
            row = Vector512.ConditionalSelect(reached, Vector512.Load(r->ForwardRows + next), row);
        }

        var first = b & Low16;
        var steps = Avx512F.ShiftRightLogicalVariable(((cf + a) & Low16) * (a >> RansModels.HalfShift), b >> RansModels.HalfShift);

        var symbol = (first + (steps & SymbolMask)) & SymbolMask;

        // RansModels.ForwardExact: cum = base + freq * (symbol - first); the reserved slot 0 gives (freq - 1, 1).
        var freq = row & Low16;
        var cum = ((row >> RansModels.HalfShift) + (freq * (symbol - first))) & Low16;
        var slotZero = Vector512.Equals(cum, Vector512<uint>.Zero);
        freq = Vector512.ConditionalSelect(slotZero, (freq - Vector512<uint>.One) & Low16, freq);
        cum = Vector512.ConditionalSelect(slotZero, Vector512<uint>.One, cum);

        return (symbol, freq | (cum << RansModels.HalfShift));
    }

    /// <summary>A lane bit mask as all ones or zero per 32-bit lane.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static Vector512<uint> Mask(uint bits)
    {
        return Vector512.Equals(Vector512.Create(bits) & LaneBits, LaneBits);
    }

    /// <summary>The top bit of each of the 16 lane values; the callers mask out inert lanes where they could be set.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static uint Bits(Vector512<uint> value)
    {
        return (uint)Vector512.ExtractMostSignificantBits(value);
    }
}
