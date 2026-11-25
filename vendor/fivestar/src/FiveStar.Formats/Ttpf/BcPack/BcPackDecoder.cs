// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The entropy layer follows bcpack-rs (src/rans.rs; MIT), whose lanes port Microsoft silicon-rans mdu_minirans.v (MIT).
// See THIRD_PARTY_NOTICES.md.

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The BCPack entropy layer of every codec: the codec's 12 or 16 rANS lanes read one shared stream, and each lane emits
/// one symbol per 8 bytes of output. Stateless and thread-safe. The fastest engine the processor supports runs
/// (AVX-512, AVX2, then scalar); all give the same symbols.
/// </summary>
public static unsafe class BcPackDecoder
{
    /// <summary>The most symbols per lane one decode takes: the output must fit one span.</summary>
    public const int MaxSymbols = int.MaxValue / LaneLayout.BytesPerSymbol;

    /// <summary>Gets the engine the decoders use by default: the fastest this processor runs.</summary>
    public static BcPackEngine DefaultEngine { get; } = FastestEngine();

    /// <summary>Whether this processor can run <paramref name="engine"/>.</summary>
    public static bool IsSupported(BcPackEngine engine)
    {
        switch (engine)
        {
            case BcPackEngine.Avx512:
                return Avx512LaneEngine.IsSupported;
            case BcPackEngine.Avx2:
                return Avx2LaneEngine.IsSupported;
            default:
                return true;
        }
    }

    /// <summary>Gets the lanes of <paramref name="codec"/>.</summary>
    public static int Lanes(BcPackCodec codec) => LaneLayout.For(codec).Lanes;

    /// <summary>The symbols each lane emits for a page of <paramref name="virtualSize"/> bytes, or 0 when it is not whole blocks.</summary>
    public static int SymbolsPerLane(BcPackCodec codec, long virtualSize)
    {
        var layout = LaneLayout.For(codec);

        if (virtualSize <= 0 || virtualSize % layout.BlockBytes != 0 || virtualSize / LaneLayout.BytesPerSymbol > MaxSymbols)
        {
            return 0;
        }

        return (int)(virtualSize / LaneLayout.BytesPerSymbol);
    }

    /// <summary>
    /// The lane symbols of a page, lane major: <c>symbols[lane * count + n]</c> is lane <c>lane</c>'s n-th symbol.
    /// </summary>
    /// <param name="chunk">The stored bytes; trailing zero padding is allowed.</param>
    /// <param name="codec">The page's codec.</param>
    /// <param name="count">Symbols per lane (<see cref="SymbolsPerLane"/>).</param>
    /// <param name="symbols">At least lanes * <paramref name="count"/> entries; valid only when the result is ok.</param>
    /// <param name="engine">The engine; one the processor cannot run falls back to the scalar engine.</param>
    public static BcPackDecodeResult DecodeSymbols(ReadOnlySpan<byte> chunk, BcPackCodec codec, int count, Span<ushort> symbols, BcPackEngine engine)
    {
        var layout = LaneLayout.For(codec);

        if (count <= 0 || count > MaxSymbols || symbols.Length < (long)layout.Lanes * count)
        {
            return new BcPackDecodeResult(BcPackDecodeStatus.InvalidArgument, 0, chunk.Length, 0, 0, 0);
        }

        fixed (ushort* lanes = symbols)
        {
            var sink = new LaneSymbolSink(lanes, count, layout.Lanes);

            return DecodeWith(chunk, layout, count, ref sink, engine);
        }
    }

    /// <summary>Runs the lanes of <paramref name="layout"/> into <paramref name="sink"/>; a converged decode fails when anything but zero padding follows.</summary>
    [HotPath]
    internal static BcPackDecodeResult DecodeWith<TSink>(ReadOnlySpan<byte> chunk, LaneLayout layout, int count, ref TSink sink, BcPackEngine engine)
        where TSink : struct, ISymbolVectorSink
    {
        var result = RunEngine(chunk, layout.Widths, count, ref sink, engine);

        if (result.Success && chunk[result.Consumed..].ContainsAnyExcept((byte)0))
        {
            return result with { Status = BcPackDecodeStatus.TrailingData };
        }

        return result;
    }

    [HotPath]
    private static BcPackDecodeResult RunEngine<TSink>(ReadOnlySpan<byte> chunk, ReadOnlySpan<byte> widths, int count, ref TSink sink, BcPackEngine engine)
        where TSink : struct, ISymbolVectorSink
    {
        if (engine == BcPackEngine.Avx512 && Avx512LaneEngine.IsSupported)
        {
            return Avx512LaneEngine.DecodeLanes(chunk, count, widths, ref sink);
        }

        if (engine == BcPackEngine.Avx2 && Avx2LaneEngine.IsSupported)
        {
            return Avx2LaneEngine.DecodeLanes(chunk, count, widths, ref sink);
        }

        return ScalarLaneEngine.DecodeLanes(chunk, count, widths, ref sink);
    }

    private static BcPackEngine FastestEngine()
    {
        if (Avx512LaneEngine.IsSupported)
        {
            return BcPackEngine.Avx512;
        }

        if (Avx2LaneEngine.IsSupported)
        {
            return BcPackEngine.Avx2;
        }

        return BcPackEngine.Scalar;
    }
}
