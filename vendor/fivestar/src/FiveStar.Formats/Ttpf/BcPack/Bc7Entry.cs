// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// A decoded BC7 block as the block layer predicts from it: the control word (mode in bits 0-2, rotation 3-4, index
/// selection 5, subsets 6-7, partition 8-13), per subset the endpoints R0 R1 G0 G1 B0 B1 A0 A1 left-aligned in 16 bits,
/// and the bits and signed values of the primary and secondary index sets.
/// </summary>
internal unsafe struct Bc7Entry
{
    public const int Subsets = 3;

    public const int Columns = 8;

    public const int RowValues = Subsets * Columns;

    public const int IndexSets = 2;

    public const int Pixels = 16;

    public const int IndexValues = IndexSets * Pixels;

    public ushort Control;

    /// <summary>Subset s column c at <c>s * 8 + c</c>.</summary>
    public fixed ushort Rows[RowValues];

    public fixed byte Bits[IndexSets];

    /// <summary>Set s pixel p at <c>s * 16 + p</c>.</summary>
    public fixed short Indices[IndexValues];
}
