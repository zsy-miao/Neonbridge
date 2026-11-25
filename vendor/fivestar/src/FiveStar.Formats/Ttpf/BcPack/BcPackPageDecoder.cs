// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// Decodes a BCPack page of any codec that has a block decoder to its BC blocks, which replace the page's virtual
/// bytes. With the user's tables installed (<see cref="Install"/>) every codec decodes exactly. Without them BC1 uses
/// the tables of bcpack-rs with our corrections, BC4 and BC5 tables fitted here (approximate,
/// <see cref="IsApproximate"/>), and BC3, BC6H and BC7 do not decode.
/// </summary>
public static class BcPackPageDecoder
{
    private static BcPackTables? _tables;

    /// <summary>Gets the installed tables, or null.</summary>
    public static BcPackTables? Tables => Volatile.Read(ref _tables);

    /// <summary>Installs the tables for every later decode in this process (null removes them); call once at start-up.</summary>
    public static void Install(BcPackTables? tables) => Volatile.Write(ref _tables, tables);

    /// <summary>Whether pages of <paramref name="codec"/> decode to blocks.</summary>
    public static bool CanDecode(int codec)
    {
        switch ((BcPackCodec)codec)
        {
            case BcPackCodec.Bc1:
                return true;
            case BcPackCodec.Bc3:
            case BcPackCodec.Bc6H:
            case BcPackCodec.Bc7:
                return Tables is not null;
            case BcPackCodec.Bc4:
                return Tables is not null || RgtcTables.Bc4 is not null;
            case BcPackCodec.Bc5:
                return Tables is not null || RgtcTables.Bc5 is not null;
            default:
                return false;
        }
    }

    /// <summary>
    /// Whether the blocks of <paramref name="codec"/> rest on tables fitted only against neighbouring mips (BC4 and BC5
    /// without the user's tables): they look right but are not known to be the stored blocks.
    /// </summary>
    public static bool IsApproximate(int codec) =>
        Tables is null && CanDecode(codec) && codec != (int)BcPackCodec.Bc1;

    /// <summary>Decodes one page's stored bytes.</summary>
    /// <param name="codec">The page's codec (<see cref="CanDecode"/>).</param>
    /// <param name="chunk">The stored bytes; trailing zero padding is allowed.</param>
    /// <param name="virtualSize">The page's virtual size: the bytes of blocks it decodes to.</param>
    /// <param name="output">At least <paramref name="virtualSize"/> bytes; valid only when the result is ok.</param>
    /// <param name="engine">The lane engine.</param>
    public static BcPackDecodeResult Decode(int codec, ReadOnlySpan<byte> chunk, long virtualSize, Span<byte> output, BcPackEngine engine)
    {
        if (!CanDecode(codec))
        {
            return new BcPackDecodeResult(BcPackDecodeStatus.InvalidArgument, 0, chunk.Length, 0, 0, 0);
        }

        var count = BcPackDecoder.SymbolsPerLane((BcPackCodec)codec, virtualSize);
        var tables = Tables;

        if (tables is not null)
        {
            return TableBlockDecoder.Decode((BcPackCodec)codec, chunk, count, output, tables, engine);
        }

        switch ((BcPackCodec)codec)
        {
            case BcPackCodec.Bc1:
                return Bc1Decoder.Decode(chunk, count, output, engine);
            case BcPackCodec.Bc4:
                return RgtcDecoder.Decode(BcPackCodec.Bc4, chunk, count, output, RgtcTables.Bc4!, engine);
            default:
                return RgtcDecoder.Decode(BcPackCodec.Bc5, chunk, count, output, RgtcTables.Bc5!, engine);
        }
    }

    /// <summary>Decodes with the fastest engine this processor runs.</summary>
    public static BcPackDecodeResult Decode(int codec, ReadOnlySpan<byte> chunk, long virtualSize, Span<byte> output) =>
        Decode(codec, chunk, virtualSize, output, BcPackDecoder.DefaultEngine);

    /// <summary>Decodes with explicit tables, whatever is installed.</summary>
    public static BcPackDecodeResult Decode(int codec, ReadOnlySpan<byte> chunk, long virtualSize, Span<byte> output, BcPackTables tables, BcPackEngine engine)
    {
        ArgumentNullException.ThrowIfNull(tables);

        if (!Enum.IsDefined((BcPackCodec)codec))
        {
            return new BcPackDecodeResult(BcPackDecodeStatus.InvalidArgument, 0, chunk.Length, 0, 0, 0);
        }

        var count = BcPackDecoder.SymbolsPerLane((BcPackCodec)codec, virtualSize);

        return TableBlockDecoder.Decode((BcPackCodec)codec, chunk, count, output, tables, engine);
    }
}
