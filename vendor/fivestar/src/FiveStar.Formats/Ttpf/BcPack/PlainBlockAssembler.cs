// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The lane meanings and rank tables follow VIctory (crates/victory-formats/src/bcpack.rs, GPL-3.0); see
// THIRD_PARTY_NOTICES.md.

using System.Buffers.Binary;
using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The block layer of BC1, BC3, BC4 and BC5 (<see cref="PlainEntry"/>). Each endpoint field and index is the
/// referenced entry's value plus a zigzag residual, modulo the field's width. Residual lanes per codec:
/// <list type="bullet">
/// <item>BC1: lanes 2-7 R0 G0 B0 R1 G1 B1; lanes 8-11 one 2x2 index group each (TL, TR, BL, BR) through the BC1 rank table.</item>
/// <item>BC3: lane 2 alpha e0 e1; lanes 3-6 two alpha indices per symbol through the alpha rank table; lanes 7-9 R, G
/// and B (E endpoint 0, O endpoint 1); lanes 10-11 the colour 2x2 groups through the colour rank table.</item>
/// <item>BC4: lanes 2-3 e0 and e1; lanes 4-11 two indices per symbol.</item>
/// <item>BC5: lanes 2-3 channel 0 and 1 (E e0, O e1); lanes 4-11 two indices per symbol, channel 0 then 1.</item>
/// </list>
/// </summary>
internal readonly unsafe struct PlainBlockAssembler : IBlockAssembler
{
    private const int HalfBlockBytes = 8;
    private const int Pixels = 16;
    private const int Blue0 = 2;
    private const int Red1 = 3;
    private const int Green1 = 4;
    private const int Blue1 = 5;
    private const int AlphaE0 = 6;
    private const int AlphaE1 = 7;
    private const int Channel1E0 = 2;
    private const int Channel1E1 = 3;
    private const int RedShift = 11;
    private const int GreenShift = 5;
    private const int Colour1Offset = 2;
    private const int ColourIndexOffset = 4;
    private const int AlphaE1Shift = 8;
    private const int AlphaIndexShift = 16;
    private const int ThreeBits = 3;
    private const int TwoBits = 2;
    private const int ThreeBitMask = (1 << ThreeBits) - 1;
    private const int TwoBitMask = (1 << TwoBits) - 1;
    private const int PairMask = 0x3F;
    private const int PairSecondShift = 4;
    private const int QuadGroups = 4;
    private const int QuadRowStride = 8;
    private const int QuadColumnStride = 2;
    private const byte AlphaBias = 0x80;
    private const int Bc1QuadStart = 8;
    private const int Bc3PairStart = 6;
    private const int Bc3QuadStart = 20;
    private const int Bc4PairStart = 4;
    private const int Bc5PairStart = 8;
    private const int SmallPairs = 8;
    private const int LargePairs = 16;

    // Record positions of the endpoint residuals, in entry order.
    private static readonly byte[] Bc1Sources = [2, 3, 4, 5, 6, 7];
    private static readonly byte[] Bc3Sources = [14, 16, 18, 15, 17, 19, 4, 5];
    private static readonly byte[] Bc4Sources = [2, 3];
    private static readonly byte[] Bc5Sources = [4, 5, 6, 7];

    // Field masks of the endpoints and indices.
    private static readonly byte[] Bc1EndpointMasks = [31, 63, 31, 31, 63, 31, 0, 0];
    private static readonly byte[] Bc3EndpointMasks = [31, 63, 31, 31, 63, 31, 255, 255];
    private static readonly byte[] Bc4EndpointMasks = [255, 255, 0, 0, 0, 0, 0, 0];
    private static readonly byte[] Bc5EndpointMasks = [255, 255, 255, 255, 0, 0, 0, 0];
    private static readonly byte[] Bc1IndexMasks = IndexMasks(0, Pixels);
    private static readonly byte[] Bc3IndexMasks = IndexMasks(Pixels, Pixels);
    private static readonly byte[] Bc4IndexMasks = IndexMasks(Pixels, 0);
    private static readonly byte[] Bc5IndexMasks = IndexMasks(PlainEntry.IndexCount, 0);

    // Where the pixels of a 2x2 group sit relative to its first pixel, in rank-table order.
    private static readonly byte[] QuadPixels = [0, 1, 4, 5];

    // Index along the endpoint line to BC1 and BC4 codes.
    private static readonly byte[] Bc1Codes = [0, 2, 3, 1];
    private static readonly byte[] Bc4Codes = [0, 2, 3, 4, 5, 6, 7, 1];

    private readonly BcPackCodec _codec;
    private readonly byte* _output;
    private readonly PlainEntry* _history;
    private readonly PlainEntry* _dictionary;
    private readonly PlainEntry* _zero;
    private readonly byte* _pairRank;
    private readonly byte* _quadRank;
    private readonly byte[] _endpointMasks;
    private readonly byte[] _indexMasks;

    /// <param name="codec">BC1, BC3, BC4 or BC5.</param>
    /// <param name="output">Room for every block.</param>
    /// <param name="history">256 entries.</param>
    /// <param name="zero">One zero entry.</param>
    /// <param name="tables">The tables.</param>
    public PlainBlockAssembler(BcPackCodec codec, byte* output, PlainEntry* history, PlainEntry* zero, BcPackTables tables)
    {
        _codec = codec;
        _output = output;
        _history = history;
        _zero = zero;
        _pairRank = null;
        _quadRank = null;

        switch (codec)
        {
            case BcPackCodec.Bc1:
                _dictionary = BcPackTables.Address(tables.Bc1);
                _endpointMasks = Bc1EndpointMasks;
                _indexMasks = Bc1IndexMasks;
                _quadRank = BcPackTables.Address(tables.QuadRank1);
                break;
            case BcPackCodec.Bc3:
                _dictionary = BcPackTables.Address(tables.Bc3);
                _endpointMasks = Bc3EndpointMasks;
                _indexMasks = Bc3IndexMasks;
                _pairRank = BcPackTables.Address(tables.PairRank3);
                _quadRank = BcPackTables.Address(tables.QuadRank3);
                break;
            case BcPackCodec.Bc4:
                _dictionary = BcPackTables.Address(tables.Bc4);
                _endpointMasks = Bc4EndpointMasks;
                _indexMasks = Bc4IndexMasks;
                _pairRank = BcPackTables.Address(tables.PairRank4);
                break;
            default:
                _dictionary = BcPackTables.Address(tables.Bc5);
                _endpointMasks = Bc5EndpointMasks;
                _indexMasks = Bc5IndexMasks;
                _pairRank = BcPackTables.Address(tables.PairRank5);
                break;
        }
    }

    [HotPath]
    public bool Assemble(int block, byte* record)
    {
        var paired = _codec == BcPackCodec.Bc3 || _codec == BcPackCodec.Bc5;
        var (endpointReference, indexReference) = BlockRecord.References(record, paired);
        var endpoints = BlockRecord.Pick(block, endpointReference, _history, _dictionary, _zero);
        var indices = BlockRecord.Pick(block, indexReference, _history, _dictionary, _zero);
        PlainEntry value = default;

        Residuals(record, &value);

        for (var k = 0; k < PlainEntry.EndpointCount; k++)
        {
            value.Endpoints[k] = (byte)((value.Endpoints[k] + endpoints->Endpoints[k]) & _endpointMasks[k]);
        }

        for (var k = 0; k < PlainEntry.IndexCount; k++)
        {
            value.Indices[k] = (byte)((value.Indices[k] + indices->Indices[k]) & _indexMasks[k]);
        }

        _history[block & BlockRecord.HistoryMask] = value;
        Write(block, &value);

        return true;
    }

    private static byte[] IndexMasks(int threeBit, int twoBit)
    {
        var masks = new byte[PlainEntry.IndexCount];
        Array.Fill(masks, (byte)ThreeBitMask, 0, threeBit);
        Array.Fill(masks, (byte)TwoBitMask, threeBit, twoBit);

        return masks;
    }

    [HotPath]
    private void Residuals(byte* record, PlainEntry* value)
    {
        switch (_codec)
        {
            case BcPackCodec.Bc1:
                Endpoints(record, Bc1Sources, value);
                Quads(record + Bc1QuadStart, value->Indices);
                break;
            case BcPackCodec.Bc3:
                Endpoints(record, Bc3Sources, value);
                Pairs(record + Bc3PairStart, SmallPairs, value->Indices);
                Quads(record + Bc3QuadStart, value->Indices + Pixels);
                break;
            case BcPackCodec.Bc4:
                Endpoints(record, Bc4Sources, value);
                Pairs(record + Bc4PairStart, SmallPairs, value->Indices);
                break;
            default:
                Endpoints(record, Bc5Sources, value);
                Pairs(record + Bc5PairStart, LargePairs, value->Indices);
                break;
        }
    }

    [HotPath]
    private void Write(int block, PlainEntry* value)
    {
        var e = value->Endpoints;

        switch (_codec)
        {
            case BcPackCodec.Bc1:
                WriteColour(e, value->Indices, _output + ((long)block * HalfBlockBytes));
                break;
            case BcPackCodec.Bc3:
                var bc3 = _output + ((long)block * BlockBits.BlockBytes);
                WriteAlpha(e[AlphaE0], e[AlphaE1], value->Indices, bc3);
                WriteColour(e, value->Indices + Pixels, bc3 + HalfBlockBytes);
                break;
            case BcPackCodec.Bc4:
                WriteAlpha(e[0], e[1], value->Indices, _output + ((long)block * HalfBlockBytes));
                break;
            default:
                var bc5 = _output + ((long)block * BlockBits.BlockBytes);
                WriteAlpha(e[0], e[1], value->Indices, bc5);
                WriteAlpha(e[Channel1E0], e[Channel1E1], value->Indices + Pixels, bc5 + HalfBlockBytes);
                break;
        }
    }

    [HotPath]
    private static void Endpoints(byte* record, byte[] sources, PlainEntry* value)
    {
        for (var k = 0; k < sources.Length; k++)
        {
            value->Endpoints[k] = (byte)BlockRecord.Signed(record[sources[k]]);
        }
    }

    /// <summary>One symbol per 2x2 group: four 2-bit residuals through the quad rank table.</summary>
    [HotPath]
    private void Quads(byte* symbols, byte* indices)
    {
        for (var group = 0; group < QuadGroups; group++)
        {
            int ranked = _quadRank[symbols[group]];
            var first = ((group >> 1) * QuadRowStride) + ((group & 1) * QuadColumnStride);

            for (var k = 0; k < QuadPixels.Length; k++)
            {
                indices[first + QuadPixels[k]] = (byte)((ranked >> (k * TwoBits)) & TwoBitMask);
            }
        }
    }

    /// <summary>One symbol per two pixels in raster order: two 3-bit residuals through the pair rank table.</summary>
    [HotPath]
    private void Pairs(byte* symbols, int count, byte* indices)
    {
        for (var k = 0; k < count; k++)
        {
            int ranked = _pairRank[symbols[k] & PairMask];
            indices[k << 1] = (byte)(ranked & ThreeBitMask);
            indices[(k << 1) + 1] = (byte)((ranked >> PairSecondShift) & ThreeBitMask);
        }
    }

    /// <summary>A BC1 block (or the colour half of BC3) from R0 G0 B0 R1 G1 B1 and 16 indices.</summary>
    [HotPath]
    private static void WriteColour(byte* e, byte* indices, byte* output)
    {
        var c0 = (e[0] << RedShift) | (e[1] << GreenShift) | e[Blue0];
        var c1 = (e[Red1] << RedShift) | (e[Green1] << GreenShift) | e[Blue1];
        uint bits = 0;

        for (var p = 0; p < Pixels; p++)
        {
            bits |= (uint)Bc1Codes[indices[p] & TwoBitMask] << (p * TwoBits);
        }

        BinaryPrimitives.WriteUInt16LittleEndian(new Span<byte>(output, sizeof(ushort)), (ushort)c0);
        BinaryPrimitives.WriteUInt16LittleEndian(new Span<byte>(output + Colour1Offset, sizeof(ushort)), (ushort)c1);
        BinaryPrimitives.WriteUInt32LittleEndian(new Span<byte>(output + ColourIndexOffset, sizeof(uint)), bits);
    }

    /// <summary>A BC4 block (or the alpha half of BC3, a channel of BC5); the endpoints are kept 128 below their stored value.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static void WriteAlpha(byte e0, byte e1, byte* indices, byte* output)
    {
        var bits = (ulong)(byte)(e0 ^ AlphaBias) | ((ulong)(byte)(e1 ^ AlphaBias) << AlphaE1Shift);

        for (var p = 0; p < Pixels; p++)
        {
            bits |= (ulong)Bc4Codes[indices[p] & ThreeBitMask] << (AlphaIndexShift + (p * ThreeBits));
        }

        BinaryPrimitives.WriteUInt64LittleEndian(new Span<byte>(output, sizeof(ulong)), bits);
    }
}
