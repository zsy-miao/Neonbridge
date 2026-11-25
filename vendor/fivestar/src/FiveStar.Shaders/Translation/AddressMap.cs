// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Shaders.Translation;

/// <summary>
/// The address map translated programs read console GPU addresses through (rdna_prelude.hlsli, <c>gpu_page</c>): a raw
/// buffer at heap slot <see cref="HeapSlot"/>. Level 1 holds one dword per 4 GiB span of the 47-bit address space (the
/// address's high dword): the byte offset of the span's page table. A page table holds one entry per 64 KiB page: the
/// heap index of a raw SRV over the page (its UAV is the next index) and the byte offset of the page's start in that
/// view. Table 0 is the null table every span starts with; it and every page nothing maps name the null view pair.
/// </summary>
public sealed class AddressMap
{
    /// <summary>rdna_prelude GPU_MAP_HEAP.</summary>
    public const uint HeapSlot = 7;

    /// <summary>rdna_prelude GPU_MAP_SPANS: the high dwords a user-mode address can have.</summary>
    public const uint Spans = 0x8000;

    private const int PageShift = 16;
    private const int SpanShift = 32;
    private const int EntryBytes = 8;
    private const int SpanEntryBytes = 4;
    private const int ViewOffsetField = 4;
    /// <summary>The view offset is the entry's high dword.</summary>
    private const int OffsetShift = 32;
    private const int PagesPerSpan = 1 << (SpanShift - PageShift);
    private const int TableBytes = PagesPerSpan * EntryBytes;
    private const int Level1Bytes = (int)Spans * SpanEntryBytes;

    private readonly uint _nullView;
    private int _tables;

    /// <param name="capacity">Page tables beyond the null table: the 4 GiB spans that can be mapped.</param>
    /// <param name="nullView">The SRV every unmapped page names (its UAV is the next index).</param>
    public AddressMap(int capacity, uint nullView)
    {
        Capacity = capacity;
        _nullView = nullView;
    }

    public int Capacity { get; }

    /// <summary>Bytes of the map buffer.</summary>
    public int Bytes => Level1Bytes + ((Capacity + 1) * TableBytes);

    /// <summary>Starts a map: every span names the null table, whose pages name the null view.</summary>
    public void Clear(Span<byte> map)
    {
        map[..Bytes].Clear();
        _tables = 1;

        for (var span = 0; span < Spans; span++)
        {
            BinaryPrimitives.WriteUInt32LittleEndian(map[(span * SpanEntryBytes)..], Level1Bytes);
        }

        FillTable(map, Level1Bytes);
    }

    /// <summary>
    /// Maps the pages of [<paramref name="address"/>, + <paramref name="size"/>) to <paramref name="view"/>, whose byte 0
    /// is console address <paramref name="viewStart"/>. False when a span outside the map or past its capacity leaves
    /// pages unmapped.
    /// </summary>
    public bool Map(Span<byte> map, ulong address, ulong size, uint view, ulong viewStart)
    {
        if (size == 0)
        {
            return true;
        }

        var complete = true;
        var last = (address + size - 1) >> PageShift;

        for (var page = address >> PageShift; page <= last; page++)
        {
            var table = Table(map, page >> (SpanShift - PageShift));

            if (table < 0)
            {
                complete = false;
                continue;
            }

            // One 8-byte store: programs read the map while pages are remapped, and must not see a view with another's offset.
            var entry = map[(table + ((int)(page % PagesPerSpan) * EntryBytes))..];
            ulong offset = unchecked((uint)((page << PageShift) - viewStart));
            BinaryPrimitives.WriteUInt64LittleEndian(entry, view | (offset << OffsetShift));
        }

        return complete;
    }

    /// <summary>
    /// One page's entry with <paramref name="view"/> and a raw offset field, <paramref name="field"/>: the tag of a page
    /// that waits for memory (the loader's window feedback). False when no page table is left for it.
    /// </summary>
    public bool Mark(Span<byte> map, ulong address, uint view, uint field)
    {
        var page = address >> PageShift;
        var table = Table(map, page >> (SpanShift - PageShift));

        if (table < 0)
        {
            return false;
        }

        var entry = map[(table + ((int)(page % PagesPerSpan) * EntryBytes))..];
        BinaryPrimitives.WriteUInt64LittleEndian(entry, view | ((ulong)field << OffsetShift));

        return true;
    }

    /// <summary>The view and offset field of the page at <paramref name="address"/>, as a program reads them.</summary>
    public static (uint View, uint Field) Read(ReadOnlySpan<byte> map, ulong address)
    {
        var table = (int)BinaryPrimitives.ReadUInt32LittleEndian(map[((int)((address >> SpanShift) % Spans) * SpanEntryBytes)..]);
        var entry = BinaryPrimitives.ReadUInt64LittleEndian(map[(table + ((int)((address >> PageShift) % PagesPerSpan) * EntryBytes))..]);

        return ((uint)entry, (uint)(entry >> OffsetShift));
    }

    /// <summary>The byte offset of a span's own page table, made on first use; -1 when none is left.</summary>
    private int Table(Span<byte> map, ulong span)
    {
        if (span >= Spans)
        {
            return -1;
        }

        var slot = map[((int)span * SpanEntryBytes)..];
        var table = (int)BinaryPrimitives.ReadUInt32LittleEndian(slot);

        if (table != Level1Bytes)
        {
            return table;
        }

        if (_tables > Capacity)
        {
            return -1;
        }

        table = Level1Bytes + (_tables * TableBytes);
        _tables++;
        FillTable(map, table);
        BinaryPrimitives.WriteUInt32LittleEndian(slot, (uint)table);

        return table;
    }

    private void FillTable(Span<byte> map, int table)
    {
        for (var page = 0; page < PagesPerSpan; page++)
        {
            var entry = map[(table + (page * EntryBytes))..];
            BinaryPrimitives.WriteUInt32LittleEndian(entry, _nullView);
            BinaryPrimitives.WriteUInt32LittleEndian(entry[ViewOffsetField..], 0);
        }
    }
}
