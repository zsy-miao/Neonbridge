// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The inverse and forward rules follow the lane decoder of Microsoft silicon-rans mdu_minirans.v (MIT), as ported in
// bcpack-rs src/rans.rs (MIT); see THIRD_PARTY_NOTICES.md.

using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The 9 x 16 static rANS models of the lane decoder, laid out for the decode loops. A lane addresses its model through
/// <c>segmentBase = model * 16</c> into the segment tables and <c>symbolBase = model * 512</c> into
/// <see cref="Forward"/>, with <c>model = alphabet * 16 + q</c>. Built once at start-up from <see cref="Table"/> in
/// aligned native memory that never moves or changes.
/// </summary>
internal static unsafe class RansModels
{
    public const int Count = MduModelTable.AlphabetIndices * MduModelTable.ModelsPerAlphabet;

    public const int SegmentStride = 16;

    public const int SymbolStride = 512;

    /// <summary>Thresholds a BC1 lane uses: its alphabet index is at most 7, so segments 1 to 8.</summary>
    public const int Bc1Thresholds = 8;

    /// <summary>Probabilities are 16 bits: the low half of the state picks the symbol.</summary>
    public const int HalfShift = 16;

    public const uint Low16 = 0xFFFF;

    public const uint SymbolMask = 0x1FF;

    /// <summary>The reciprocal divide shifts right by 15 + rs.</summary>
    public const uint ReciprocalShift = 15;

    private const int Alignment = 64;
    private const uint Disabled = 0x10000;
    private const int MaxThresholds = MduModelTable.Segments - 1;
    private const int WordBits = 32;

    // A model of alphabet index a enables segment 0 and one segment per symbol bit: a + 2 in all.
    private const int SegmentsOverAlphabet = 2;

    static RansModels()
    {
        Table = BcPackTableReader.ReadModels();
        Thresholds = Allocate(Count * SegmentStride);
        SegA = Allocate(Count * SegmentStride);
        SegB = Allocate(Count * SegmentStride);
        ForwardSegment = Allocate(Count * SegmentStride);
        Forward = Allocate(Count * SymbolStride);

        for (var alphabet = 0; alphabet < MduModelTable.AlphabetIndices; alphabet++)
        {
            for (var q = 0; q < MduModelTable.ModelsPerAlphabet; q++)
            {
                BuildSegments(alphabet, q);
                BuildForward(alphabet, q);
            }
        }
    }

    /// <summary>Gets the exact tables the models are built from.</summary>
    public static MduModelTable Table { get; }

    /// <summary>Gets the segment thresholds base[1..15] per model, <see cref="Disabled"/> past the model's alphabet.</summary>
    public static uint* Thresholds { get; }

    /// <summary>Gets, per model and segment, <c>(rn - base) &amp; 0xFFFF | rf &lt;&lt; 16</c> (rf is below 2^16).</summary>
    public static uint* SegA { get; }

    /// <summary>Gets, per model and segment, the segment's first symbol <c>| (15 + rs) &lt;&lt; 16</c>.</summary>
    public static uint* SegB { get; }

    /// <summary>Gets, per model and segment, <c>freq | base &lt;&lt; 16</c>: the inputs of <see cref="ForwardExact"/>.</summary>
    public static uint* ForwardSegment { get; }

    /// <summary>Gets, per model and symbol, <c>freq | cumulative freq &lt;&lt; 16</c> with the slot-0 rule applied.</summary>
    public static uint* Forward { get; }

    public static int Index(int alphabet, int q) => (alphabet * MduModelTable.ModelsPerAlphabet) + q;

    /// <summary>
    /// The symbol for the low 16 state bits. The enabled segment bases never decrease, so the hardware's priority
    /// encoder equals the number of thresholds <paramref name="cf"/> reaches (the tests check this against
    /// <see cref="InverseExact"/> for every model and input).
    /// </summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static uint Inverse(uint segmentBase, uint cf)
    {
        var segment = segmentBase + Segment(segmentBase, cf);
        var a = SegA[segment];
        var b = SegB[segment];
        var steps = ((((cf + a) & Low16) * (a >> HalfShift)) >> (int)(b >> HalfShift)) & SymbolMask;

        return ((b & Low16) + steps) & SymbolMask;
    }

    /// <summary>The segment of <paramref name="cf"/>: how many thresholds it reaches.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static uint Segment(uint segmentBase, uint cf)
    {
        var thresholds = Thresholds + segmentBase;
        uint segment = 0;

        for (var k = 0; k < MaxThresholds; k++)
        {
            segment += Unsafe.BitCast<bool, byte>(cf >= thresholds[k]);
        }

        return segment;
    }

    /// <summary>The state after decoding <paramref name="symbol"/> from state <paramref name="x"/>.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static uint Advance(uint symbolBase, uint x, uint symbol)
    {
        var forward = Forward[symbolBase + symbol];

        return ((forward & Low16) * (x >> HalfShift)) + (x & Low16) - (forward >> HalfShift);
    }

    /// <summary>The hardware inverse as written: a comparator chain over the enabled segments, then the reciprocal divide.</summary>
    public static uint InverseExact(int alphabet, int q, uint cf)
    {
        var row = MduModelTable.Row(alphabet, q);
        var segment = 0;

        for (var k = alphabet + 1; k > 0; k--)
        {
            if (cf >= Table.Base[row + k])
            {
                segment = k;

                break;
            }
        }

        var distance = (cf - Table.Base[row + segment] + Table.Rn[row + segment]) & Low16;
        var steps = ((distance * Table.Rf[row + segment]) >> (int)ReciprocalShift >> Table.Rs[row + segment]) & SymbolMask;

        return (FirstSymbol(segment) + steps) & SymbolMask;
    }

    /// <summary>(frequency, cumulative frequency) of <paramref name="symbol"/>; slot 0 is reserved (cf 0 gives f - 1, 1).</summary>
    public static (uint Freq, uint Cum) ForwardExact(int alphabet, int q, uint symbol)
    {
        var row = MduModelTable.Row(alphabet, q);
        var segment = WordBits - BitOperations.LeadingZeroCount(symbol);
        uint freq = Table.Freq[row + segment];
        var cum = (Table.Base[row + segment] + (freq * (symbol - FirstSymbol(segment)))) & Low16;

        if (cum == 0)
        {
            return ((freq - 1) & Low16, 1);
        }

        return (freq, cum);
    }

    /// <summary>The first symbol of a segment: 0, then 2^(segment - 1).</summary>
    public static uint FirstSymbol(int segment)
    {
        if (segment == 0)
        {
            return 0;
        }

        return 1u << (segment - 1);
    }

    private static uint* Allocate(int count)
    {
        return (uint*)NativeMemory.AlignedAlloc((nuint)(count * sizeof(uint)), Alignment);
    }

    private static void BuildSegments(int alphabet, int q)
    {
        var model = Index(alphabet, q) * SegmentStride;
        var row = MduModelTable.Row(alphabet, q);
        var enabled = alphabet + SegmentsOverAlphabet;

        for (var k = 1; k < SegmentStride; k++)
        {
            Thresholds[model + k - 1] = k < enabled ? Table.Base[row + k] : Disabled;
        }

        for (var segment = 0; segment < SegmentStride; segment++)
        {
            uint a = 0;
            uint b = 0;
            uint forward = 0;

            // rf is below 2^16 and rs at most 16 in the tables, so both halves fit and 15 + rs is a valid shift.
            if (segment < MduModelTable.Segments)
            {
                forward = Table.Freq[row + segment] | ((uint)Table.Base[row + segment] << HalfShift);
                a = ((uint)(Table.Rn[row + segment] - Table.Base[row + segment]) & Low16) | (Table.Rf[row + segment] << HalfShift);
                b = FirstSymbol(segment) | ((ReciprocalShift + Table.Rs[row + segment]) << HalfShift);
            }

            SegA[model + segment] = a;
            SegB[model + segment] = b;
            ForwardSegment[model + segment] = forward;
        }
    }

    private static void BuildForward(int alphabet, int q)
    {
        var model = Index(alphabet, q) * SymbolStride;

        for (uint symbol = 0; symbol < SymbolStride; symbol++)
        {
            var (freq, cum) = ForwardExact(alphabet, q, symbol);
            Forward[model + symbol] = freq | (cum << HalfShift);
        }
    }
}
