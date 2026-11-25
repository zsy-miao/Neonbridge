// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The BC5 lane meanings follow the GTA6-Asset-Info notes (formats/06-bcpack.md, GPL-3.0), checked on the dump; see
// THIRD_PARTY_NOTICES.md.

using System.Buffers.Binary;
using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Turns each pair of symbol vectors of a BC5 page (E, then O) into one BC5 block. Lane 0 is the back-reference
/// distance: E and O interleaved bit by bit (E on the even bits), then zigzag. Lane 2 codes channel 0's e0 (E) and e1
/// (O) residuals, lane 3 channel 1's; distance 0 predicts 128. Lanes 4 + r (channel 0) and 8 + r (channel 1) code row
/// r: E the pixels in columns 0 and 1, O those in columns 2 and 3, through the pair table. Lane 1 is unused.
/// </summary>
internal unsafe struct Bc5BlockSink : ISymbolVectorSink
{
    private const int Lanes = 12;
    private const int ChannelLane1 = RgtcSymbols.EndpointLane + 1;
    private const int RowsPerChannel = 4;
    private const int Channel1IndexLane = RgtcSymbols.IndexLane + RowsPerChannel;
    private const int ChannelBytes = 2 * RgtcSymbols.ByteBits;
    private const int E1Shift = RgtcSymbols.ByteBits;
    private const int Channel1E0Shift = 2 * RgtcSymbols.ByteBits;
    private const int Channel1E1Shift = 3 * RgtcSymbols.ByteBits;
    private const uint ChannelMask = 0xFFFF;
    private const int BlockBytes = 2 * sizeof(ulong);
    private const int RowBits = 2 * RgtcSymbols.PairBits;

    private readonly byte* _output;
    private readonly uint* _ring;
    private readonly byte* _unfitted;
    private readonly uint* _pairs;
    private fixed uint _even[Lanes];

    /// <param name="output">Room for every block.</param>
    /// <param name="ring">The preset slots, then the endpoints of the last 256 blocks.</param>
    /// <param name="unfitted">1 where the ring entry descends from an unfitted preset slot.</param>
    /// <param name="pairs">The pair table (<see cref="RgtcTables.PairCodes"/>).</param>
    public Bc5BlockSink(byte* output, uint* ring, byte* unfitted, uint* pairs)
    {
        _output = output;
        _ring = ring;
        _unfitted = unfitted;
        _pairs = pairs;
        UnfittedBlocks = 0;
    }

    public int UnfittedBlocks { get; private set; }

    /// <summary>Keeps the E vector; the O vector that follows completes the block.</summary>
    [HotPath]
    public void Emit(int vector, uint* symbols)
    {
        if ((vector & 1) == 0)
        {
            for (var lane = 0; lane < Lanes; lane++)
            {
                _even[lane] = symbols[lane];
            }

            return;
        }

        var block = vector >> 1;
        var code = RgtcSymbols.Interleave(_even[RgtcSymbols.DistanceLane], symbols[RgtcSymbols.DistanceLane]);
        var distance = RgtcSymbols.Unzigzag(code);
        var slot = (block - (int)distance) & (RgtcSymbols.RingSize - 1);
        var predicted = RgtcSymbols.Midpoint;
        byte unfitted = 0;

        if (distance != 0)
        {
            predicted = _ring[slot];
            unfitted = _unfitted[slot];
        }

        var endpoints = Endpoints(predicted, symbols);
        _ring[block & (RgtcSymbols.RingSize - 1)] = endpoints;
        _unfitted[block & (RgtcSymbols.RingSize - 1)] = unfitted;
        UnfittedBlocks += unfitted;

        var target = _output + ((long)block * BlockBytes);
        Write(target, endpoints & ChannelMask, Indices(RgtcSymbols.IndexLane, symbols));
        Write(target + sizeof(ulong), endpoints >> ChannelBytes, Indices(Channel1IndexLane, symbols));
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static void Write(byte* target, uint endpoints, ulong indices)
    {
        BinaryPrimitives.WriteUInt64LittleEndian(new Span<byte>(target, sizeof(ulong)), endpoints | (indices << RgtcSymbols.IndexShift));
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private readonly uint Endpoints(uint predicted, uint* odd)
    {
        return RgtcSymbols.AddResidual(predicted, 0, _even[RgtcSymbols.EndpointLane]) |
            RgtcSymbols.AddResidual(predicted, E1Shift, odd[RgtcSymbols.EndpointLane]) |
            RgtcSymbols.AddResidual(predicted, Channel1E0Shift, _even[ChannelLane1]) |
            RgtcSymbols.AddResidual(predicted, Channel1E1Shift, odd[ChannelLane1]);
    }

    /// <summary>The 48-bit index field of one channel: per row, E's pair then O's.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private readonly ulong Indices(int firstLane, uint* odd)
    {
        ulong indices = 0;

        for (var row = 0; row < RowsPerChannel; row++)
        {
            var left = _pairs[_even[firstLane + row] & RgtcSymbols.PairMask];
            var right = _pairs[odd[firstLane + row] & RgtcSymbols.PairMask];
            indices |= ((ulong)left | ((ulong)right << RgtcSymbols.PairBits)) << (row * RowBits);
        }

        return indices;
    }
}
