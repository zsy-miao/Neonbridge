// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The BC4 lane meanings follow the GTA6-Asset-Info notes (formats/06-bcpack.md, GPL-3.0), checked on the dump; see
// THIRD_PARTY_NOTICES.md.

using System.Buffers.Binary;
using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Turns each symbol vector of a BC4 page into one BC4 block. Lane 0 is a back-reference distance d (zigzag, mod 256):
/// the endpoints start from block i - d, from a preset slot when that lies before block 0, or from 128 when d is 0.
/// Lanes 2 and 3 add zigzag residuals to e0 and e1, modulo 256. Lane 4 + k codes pixels 2k and 2k + 1 through the pair
/// table. Lane 1 is unused.
/// </summary>
internal unsafe struct Bc4BlockSink : ISymbolVectorSink
{
    private const int EndpointShift = RgtcSymbols.ByteBits;

    private readonly byte* _output;
    private readonly uint* _ring;
    private readonly byte* _unfitted;
    private readonly uint* _pairs;

    /// <param name="output">Room for every block.</param>
    /// <param name="ring">The preset slots, then the endpoints of the last 256 blocks.</param>
    /// <param name="unfitted">1 where the ring entry descends from an unfitted preset slot.</param>
    /// <param name="pairs">The pair table (<see cref="RgtcTables.PairCodes"/>).</param>
    public Bc4BlockSink(byte* output, uint* ring, byte* unfitted, uint* pairs)
    {
        _output = output;
        _ring = ring;
        _unfitted = unfitted;
        _pairs = pairs;
        UnfittedBlocks = 0;
    }

    public int UnfittedBlocks { get; private set; }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public void Emit(int block, uint* symbols)
    {
        var distance = RgtcSymbols.Unzigzag(symbols[RgtcSymbols.DistanceLane]);
        var slot = (block - (int)distance) & (RgtcSymbols.RingSize - 1);
        var predicted = RgtcSymbols.Midpoint;
        byte unfitted = 0;

        if (distance != 0)
        {
            predicted = _ring[slot];
            unfitted = _unfitted[slot];
        }

        var endpoints = RgtcSymbols.AddResidual(predicted, 0, symbols[RgtcSymbols.EndpointLane]) |
            RgtcSymbols.AddResidual(predicted, EndpointShift, symbols[RgtcSymbols.EndpointLane + 1]);
        _ring[block & (RgtcSymbols.RingSize - 1)] = endpoints;
        _unfitted[block & (RgtcSymbols.RingSize - 1)] = unfitted;
        UnfittedBlocks += unfitted;

        ulong indices = 0;

        for (var pair = 0; pair < RgtcSymbols.IndexLanes; pair++)
        {
            indices |= (ulong)_pairs[symbols[RgtcSymbols.IndexLane + pair] & RgtcSymbols.PairMask] << (pair * RgtcSymbols.PairBits);
        }

        var target = new Span<byte>(_output + ((long)block * sizeof(ulong)), sizeof(ulong));
        BinaryPrimitives.WriteUInt64LittleEndian(target, endpoints | (indices << RgtcSymbols.IndexShift));
    }
}
