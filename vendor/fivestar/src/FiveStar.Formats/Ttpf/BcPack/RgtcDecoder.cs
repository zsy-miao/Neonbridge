// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Decodes BCPack BC4 and BC5 pages (codecs 4 and 5) to BC4 or BC5 blocks in stream order. The pair tables and preset
/// slots are fitted, so the blocks are approximate: they agree with the neighbouring mips, but no copy of the same
/// blocks confirms them byte for byte. Stateless and thread-safe; a decode allocates nothing on the heap.
/// </summary>
internal static unsafe class RgtcDecoder
{
    [HotPath]
    [SkipLocalsInit]
    public static BcPackDecodeResult Decode(BcPackCodec codec, ReadOnlySpan<byte> chunk, int count, Span<byte> output, RgtcTables tables, BcPackEngine engine)
    {
        var layout = LaneLayout.For(codec);

        if (count <= 0 || count > BcPackDecoder.MaxSymbols || output.Length < count * LaneLayout.BytesPerSymbol || count % layout.SymbolsPerBlock != 0)
        {
            return new BcPackDecodeResult(BcPackDecodeStatus.InvalidArgument, 0, chunk.Length, 0, 0, 0);
        }

        var ring = stackalloc uint[RgtcSymbols.RingSize];
        var unfitted = stackalloc byte[RgtcSymbols.RingSize];
        tables.Presets.CopyTo(new Span<uint>(ring, RgtcSymbols.RingSize));
        tables.Unfitted.CopyTo(new Span<byte>(unfitted, RgtcSymbols.RingSize));

        fixed (byte* blocks = output)
        fixed (uint* pairs = tables.PairCodes)
        {
            if (codec == BcPackCodec.Bc4)
            {
                var bc4 = new Bc4BlockSink(blocks, ring, unfitted, pairs);
                var result = BcPackDecoder.DecodeWith(chunk, layout, count, ref bc4, engine);

                return result with { UnfittedBlocks = bc4.UnfittedBlocks };
            }

            var bc5 = new Bc5BlockSink(blocks, ring, unfitted, pairs);
            var decoded = BcPackDecoder.DecodeWith(chunk, layout, count, ref bc5, engine);

            return decoded with { UnfittedBlocks = bc5.UnfittedBlocks };
        }
    }
}
