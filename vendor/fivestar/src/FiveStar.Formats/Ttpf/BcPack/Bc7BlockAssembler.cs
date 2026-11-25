// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// The lane meanings follow VIctory (crates/victory-formats/src/bcpack.rs, GPL-3.0); see THIRD_PARTY_NOTICES.md.

using System.Runtime.CompilerServices;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The block layer of BC7 (<see cref="Bc7Entry"/>). Lane 15 (E low 5 bits, O low 4) selects a control word from the
/// table. Lanes 2-4 are subset 0's R G B (E endpoint 0, O endpoint 1), lanes 6-8 subset 1's, lane 10 subset 2's R;
/// lane 5 is alpha of subset 0 or G of subset 2, lane 9 alpha of subset 1 or B of subset 2. Each is a zigzag residual
/// added at the channel's precision to the referenced subset (or its first one). Lanes 11-14 hold the primary index
/// residuals, rows 0-3; the secondary set of modes 4 and 5 uses lanes 6-8 and 10.
/// </summary>
internal readonly unsafe struct Bc7BlockAssembler : IBlockAssembler
{
    private const int ControlLaneE = 30;
    private const int ControlLaneO = 31;
    private const int ControlLowMask = 31;
    private const int ControlHighMask = 15;
    private const int ControlHighShift = 5;
    private const int ModeMask = 7;
    private const int RotationShift = 3;
    private const int RotationMask = 3;
    private const int SelectionShift = 5;
    private const int SubsetShift = 6;
    private const int PartitionShift = 8;
    private const int PartitionMask = 63;
    private const int LowByte = 0xFF;
    private const int WordBits = 16;
    private const int Channels = 4;
    private const int Alpha = 3;
    private const int ColourColumns = 6;
    private const int RowStart0 = 4;
    private const int RowStart1 = 12;
    private const int RowStart2 = 20;
    private const int ThirdSubsetG = 10;
    private const int ThirdSubsetB = 18;
    private const int ThreeSubsets = 3;
    private const int TwoSubsets = 2;
    private const int Mode0 = 0;
    private const int Mode1 = 1;
    private const int Mode4 = 4;
    private const int Mode5 = 5;
    private const int Mode6 = 6;
    private const int Mode0PartitionBits = 4;
    private const int PartitionBits = 6;
    private const int RotationBits = 2;
    private const int IndexRows = 4;
    private const int PixelsPerRow = 4;
    private const int OddFirstRows = 3;
    private const int SymbolsPerRow = 2;

    // First record position of each index row, per set.
    private static readonly byte[] PrimaryRows = [22, 24, 26, 28];
    private static readonly byte[] SecondaryRows = [12, 14, 16, 20];

    private readonly byte* _output;
    private readonly Bc7Entry* _history;
    private readonly Bc7Entry* _dictionary;
    private readonly Bc7Entry* _zero;
    private readonly ushort* _controls;

    /// <param name="output">Room for every block.</param>
    /// <param name="history">256 entries.</param>
    /// <param name="zero">One zero entry.</param>
    /// <param name="tables">The tables.</param>
    public Bc7BlockAssembler(byte* output, Bc7Entry* history, Bc7Entry* zero, BcPackTables tables)
    {
        _output = output;
        _history = history;
        _zero = zero;
        _dictionary = BcPackTables.Address(tables.Bc7);
        _controls = BcPackTables.Address(tables.Controls);
    }

    [HotPath]
    public bool Assemble(int block, byte* record)
    {
        var control = _controls[((record[ControlLaneO] & ControlHighMask) << ControlHighShift) | (record[ControlLaneE] & ControlLowMask)];

        if (control == 0)
        {
            return false;
        }

        var (endpointReference, indexReference) = BlockRecord.References(record, paired: true);
        var endpoints = BlockRecord.Pick(block, endpointReference, _history, _dictionary, _zero);
        var indices = BlockRecord.Pick(block, indexReference, _history, _dictionary, _zero);
        var precision = stackalloc byte[Channels];
        Bc7Entry value = default;
        value.Control = control;

        Precision(control, precision);
        Endpoints(record, precision, endpoints, &value);
        Indices(record, indices, &value);
        _history[block & BlockRecord.HistoryMask] = value;
        Write(&value, precision, _output + ((long)block * BlockBits.BlockBytes));

        return true;
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int Subsets(int control) => (control & LowByte) >> SubsetShift;

    /// <summary>Stored bits per channel: the p-bit counts as red's lowest; rotation swaps alpha with the rotated channel.</summary>
    [HotPath]
    private static void Precision(int control, byte* precision)
    {
        var mode = control & ModeMask;
        var rotation = (control >> RotationShift) & RotationMask;
        var bits = Bc7Partitions.EndpointBits[mode];

        for (var channel = 0; channel < Channels; channel++)
        {
            precision[channel] = bits[channel];
        }

        if (rotation != 0)
        {
            (precision[Alpha], precision[rotation - 1]) = (precision[rotation - 1], precision[Alpha]);
        }
        else if (Bc7Partitions.HasPBits[mode])
        {
            precision[0]++;
        }
    }

    /// <summary>The residual symbol of subset <paramref name="row"/> column <paramref name="column"/>.</summary>
    [HotPath]
    private static byte Stored(byte* record, int row, int column, bool three)
    {
        if (row == ThreeSubsets - 1)
        {
            return ThirdRow(record, column, three);
        }

        var start = RowStart0;

        if (row == 1)
        {
            start = RowStart1;
        }

        if (column >= ColourColumns && three)
        {
            return 0;
        }

        return record[start + column];
    }

    [HotPath]
    private static byte ThirdRow(byte* record, int column, bool three)
    {
        if (column < TwoSubsets)
        {
            return record[RowStart2 + column];
        }

        if (!three || column >= ColourColumns)
        {
            return 0;
        }

        if (column < Channels)
        {
            return record[ThirdSubsetG + column - TwoSubsets];
        }

        return record[ThirdSubsetB + column - Channels];
    }

    [HotPath]
    private static void Endpoints(byte* record, byte* precision, Bc7Entry* reference, Bc7Entry* value)
    {
        var subsets = Subsets(value->Control);
        var common = Subsets(reference->Control);

        if (common > subsets)
        {
            common = subsets;
        }

        var three = subsets == ThreeSubsets;
        var columns = ColourColumns;

        if (precision[Alpha] > 0)
        {
            columns = Bc7Entry.Columns;
        }

        for (var column = 0; column < columns; column++)
        {
            int bits = precision[column >> 1];

            if (bits == 0)
            {
                continue;
            }

            var shift = WordBits - bits;
            var mask = ushort.MaxValue << shift;

            for (var row = 0; row < subsets; row++)
            {
                int code = Stored(record, row, column, three);
                var residual = (ushort)(BlockRecord.Signed(code) << shift);
                var from = 0;

                if (row < common)
                {
                    from = row;
                }

                var predicted = reference->Rows[(from * Bc7Entry.Columns) + column] & mask;
                value->Rows[(row * Bc7Entry.Columns) + column] = (ushort)(predicted + residual);
            }
        }
    }

    [HotPath]
    private static void Indices(byte* record, Bc7Entry* reference, Bc7Entry* value)
    {
        var bits = Bc7Partitions.IndexBits[value->Control & ModeMask];

        for (var set = 0; set < Bc7Entry.IndexSets; set++)
        {
            value->Bits[set] = bits[set];

            if (bits[set] == 0)
            {
                continue;
            }

            var rows = PrimaryRows;

            if (set == 1)
            {
                rows = SecondaryRows;
            }

            for (var row = 0; row < IndexRows; row++)
            {
                for (var half = 0; half < SymbolsPerRow; half++)
                {
                    var (first, second) = BlockRecord.Nibbles(record[rows[row] + half], row < OddFirstRows);
                    var p = (set * Bc7Entry.Pixels) + (row * PixelsPerRow) + (half * SymbolsPerRow);
                    value->Indices[p] = BlockRecord.AddIndex(first, bits[set], reference->Indices[p], reference->Bits[set]);
                    value->Indices[p + 1] = BlockRecord.AddIndex(second, bits[set], reference->Indices[p + 1], reference->Bits[set]);
                }
            }
        }
    }

    [HotPath]
    private static void Write(Bc7Entry* value, byte* precision, byte* output)
    {
        int control = value->Control;
        var mode = control & ModeMask;
        var bits = new BlockBits();

        Header(control, ref bits);
        WriteEndpoints(value, precision, ref bits);
        WritePBits(value, precision, ref bits);

        switch (mode)
        {
            case Mode4:
                WriteIndices(value, 1, ref bits);
                WriteIndices(value, 0, ref bits);
                break;
            case Mode5:
                WriteIndices(value, 0, ref bits);
                WriteIndices(value, 1, ref bits);
                break;
            default:
                WriteIndices(value, 0, ref bits);
                break;
        }

        bits.WriteTo(output);
    }

    [HotPath]
    private static void Header(int control, ref BlockBits bits)
    {
        var mode = control & ModeMask;
        var partition = (control >> PartitionShift) & PartitionMask;
        var rotation = (control >> RotationShift) & RotationMask;
        bits.Put(1UL << mode, mode + 1);

        switch (mode)
        {
            case Mode0:
                bits.Put((ulong)partition, Mode0PartitionBits);
                break;
            case Mode4:
                bits.Put((ulong)rotation, RotationBits);
                bits.Put((ulong)((control >> SelectionShift) & 1), 1);
                break;
            case Mode5:
                bits.Put((ulong)rotation, RotationBits);
                break;
            case Mode6:
                break;
            default:
                bits.Put((ulong)partition, PartitionBits);
                break;
        }
    }

    /// <summary>Channel by channel (alpha and the rotated channel swapped), endpoint by endpoint; p-bit modes drop red's lowest bit here.</summary>
    [HotPath]
    private static void WriteEndpoints(Bc7Entry* value, byte* precision, ref BlockBits bits)
    {
        int control = value->Control;
        var mode = control & ModeMask;
        var rotation = (control >> RotationShift) & RotationMask;
        var ends = Subsets(control) << 1;

        for (var step = 0; step < Channels; step++)
        {
            var channel = step;

            if (rotation != 0 && step == Alpha)
            {
                channel = rotation - 1;
            }
            else if (rotation != 0 && step == rotation - 1)
            {
                channel = Alpha;
            }

            int width = precision[channel];

            if (width == 0)
            {
                continue;
            }

            var drop = 0;

            if (channel == 0 && Bc7Partitions.HasPBits[mode])
            {
                drop = 1;
            }

            for (var end = 0; end < ends; end++)
            {
                var field = Field(value, end >> 1, (channel << 1) + (end & 1), width);
                bits.Put((ulong)(field >> drop), width - drop);
            }
        }
    }

    [HotPath]
    private static void WritePBits(Bc7Entry* value, byte* precision, ref BlockBits bits)
    {
        int control = value->Control;
        var mode = control & ModeMask;
        var subsets = Subsets(control);

        if (mode == Mode1)
        {
            for (var row = 0; row < subsets; row++)
            {
                bits.Put((ulong)(Field(value, row, 0, precision[0]) & 1), 1);
            }

            return;
        }

        if (!Bc7Partitions.HasPBits[mode])
        {
            return;
        }

        for (var end = 0; end < subsets << 1; end++)
        {
            bits.Put((ulong)(Field(value, end >> 1, end & 1, precision[0]) & 1), 1);
        }
    }

    [HotPath]
    private static void WriteIndices(Bc7Entry* value, int set, ref BlockBits bits)
    {
        int control = value->Control;
        var subsets = Subsets(control);
        var partition = (control >> PartitionShift) & PartitionMask;
        var second = -1;
        var third = -1;

        if (subsets == TwoSubsets)
        {
            second = Bc7Partitions.Anchor2[partition];
        }
        else if (subsets == ThreeSubsets)
        {
            second = Bc7Partitions.Anchor3Second[partition];
            third = Bc7Partitions.Anchor3Third[partition];
        }

        int width = value->Bits[set];

        for (var p = 0; p < Bc7Entry.Pixels; p++)
        {
            var count = width;

            if (p == 0 || p == second || p == third)
            {
                count--;
            }

            bits.Put((ushort)value->Indices[(set * Bc7Entry.Pixels) + p], count);
        }
    }

    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    private static int Field(Bc7Entry* value, int row, int column, int width) =>
        value->Rows[(row * Bc7Entry.Columns) + column] >> (WordBits - width);
}
