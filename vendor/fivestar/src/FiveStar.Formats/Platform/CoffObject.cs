// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Formats.Platform;

/// <summary>
/// A COFF object file (PE/COFF specification): file header, section table with relocations, symbol table and string
/// table. Big objects (<c>/bigobj</c>) and short import members are not COFF objects in this sense.
/// </summary>
public sealed class CoffObject
{
    public const ushort MachineAmd64 = 0x8664;

    private const int FileHeaderSize = 20;
    private const int SectionCountField = 2;
    private const int SymbolTableField = 8;
    private const int SymbolCountField = 12;
    private const int OptionalHeaderSizeField = 16;

    private const int SectionHeaderSize = 40;
    private const int NameSize = 8;
    private const int RawSizeField = 16;
    private const int RawOffsetField = 20;
    private const int RelocationsField = 24;
    private const int RelocationCountField = 32;

    private const int RelocationSize = 10;
    private const int RelocationSymbolField = 4;
    private const int RelocationTypeField = 8;

    private const int SymbolSize = 18;
    private const int SymbolValueField = 8;
    private const int SymbolSectionField = 12;
    private const int SymbolTypeField = 14;
    private const int SymbolAuxCountField = 17;
    private const int LongNameOffsetField = 4;
    private const ushort FunctionType = 0x20;

    private readonly ReadOnlyMemory<byte> _data;

    private CoffObject(ReadOnlyMemory<byte> data, IReadOnlyList<CoffSymbol?> symbols)
    {
        _data = data;
        Symbols = symbols;
        Sections = ReadSections();
    }

    /// <summary>Symbol records by index; auxiliary records are null.</summary>
    public IReadOnlyList<CoffSymbol?> Symbols { get; }

    public IReadOnlyList<CoffSection> Sections { get; }

    /// <summary>Reads an x64 COFF object, or returns null for anything else (import members, linker members, bigobj).</summary>
    public static CoffObject? TryRead(ReadOnlyMemory<byte> data)
    {
        var span = data.Span;

        if (span.Length < FileHeaderSize || BinaryPrimitives.ReadUInt16LittleEndian(span) != MachineAmd64)
        {
            return null;
        }

        return new CoffObject(data, ReadSymbols(span));
    }

    private static List<CoffSymbol?> ReadSymbols(ReadOnlySpan<byte> span)
    {
        var table = (int)BinaryPrimitives.ReadUInt32LittleEndian(span[SymbolTableField..]);
        var count = (int)BinaryPrimitives.ReadUInt32LittleEndian(span[SymbolCountField..]);
        var strings = table + (count * SymbolSize);
        var symbols = new List<CoffSymbol?>(count);

        while (symbols.Count < count)
        {
            var record = span.Slice(table + (symbols.Count * SymbolSize), SymbolSize);
            var section = BinaryPrimitives.ReadInt16LittleEndian(record[SymbolSectionField..]);
            var type = BinaryPrimitives.ReadUInt16LittleEndian(record[SymbolTypeField..]);
            var value = BinaryPrimitives.ReadUInt32LittleEndian(record[SymbolValueField..]);
            symbols.Add(new CoffSymbol(SymbolName(span, record, strings), value, section, type == FunctionType));

            // Auxiliary records follow their symbol and occupy symbol indices.
            symbols.AddRange(Enumerable.Repeat<CoffSymbol?>(null, record[SymbolAuxCountField]));
        }

        return symbols;
    }

    private static string SymbolName(ReadOnlySpan<byte> span, ReadOnlySpan<byte> record, int strings)
    {
        if (BinaryPrimitives.ReadUInt32LittleEndian(record) != 0)
        {
            return Encoding.UTF8.GetString(record[..NameSize].TrimEnd((byte)0));
        }

        var name = span[(strings + (int)BinaryPrimitives.ReadUInt32LittleEndian(record[LongNameOffsetField..]))..];

        return Encoding.UTF8.GetString(name[..name.IndexOf((byte)0)]);
    }

    private List<CoffSection> ReadSections()
    {
        var span = _data.Span;
        var count = BinaryPrimitives.ReadUInt16LittleEndian(span[SectionCountField..]);
        var table = FileHeaderSize + BinaryPrimitives.ReadUInt16LittleEndian(span[OptionalHeaderSizeField..]);
        var sections = new List<CoffSection>(count);

        for (var index = 0; index < count; index++)
        {
            var header = span.Slice(table + (index * SectionHeaderSize), SectionHeaderSize);
            var rawSize = (int)BinaryPrimitives.ReadUInt32LittleEndian(header[RawSizeField..]);
            var rawOffset = (int)BinaryPrimitives.ReadUInt32LittleEndian(header[RawOffsetField..]);
            var data = ReadOnlyMemory<byte>.Empty;

            if (rawOffset != 0)
            {
                data = _data.Slice(rawOffset, rawSize);
            }

            var relocations = ReadRelocations(
                (int)BinaryPrimitives.ReadUInt32LittleEndian(header[RelocationsField..]),
                BinaryPrimitives.ReadUInt16LittleEndian(header[RelocationCountField..]));
            sections.Add(new CoffSection(Encoding.UTF8.GetString(header[..NameSize].TrimEnd((byte)0)), data, relocations));
        }

        return sections;
    }

    private List<CoffRelocation> ReadRelocations(int at, int count)
    {
        var span = _data.Span;
        var relocations = new List<CoffRelocation>(count);

        for (var entry = 0; entry < count; entry++)
        {
            var record = span.Slice(at + (entry * RelocationSize), RelocationSize);
            var index =(int)BinaryPrimitives.ReadUInt32LittleEndian(record[RelocationSymbolField..]);
            CoffSymbol? symbol = null;

            if (index < Symbols.Count)
            {
                symbol = Symbols[index];
            }

            relocations.Add(new CoffRelocation(
                BinaryPrimitives.ReadUInt32LittleEndian(record),
                symbol,
                BinaryPrimitives.ReadUInt16LittleEndian(record[RelocationTypeField..])));
        }

        return relocations;
    }
}
