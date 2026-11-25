// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The lane meanings follow VIctory (crates/victory-formats/src/bcpack.rs, GPL-3.0); see THIRD_PARTY_NOTICES.md.

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The block layer of BC6H (<see cref="Bc6HEntry"/>). Lane 15 gives the mode (E + 1) and the partition (O). Endpoint 0
/// (lanes 2-4) and endpoint 1 (lanes 5-7) are 16-bit zigzag residuals, E and O interleaved bit by bit; endpoints 2 and
/// 3 of the two-region modes are lanes 8-10 E and O. Each residual is added at the endpoint's precision. Lanes 11-14
/// hold two 4-bit zigzag index residuals per symbol, added to the referenced indices.
/// </summary>
internal readonly unsafe struct Bc6HBlockAssembler : IBlockAssembler
{
    private const int ModeLane = 30;
    private const int PartitionLane = 31;
    private const int PartitionMask = 31;
    private const int Endpoint0Lane = 4;
    private const int Endpoint1Lane = 10;
    private const int Endpoint2Lane = 16;
    private const int IndexLane = 22;
    private const int IndexBytes = 8;
    private const int OddFirstBytes = 6;
    private const int WordBits = 16;
    private const int ByteBits = 8;
    private const int TwoRegionIndexBits = 3;
    private const int OneRegionIndexBits = 4;
    private const int Channels = Bc6HEntry.Channels;
    private const int Endpoint2 = 2;
    private const int Endpoint3 = 3;
    private const int RunFirst = 2;
    private const int RunLength = 3;
    private const ushort SignFlip = 0x8000;
    private const byte ZeroMode = Bc6HModes.FirstOneRegion;

    private readonly byte* _output;
    private readonly Bc6HEntry* _history;
    private readonly Bc6HEntry* _dictionary;
    private readonly Bc6HEntry* _zero;

    /// <param name="output">Room for every block.</param>
    /// <param name="history">256 entries.</param>
    /// <param name="zero">One entry, which this sets to the zero reference (mode 11).</param>
    /// <param name="tables">The tables.</param>
    public Bc6HBlockAssembler(byte* output, Bc6HEntry* history, Bc6HEntry* zero, BcPackTables tables)
    {
        _output = output;
        _history = history;
        _zero = zero;
        _dictionary = BcPackTables.Address(tables.Bc6H);
        *zero = default;
        zero->Mode = ZeroMode;
    }

    [HotPath]
    public bool Assemble(int block, byte* record)
    {
        var mode = record[ModeLane] + 1;

        if (mode > Bc6HModes.LastMode)
        {
            return false;
        }

        var (endpointReference, indexReference) = BlockRecord.References(record, paired: true);
        var endpoints = BlockRecord.Pick(block, endpointReference, _history, _dictionary, _zero);
        var indices = BlockRecord.Pick(block, indexReference, _history, _dictionary, _zero);
        Bc6HEntry value = default;
        value.Mode = (byte)mode;

        Endpoints(record, mode, endpoints, &value);
        Indices(record, mode, indices, &value);
        _history[block & BlockRecord.HistoryMask] = value;
        Write(&value, record[PartitionLane] & PartitionMask, _output + ((long)block * BlockBits.BlockBytes));

        return true;
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int IndexBits(int mode)
    {
        if (mode == 0)
        {
            return 0;
        }

        if (mode < Bc6HModes.FirstOneRegion)
        {
            return TwoRegionIndexBits;
        }

        return OneRegionIndexBits;
    }

    /// <summary>The shift that left-aligns endpoint <paramref name="endpoint"/> channel <paramref name="channel"/>.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int Shift(int mode, int endpoint, int channel)
    {
        var precision = Bc6HModes.Precision[mode];

        if (endpoint == 0)
        {
            return WordBits - precision[0];
        }

        return WordBits - precision[1 + channel];
    }

    /// <summary>16-bit residual of a lane: E on the even bits, O on the odd.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static ushort Wide(byte* record, int lane)
    {
        var interleaved = Spread(record[lane]) | (Spread(record[lane + 1]) << 1);

        return (ushort)BlockRecord.Signed(interleaved);
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int Spread(int value)
    {
        var spread = 0;

        for (var k = 0; k < ByteBits; k++)
        {
            spread |= ((value >> k) & 1) << (k << 1);
        }

        return spread;
    }

    [HotPath]
    private static void Endpoints(byte* record, int mode, Bc6HEntry* reference, Bc6HEntry* value)
    {
        var twoRegions = mode < Bc6HModes.FirstOneRegion;

        for (var c = 0; c < Channels; c++)
        {
            Add(value, reference, mode, 0, c, Wide(record, Endpoint0Lane + (c << 1)));
            Add(value, reference, mode, 1, c, Wide(record, Endpoint1Lane + (c << 1)));

            if (twoRegions)
            {
                Add(value, reference, mode, Endpoint2, c, (ushort)BlockRecord.Signed(record[Endpoint2Lane + (c << 1)]));
                Add(value, reference, mode, Endpoint3, c, (ushort)BlockRecord.Signed(record[Endpoint2Lane + (c << 1) + 1]));
            }
        }

        if (twoRegions)
        {
            return;
        }

        // One region: endpoints 2 and 3 repeat 0 and 1 for the predictions of later blocks.
        var shift0 = Shift(mode, 0, 0);
        var shift1 = Shift(mode, 1, 0);

        for (var c = 0; c < Channels; c++)
        {
            value->Endpoints[(Endpoint2 * Channels) + c] = (ushort)((value->Endpoints[c] >> shift0) << shift1);
            value->Endpoints[(Endpoint3 * Channels) + c] = value->Endpoints[Channels + c];
        }
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static void Add(Bc6HEntry* value, Bc6HEntry* reference, int mode, int endpoint, int channel, ushort residual)
    {
        var shift = Shift(mode, endpoint, channel);
        var at = (endpoint * Channels) + channel;
        var sum = (ushort)(reference->Endpoints[at] + (residual << shift));
        value->Endpoints[at] = (ushort)(sum & (ushort.MaxValue << shift));
    }

    [HotPath]
    private static void Indices(byte* record, int mode, Bc6HEntry* reference, Bc6HEntry* value)
    {
        var bits = IndexBits(mode);
        var referenceBits = IndexBits(reference->Mode);

        for (var k = 0; k < IndexBytes; k++)
        {
            var (first, second) = BlockRecord.Nibbles(record[IndexLane + k], k < OddFirstBytes);
            var p = k << 1;
            value->Indices[p] = BlockRecord.AddIndex(first, bits, reference->Indices[p], referenceBits);
            value->Indices[p + 1] = BlockRecord.AddIndex(second, bits, reference->Indices[p + 1], referenceBits);
        }
    }

    [HotPath]
    private static void Write(Bc6HEntry* value, int partition, byte* output)
    {
        int mode = value->Mode;
        var bits = new BlockBits();
        var runs = Bc6HModes.Runs[mode];
        bits.Put(Bc6HModes.Code[mode], Bc6HModes.CodeBits[mode]);

        for (var at = 0; at < runs.Length; at += Bc6HModes.RunBytes)
        {
            int channel = runs[at];

            if (channel == Bc6HModes.Partition)
            {
                bits.Put((ulong)partition, Bc6HModes.PartitionBits);

                continue;
            }

            int endpoint = runs[at + 1];
            var field = (value->Endpoints[(endpoint * Channels) + channel] ^ SignFlip) >> Shift(mode, endpoint, channel);
            bits.Put((ulong)(field >> runs[at + RunFirst]), runs[at + RunLength]);
        }

        var indexBits = IndexBits(mode);
        var anchor = 0;

        if (mode < Bc6HModes.FirstOneRegion)
        {
            anchor = Bc7Partitions.Anchor2[partition];
        }

        for (var p = 0; p < Bc6HEntry.IndexCount; p++)
        {
            var width = indexBits;

            if (p == 0 || p == anchor)
            {
                width--;
            }

            bits.Put((ushort)(value->Indices[p] + (1 << (indexBits - 1))), width);
        }

        bits.WriteTo(output);
    }
}
