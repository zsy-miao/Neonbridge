// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The fitted tables of a BCPack BC4 or BC5 page: the 64-entry table of 3-bit index pairs, and the 256-slot preset
/// endpoint history that back references before block 0 read. <see cref="Bc4"/> and <see cref="Bc5"/> hold the tables
/// of <c>Tables/bc4_tables.json</c> and <c>Tables/bc5_tables.json</c>, fitted to the reference dump. Immutable.
/// </summary>
public sealed class RgtcTables
{
    public const int Slots = 256;

    public const int PairSymbols = 64;

    /// <summary>Endpoint bytes per preset slot: e0 and e1 of each of two channels (BC4 uses the first channel).</summary>
    public const int SlotFields = 4;

    private const int IndexBits = 3;
    private const int IndexMask = (1 << IndexBits) - 1;
    private const int PairValues = 2;

    private RgtcTables(ReadOnlySpan<byte> pairs, ReadOnlySpan<byte> presets, ReadOnlySpan<byte> fitted)
    {
        PairCodes = new uint[PairSymbols];
        Presets = new uint[Slots];
        Unfitted = new byte[Slots];

        for (var symbol = 0; symbol < PairSymbols; symbol++)
        {
            var first = pairs[symbol * PairValues] & IndexMask;
            var second = pairs[(symbol * PairValues) + 1] & IndexMask;
            PairCodes[symbol] = (uint)first | ((uint)second << IndexBits);
        }

        for (var slot = 0; slot < Slots; slot++)
        {
            Presets[slot] = BinaryPrimitives.ReadUInt32LittleEndian(presets.Slice(slot * SlotFields, SlotFields));

            if (fitted[slot] == 0)
            {
                Unfitted[slot] = 1;
            }
        }
    }

    /// <summary>Gets the embedded BC4 tables, or null when the build has none (BC4 pages then cannot be decoded).</summary>
    public static RgtcTables? Bc4 { get; } = BcPackTableReader.ReadRgtc(BcPackCodec.Bc4);

    /// <summary>Gets the embedded BC5 tables, or null when the build has none (BC5 pages then cannot be decoded).</summary>
    public static RgtcTables? Bc5 { get; } = BcPackTableReader.ReadRgtc(BcPackCodec.Bc5);

    /// <summary>Gets each pair symbol's two indices as a 6-bit field: the first pixel's index, then the second's.</summary>
    internal uint[] PairCodes { get; }

    /// <summary>Gets each slot's endpoints, one byte per field: channel 0 e0, e1, channel 1 e0, e1.</summary>
    internal uint[] Presets { get; }

    /// <summary>Gets 1 for each slot that is not fitted (it reads as zeros, and blocks reaching it are counted).</summary>
    internal byte[] Unfitted { get; }

    /// <summary>Tables from explicit values.</summary>
    /// <param name="pairs">64 x (first index, second index).</param>
    /// <param name="presets">256 x 4 endpoint bytes.</param>
    /// <param name="fitted">256 flags, non-zero where the slot is fitted.</param>
    internal static RgtcTables Create(ReadOnlySpan<byte> pairs, ReadOnlySpan<byte> presets, ReadOnlySpan<byte> fitted) =>
        new(pairs, presets, fitted);
}
