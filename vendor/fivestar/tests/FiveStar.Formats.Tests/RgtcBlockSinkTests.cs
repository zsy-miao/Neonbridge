// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Ttpf.BcPack;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>The BC5 block layer on hand-made symbol vectors: endpoint prediction and residuals, and where each pair lands.</summary>
public sealed unsafe class RgtcBlockSinkTests
{
    private const int Lanes = 12;
    private const int BlockBytes = 16;

    [Fact]
    public void Symbol_vectors_become_bc5_blocks()
    {
        // An identity pair table: symbol s codes indices (s & 7, s >> 3).
        var pairs = Enumerable.Range(0, 64).Select(symbol => (uint)symbol).ToArray();
        var ring = new uint[256];
        var unfitted = new byte[256];
        var output = new byte[2 * BlockBytes];

        // Block 0, distance 0: endpoints 128 plus residuals +1, -2 (channel 0) and -1, +2 (channel 1).
        uint[] even0 = [0, 0, 2, 1, 1, 2, 3, 4, 5, 6, 7, 8];
        uint[] odd0 = [0, 0, 3, 4, 9, 10, 11, 12, 13, 14, 15, 16];

        // Block 1, distance 1 (E 0, O 1 interleave to zigzag code 2): block 0's endpoints, residual 0.
        uint[] even1 = [0, 0, 0, 0, 63, 63, 63, 63, 0, 0, 0, 0];
        uint[] odd1 = [1, 0, 0, 0, 63, 63, 63, 63, 0, 0, 0, 0];

        fixed (byte* blocks = output)
        fixed (uint* ringSlots = ring)
        fixed (byte* flags = unfitted)
        fixed (uint* table = pairs)
        {
            var sink = new Bc5BlockSink(blocks, ringSlots, flags, table);
            Emit(ref sink, 0, even0);
            Emit(ref sink, 1, odd0);
            Emit(ref sink, 2, even1);
            Emit(ref sink, 3, odd1);
        }

        Assert.Equal([129, 126], output[0..2]);
        Assert.Equal([127, 130], output[8..10]);
        Assert.Equal([129, 126], output[16..18]);
        Assert.Equal([127, 130], output[24..26]);

        // Row r of channel 0: E (lane 4 + r) codes pixels 4r and 4r + 1, O pixels 4r + 2 and 4r + 3.
        for (var row = 0; row < 4; row++)
        {
            Assert.Equal(even0[4 + row] & 7, Index(output, 0, 4 * row));
            Assert.Equal(even0[4 + row] >> 3, Index(output, 0, (4 * row) + 1));
            Assert.Equal(odd0[4 + row] & 7, Index(output, 0, (4 * row) + 2));
            Assert.Equal(odd0[8 + row] >> 3, Index(output, 8, (4 * row) + 3));
            Assert.Equal(7u, Index(output, BlockBytes, (4 * row) + 3));
        }
    }

    private static void Emit(ref Bc5BlockSink sink, int vector, uint[] symbols)
    {
        Assert.Equal(Lanes, symbols.Length);

        fixed (uint* lanes = symbols)
        {
            sink.Emit(vector, lanes);
        }
    }

    /// <summary>The 3-bit index of a pixel in the BC4 block at <paramref name="offset"/>.</summary>
    private static uint Index(byte[] blocks, int offset, int pixel)
    {
        var block = BinaryPrimitives.ReadUInt64LittleEndian(blocks.AsSpan(offset));

        return (uint)(block >> (16 + (3 * pixel))) & 7;
    }
}
