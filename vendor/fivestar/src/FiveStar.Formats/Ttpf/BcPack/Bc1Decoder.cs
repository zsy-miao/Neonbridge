// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// A clean implementation after bcpack-rs (src/bc1.rs, src/rans.rs; MIT), whose entropy layer ports Microsoft
// silicon-rans mdu_minirans.v (MIT). See THIRD_PARTY_NOTICES.md.

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Decodes BCPack BC1 pages (texture page codec 1): 12 rANS lanes read one shared stream and emit one symbol each per
/// block (<see cref="BcPackDecoder"/>). The output is the page's BC1 blocks, 8 bytes each, in stream order, which is
/// the page's stored tile order. Stateless and thread-safe; a decode allocates nothing on the heap.
/// </summary>
public static unsafe class Bc1Decoder
{
    /// <summary>rANS lanes of a BC1 page.</summary>
    public const int Lanes = 12;

    /// <summary>Bytes per BC1 block.</summary>
    public const int BlockSize = 8;

    /// <summary>The most blocks one decode takes: the output must fit one span.</summary>
    public const int MaxBlocks = int.MaxValue / BlockSize;

    /// <summary><c>[field * 128 + symbol]</c>: the zigzag residual of a symbol, modulo its field, at the field's position.</summary>
    private static readonly uint[] ResidualTable = BuildResidualTable();

    /// <summary>Decodes one page's stored bytes with the embedded tables.</summary>
    /// <param name="chunk">The stored bytes; trailing zero padding is allowed.</param>
    /// <param name="blocks">BC1 blocks in the page (its virtual size / 8).</param>
    /// <param name="output">At least <paramref name="blocks"/> * 8 bytes; valid only when the result is ok.</param>
    public static BcPackDecodeResult Decode(ReadOnlySpan<byte> chunk, int blocks, Span<byte> output) =>
        Decode(chunk, blocks, output, Bc1Codebook.Default, BcPackDecoder.DefaultEngine);

    /// <summary>Decodes with a chosen engine; one the processor cannot run falls back to the scalar engine.</summary>
    public static BcPackDecodeResult Decode(ReadOnlySpan<byte> chunk, int blocks, Span<byte> output, BcPackEngine engine) =>
        Decode(chunk, blocks, output, Bc1Codebook.Default, engine);

    [HotPath]
    [SkipLocalsInit]
    internal static BcPackDecodeResult Decode(ReadOnlySpan<byte> chunk, int blocks, Span<byte> output, Bc1Codebook codebook, BcPackEngine engine)
    {
        ArgumentNullException.ThrowIfNull(codebook);

        if (blocks <= 0 || blocks > MaxBlocks || output.Length < blocks * BlockSize)
        {
            return new BcPackDecodeResult(BcPackDecodeStatus.InvalidArgument, 0, chunk.Length, 0, 0, 0);
        }

        var ring = stackalloc uint[Bc1BlockSink.RingSize];
        var unfitted = stackalloc byte[Bc1BlockSink.RingSize];
        codebook.Window.CopyTo(new Span<uint>(ring, Bc1BlockSink.RingSize));
        codebook.Unfitted.CopyTo(new Span<byte>(unfitted, Bc1BlockSink.RingSize));

        fixed (byte* blockOutput = output)
        fixed (uint* quadrants = codebook.QuadMasks)
        fixed (uint* residuals = ResidualTable)
        {
            var sink = new Bc1BlockSink(blockOutput, ring, unfitted, quadrants, residuals);
            var result = BcPackDecoder.DecodeWith(chunk, LaneLayout.Bc1, blocks, ref sink, engine);

            return result with { UnfittedBlocks = sink.UnfittedBlocks };
        }
    }

    private static uint[] BuildResidualTable()
    {
        var table = new uint[Bc1Fields.Count * Bc1BlockSink.ResidualCodes];

        for (var field = 0; field < Bc1Fields.Count; field++)
        {
            for (var code = 0; code < Bc1BlockSink.ResidualCodes; code++)
            {
                // Zigzag 0, 1, 2, 3, 4 is 0, -1, +1, -2, +2; modulo 2^width it depends only on the code modulo 2^(width + 1).
                var delta = code >> 1;

                if ((code & 1) != 0)
                {
                    delta = -((code + 1) >> 1);
                }

                var wrapped = (uint)(delta & ((1 << Bc1Fields.Width[field]) - 1));
                table[(field * Bc1BlockSink.ResidualCodes) + code] = wrapped << Bc1Fields.Shift[field];
            }
        }

        return table;
    }
}
