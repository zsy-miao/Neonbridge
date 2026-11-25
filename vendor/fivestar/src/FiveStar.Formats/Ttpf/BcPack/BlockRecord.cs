// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The record rules follow VIctory (crates/victory-formats/src/bcpack.rs, GPL-3.0); see THIRD_PARTY_NOTICES.md.

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// What every block layer shares. A block's record is its lane symbols, lane by lane (on 16-byte blocks E then O of
/// each lane). Lanes 0 and 1 give two back references: the index reference ix, and the endpoint reference ix +
/// zigzag(d). A reference r of block i is 0 (nothing: a zero entry), block i - r while that is not before block 0, else
/// entry r - i - 1 of the codec's dictionary.
/// </summary>
internal static unsafe class BlockRecord
{
    /// <summary>Decoded blocks a reference reaches.</summary>
    public const int HistorySize = 256;

    public const int HistoryMask = HistorySize - 1;

    private const int ByteMask = 0xFF;
    private const int NibbleBits = 4;
    private const int ResidualLanes = 2;
    private const int WordBits = 16;

    /// <summary>Zigzag code (0, -1, +1, -2, ...) modulo 2^<paramref name="bits"/>.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static byte Unzigzag(byte code, int bits)
    {
        var mask = (1 << bits) - 1;

        return (byte)(((code >> 1) ^ (0 - (code & 1))) & mask);
    }

    /// <summary>Zigzag code as a signed value.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static int Signed(int code) => (code >> 1) ^ (0 - (code & 1));

    /// <summary>The endpoint and index references; <paramref name="paired"/> when each is two 4-bit lane symbols (E, O).</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static (int Endpoints, int Indices) References(byte* record, bool paired)
    {
        uint distance = record[0];
        uint index = record[1];

        if (paired)
        {
            distance = RgtcSymbols.Interleave(record[0], record[1]);
            index = RgtcSymbols.Interleave(record[ResidualLanes], record[ResidualLanes + 1]);
        }

        return ((int)((index + RgtcSymbols.Unzigzag(distance)) & ByteMask), (int)index);
    }

    /// <summary>The entry reference <paramref name="reference"/> of <paramref name="block"/> names.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static T* Pick<T>(int block, int reference, T* history, T* dictionary, T* zero)
        where T : unmanaged
    {
        if (reference == 0)
        {
            return zero;
        }

        if (reference <= block)
        {
            return history + ((block - reference) & HistoryMask);
        }

        return dictionary + (reference - block - 1);
    }

    /// <summary>
    /// A byte's even and odd bits as two zigzag 4-bit residuals, the even one first unless <paramref name="oddFirst"/>.
    /// </summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static (int First, int Second) Nibbles(byte value, bool oddFirst)
    {
        var even = Signed(Gather(value));
        var odd = Signed(Gather(value >> 1));

        if (oddFirst)
        {
            return (odd, even);
        }

        return (even, odd);
    }

    /// <summary>A residual plus a reference index, both left-aligned to 16 bits at their own widths, back to <paramref name="bits"/> bits (signed).</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static short AddIndex(int residual, int bits, int reference, int referenceBits)
    {
        if (bits == 0)
        {
            return 0;
        }

        var sum = (short)(ushort)(Align(residual, bits) + Align(reference, referenceBits));

        return (short)(sum >> (WordBits - bits));
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int Align(int value, int bits)
    {
        if (bits == 0 || bits > WordBits)
        {
            return 0;
        }

        return (ushort)(value << (WordBits - bits));
    }

    /// <summary>Bits 0, 2, 4 and 6 of <paramref name="value"/> as a nibble.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int Gather(int value)
    {
        var nibble = 0;

        for (var k = 0; k < NibbleBits; k++)
        {
            nibble |= ((value >> (k << 1)) & 1) << k;
        }

        return nibble;
    }
}
