// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The two fitted BC1 tables: the 256-slot preset endpoint window that back references before block 0 read, and the
/// codebook of 256 2x2 index groups. <see cref="Default"/> holds the tables of <c>Tables/bc1_window.json</c> and
/// <c>Tables/bc1_quad_table.json</c>: the bcpack-rs fits with our corrections. Immutable and thread-safe.
/// </summary>
public sealed class Bc1Codebook
{
    public const int Slots = 256;

    public const int CodesPerSymbol = 4;

    public const int Quadrants = 4;

    private const int CodeMask = 3;
    private const int BitsPerCode = 2;
    private const int PixelsPerRow = 4;

    // Top-left pixel of the quadrants TL, TR, BL, BR, and the pixels of a group in table order.
    private static readonly int[] QuadrantX = [0, 2, 0, 2];
    private static readonly int[] QuadrantY = [0, 0, 2, 2];
    private static readonly int[] GroupX = [0, 1, 0, 1];
    private static readonly int[] GroupY = [0, 0, 1, 1];

    private Bc1Codebook(ReadOnlySpan<byte> windowFields, ReadOnlySpan<byte> fitted, ReadOnlySpan<byte> quadCodes)
    {
        Window = new uint[Slots];
        Unfitted = new byte[Slots];
        QuadMasks = new uint[Quadrants * Slots];

        for (var slot = 0; slot < Slots; slot++)
        {
            Window[slot] = PackSlot(windowFields.Slice(slot * Bc1Fields.Count, Bc1Fields.Count), slot);

            if (fitted[slot] == 0)
            {
                Unfitted[slot] = 1;
            }
        }

        for (var quadrant = 0; quadrant < Quadrants; quadrant++)
        {
            for (var symbol = 0; symbol < Slots; symbol++)
            {
                QuadMasks[(quadrant * Slots) + symbol] = QuadMask(quadCodes.Slice(symbol * CodesPerSymbol, CodesPerSymbol), quadrant);
            }
        }
    }

    /// <summary>Gets the embedded tables (255 of 256 window slots fitted; slot 0 is never read).</summary>
    public static Bc1Codebook Default { get; } = BcPackTableReader.ReadCodebook();

    /// <summary>Gets the packed endpoints <c>c0 | c1 &lt;&lt; 16</c> of each window slot.</summary>
    internal uint[] Window { get; }

    /// <summary>Gets 1 for each window slot that is not fitted (it reads as zeros, and blocks reaching it are counted).</summary>
    internal byte[] Unfitted { get; }

    /// <summary>Gets <c>[quadrant * 256 + symbol]</c>: the symbol's bits of the block's 32-bit index word.</summary>
    internal uint[] QuadMasks { get; }

    /// <summary>A codebook from explicit tables in which every window slot counts as fitted.</summary>
    /// <param name="windowFields">256 x [R0, G0, B0, R1, G1, B1].</param>
    /// <param name="quadCodes">256 x the BC1 codes (0-3) of a 2x2 group in the order TL, TR, BL, BR.</param>
    public static Bc1Codebook Create(ReadOnlySpan<byte> windowFields, ReadOnlySpan<byte> quadCodes)
    {
        var fitted = new byte[Slots];
        Array.Fill(fitted, (byte)1);

        return FromTables(windowFields, fitted, quadCodes);
    }

    internal static Bc1Codebook FromTables(ReadOnlySpan<byte> windowFields, ReadOnlySpan<byte> fitted, ReadOnlySpan<byte> quadCodes)
    {
        if (windowFields.Length != Slots * Bc1Fields.Count || fitted.Length != Slots || quadCodes.Length != Slots * CodesPerSymbol)
        {
            throw new ArgumentException("a codebook needs 256 x 6 window fields and 256 x 4 codes");
        }

        return new Bc1Codebook(windowFields, fitted, quadCodes);
    }

    private static uint PackSlot(ReadOnlySpan<byte> fields, int slot)
    {
        uint packed = 0;

        for (var field = 0; field < Bc1Fields.Count; field++)
        {
            uint value = fields[field];

            if (value >> Bc1Fields.Width[field] != 0)
            {
                throw new ArgumentException($"window slot {slot} field {field} = {value} is too wide");
            }

            packed |= value << Bc1Fields.Shift[field];
        }

        return packed;
    }

    private static uint QuadMask(ReadOnlySpan<byte> codes, int quadrant)
    {
        uint mask = 0;

        for (var pixel = 0; pixel < CodesPerSymbol; pixel++)
        {
            var x = QuadrantX[quadrant] + GroupX[pixel];
            var y = QuadrantY[quadrant] + GroupY[pixel];
            mask |= (uint)(codes[pixel] & CodeMask) << (BitsPerCode * ((PixelsPerRow * y) + x));
        }

        return mask;
    }
}
