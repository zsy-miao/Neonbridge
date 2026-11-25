// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The lane counts and symbol widths of codecs 3 to 7 are those stated in the GTA6-Asset-Info notes (formats/06-bcpack.md,
// GPL-3.0); every page of the reference dump converges with them. See THIRD_PARTY_NOTICES.md.

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The lanes of a BCPack codec: how many there are and the symbol width of each, in bits. A lane's alphabet index (the
/// hardware's <c>alphabet_bits</c>) is its width - 1. Every codec emits one symbol per lane for each 8 bytes of output.
/// </summary>
/// <param name="Codec">The codec.</param>
/// <param name="Widths">The symbol width of each lane, in bits.</param>
/// <param name="BlockBytes">Bytes per BC block.</param>
internal sealed record LaneLayout(BcPackCodec Codec, byte[] Widths, int BlockBytes)
{
    /// <summary>Output bytes per lane symbol.</summary>
    public const int BytesPerSymbol = 8;

    private const int SmallBlock = 8;
    private const int LargeBlock = 16;

    // Symbol widths per lane, in bits.
    private static readonly byte[] Bc1Widths = [8, 8, 5, 6, 5, 5, 6, 5, 8, 8, 8, 8];
    private static readonly byte[] Bc3Widths = [4, 4, 8, 6, 6, 6, 6, 5, 6, 5, 8, 8];
    private static readonly byte[] Bc4Widths = [8, 8, 8, 8, 6, 6, 6, 6, 6, 6, 6, 6];
    private static readonly byte[] Bc5Widths = [4, 4, 8, 8, 6, 6, 6, 6, 6, 6, 6, 6];
    private static readonly byte[] Bc6HWidths = [4, 4, 8, 8, 8, 5, 5, 5, 6, 6, 6, 8, 8, 8, 8, 5];
    private static readonly byte[] Bc7Widths = [4, 4, 8, 8, 8, 8, 8, 7, 7, 5, 6, 8, 8, 8, 8, 5];

    public static LaneLayout Bc1 { get; } = new(BcPackCodec.Bc1, Bc1Widths, SmallBlock);

    public static LaneLayout Bc3 { get; } = new(BcPackCodec.Bc3, Bc3Widths, LargeBlock);

    public static LaneLayout Bc4 { get; } = new(BcPackCodec.Bc4, Bc4Widths, SmallBlock);

    public static LaneLayout Bc5 { get; } = new(BcPackCodec.Bc5, Bc5Widths, LargeBlock);

    public static LaneLayout Bc6H { get; } = new(BcPackCodec.Bc6H, Bc6HWidths, LargeBlock);

    public static LaneLayout Bc7 { get; } = new(BcPackCodec.Bc7, Bc7Widths, LargeBlock);

    /// <summary>Gets the number of lanes.</summary>
    public int Lanes => Widths.Length;

    /// <summary>Gets the lane symbols per block: 1 for 8-byte blocks, 2 for 16-byte blocks.</summary>
    public int SymbolsPerBlock => BlockBytes / BytesPerSymbol;

    /// <summary>The layout of a codec.</summary>
    public static LaneLayout For(BcPackCodec codec)
    {
        switch (codec)
        {
            case BcPackCodec.Bc1:
                return Bc1;
            case BcPackCodec.Bc3:
                return Bc3;
            case BcPackCodec.Bc4:
                return Bc4;
            case BcPackCodec.Bc5:
                return Bc5;
            case BcPackCodec.Bc6H:
                return Bc6H;
            case BcPackCodec.Bc7:
                return Bc7;
            default:
                throw new ArgumentOutOfRangeException(nameof(codec), codec, "not a BCPack codec");
        }
    }
}
