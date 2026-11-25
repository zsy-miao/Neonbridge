// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Decodes BCPack pages of every codec exactly with the user's tables (<see cref="BcPackTables"/>): the lanes
/// (<see cref="BcPackDecoder"/>) feed each block's record to the codec's block layer. Stateless and thread-safe; the
/// history of the last 256 blocks lives on the stack.
/// </summary>
internal static unsafe class TableBlockDecoder
{
    public static BcPackDecodeResult Decode(BcPackCodec codec, ReadOnlySpan<byte> chunk, int count, Span<byte> output, BcPackTables tables, BcPackEngine engine)
    {
        var layout = LaneLayout.For(codec);

        if (count <= 0 || count > BcPackDecoder.MaxSymbols || output.Length < (long)count * LaneLayout.BytesPerSymbol || count % layout.SymbolsPerBlock != 0)
        {
            return new BcPackDecodeResult(BcPackDecodeStatus.InvalidArgument, 0, chunk.Length, 0, 0, 0);
        }

        fixed (byte* blocks = output)
        {
            switch (codec)
            {
                case BcPackCodec.Bc6H:
                    return DecodeBc6H(chunk, layout, count, blocks, tables, engine);
                case BcPackCodec.Bc7:
                    return DecodeBc7(chunk, layout, count, blocks, tables, engine);
                default:
                    return DecodePlain(codec, chunk, layout, count, blocks, tables, engine);
            }
        }
    }

    private static BcPackDecodeResult DecodePlain(BcPackCodec codec, ReadOnlySpan<byte> chunk, LaneLayout layout, int count, byte* blocks, BcPackTables tables, BcPackEngine engine)
    {
        var history = stackalloc PlainEntry[BlockRecord.HistorySize];
        PlainEntry zero = default;
        var sink = new RecordSink<PlainBlockAssembler>(layout, new PlainBlockAssembler(codec, blocks, history, &zero, tables));

        return Finish(BcPackDecoder.DecodeWith(chunk, layout, count, ref sink, engine), sink.InvalidBlocks);
    }

    private static BcPackDecodeResult DecodeBc6H(ReadOnlySpan<byte> chunk, LaneLayout layout, int count, byte* blocks, BcPackTables tables, BcPackEngine engine)
    {
        var history = stackalloc Bc6HEntry[BlockRecord.HistorySize];
        Bc6HEntry zero = default;
        var sink = new RecordSink<Bc6HBlockAssembler>(layout, new Bc6HBlockAssembler(blocks, history, &zero, tables));

        return Finish(BcPackDecoder.DecodeWith(chunk, layout, count, ref sink, engine), sink.InvalidBlocks);
    }

    private static BcPackDecodeResult DecodeBc7(ReadOnlySpan<byte> chunk, LaneLayout layout, int count, byte* blocks, BcPackTables tables, BcPackEngine engine)
    {
        var history = stackalloc Bc7Entry[BlockRecord.HistorySize];
        Bc7Entry zero = default;
        var sink = new RecordSink<Bc7BlockAssembler>(layout, new Bc7BlockAssembler(blocks, history, &zero, tables));

        return Finish(BcPackDecoder.DecodeWith(chunk, layout, count, ref sink, engine), sink.InvalidBlocks);
    }

    private static BcPackDecodeResult Finish(BcPackDecodeResult result, int invalid)
    {
        if (result.Success && invalid > 0)
        {
            return result with { Status = BcPackDecodeStatus.InvalidBlock, Detail = invalid };
        }

        return result;
    }
}
