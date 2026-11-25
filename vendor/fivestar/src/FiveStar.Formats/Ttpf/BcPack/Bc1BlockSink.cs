// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The BC1 lane meanings follow bcpack-rs src/bc1.rs (MIT); see THIRD_PARTY_NOTICES.md.

using System.Buffers.Binary;
using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Turns each symbol vector into one BC1 block. Lane 0 is a back-reference distance d (zigzag, mod 256): the block's
/// endpoints start from those of block i - d, or from the preset window when that lies before block 0. Lanes 2-7 add
/// zigzag residuals to R0 G0 B0 R1 G1 B1, each wrapping in its field. Lanes 8-11 pick the 2x2 index groups TL, TR, BL
/// and BR from the codebook. Lane 1 is unused.
/// </summary>
internal unsafe struct Bc1BlockSink : ISymbolVectorSink
{
    /// <summary>Blocks a back reference reaches, and the slots of the preset window.</summary>
    public const int RingSize = 256;

    /// <summary>Residual symbols that matter: a field of w bits wraps every 2^(w + 1) zigzag codes, at most 128.</summary>
    public const int ResidualCodes = 128;

    // Lane 0 codes the reference distance, lanes 2-7 the residuals, lanes 8-11 the index groups; lane 1 is unused.
    private const int DistanceLane = 0;
    private const int Red0Lane = 2;
    private const int Green0Lane = 3;
    private const int Blue0Lane = 4;
    private const int Red1Lane = 5;
    private const int Green1Lane = 6;
    private const int Blue1Lane = 7;
    private const int TopLeftLane = 8;
    private const int TopRightLane = 9;
    private const int BottomLeftLane = 10;
    private const int BottomRightLane = 11;

    // Rows of the residual table (by field) and of the codebook masks (by quadrant).
    private const uint Green0Row = ResidualCodes;
    private const uint Blue0Row = 2 * ResidualCodes;
    private const uint Red1Row = 3 * ResidualCodes;
    private const uint Green1Row = 4 * ResidualCodes;
    private const uint Blue1Row = 5 * ResidualCodes;
    private const uint TopRightRow = Bc1Codebook.Slots;
    private const uint BottomLeftRow = 2 * Bc1Codebook.Slots;
    private const uint BottomRightRow = 3 * Bc1Codebook.Slots;
    private const uint ResidualMask = ResidualCodes - 1;
    private const uint ByteMask = RingSize - 1;
    private const int IndexShift = 32;

    private readonly byte* _output;
    private readonly uint* _ring;
    private readonly byte* _unfitted;
    private readonly uint* _quadrants;
    private readonly uint* _residuals;

    /// <param name="output">Room for every block.</param>
    /// <param name="ring">The preset window, then the endpoints of the last 256 blocks.</param>
    /// <param name="unfitted">1 where the ring entry descends from an unfitted window slot.</param>
    /// <param name="quadrants">The codebook's quadrant masks (<see cref="Bc1Codebook.QuadMasks"/>).</param>
    /// <param name="residuals"><c>[field * 128 + symbol]</c>: the residual placed in its field (<see cref="Bc1Decoder"/>).</param>
    public Bc1BlockSink(byte* output, uint* ring, byte* unfitted, uint* quadrants, uint* residuals)
    {
        _output = output;
        _ring = ring;
        _unfitted = unfitted;
        _quadrants = quadrants;
        _residuals = residuals;
        UnfittedBlocks = 0;
    }

    public int UnfittedBlocks { get; private set; }

    /// <summary>
    /// The ring starts as the preset window, so a reference before block 0 (slot (i - d) mod 256 above i) reads the
    /// window and every other reference reads block i - d.
    /// </summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public void Emit(int block, uint* symbols)
    {
        var code = symbols[DistanceLane] & ByteMask;
        var distance = ((code >> 1) ^ (0u - (code & 1))) & ByteMask;
        var slot = (block - (int)distance) & (RingSize - 1);
        uint predicted = 0;
        byte unfitted = 0;

        if (distance != 0)
        {
            predicted = _ring[slot];
            unfitted = _unfitted[slot];
        }

        var delta = Residuals(symbols);
        var endpoints = ((predicted & ~Bc1Fields.TopBits) + (delta & ~Bc1Fields.TopBits)) ^ ((predicted ^ delta) & Bc1Fields.TopBits);
        _ring[block & (RingSize - 1)] = endpoints;
        _unfitted[block & (RingSize - 1)] = unfitted;
        UnfittedBlocks += unfitted;

        var indices = Indices(symbols);
        var target = new Span<byte>(_output + ((long)block * Bc1Decoder.BlockSize), Bc1Decoder.BlockSize);
        BinaryPrimitives.WriteUInt64LittleEndian(target, endpoints | ((ulong)indices << IndexShift));
    }

    /// <summary>The six residuals, each placed in its field: one table row per field.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private readonly uint Residuals(uint* symbols)
    {
        var red0 = _residuals[symbols[Red0Lane] & ResidualMask];
        var green0 = _residuals[Green0Row + (symbols[Green0Lane] & ResidualMask)];
        var blue0 = _residuals[Blue0Row + (symbols[Blue0Lane] & ResidualMask)];
        var red1 = _residuals[Red1Row + (symbols[Red1Lane] & ResidualMask)];
        var green1 = _residuals[Green1Row + (symbols[Green1Lane] & ResidualMask)];
        var blue1 = _residuals[Blue1Row + (symbols[Blue1Lane] & ResidualMask)];

        return red0 | green0 | blue0 | red1 | green1 | blue1;
    }

    /// <summary>The 32-bit index word from the four 2x2 groups: one codebook row per quadrant.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private readonly uint Indices(uint* symbols)
    {
        var topLeft = _quadrants[symbols[TopLeftLane] & ByteMask];
        var topRight = _quadrants[TopRightRow + (symbols[TopRightLane] & ByteMask)];
        var bottomLeft = _quadrants[BottomLeftRow + (symbols[BottomLeftLane] & ByteMask)];
        var bottomRight = _quadrants[BottomRightRow + (symbols[BottomRightLane] & ByteMask)];

        return topLeft | topRight | bottomLeft | bottomRight;
    }
}
