// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Formats.Platform;

/// <summary>
/// Reads a Windows registry hive (<c>regf</c>), such as the compiled package configuration. Cells are addressed
/// relative to the first hive bin at 0x1000; each starts with an i32 size (negative = in use). Key cells (<c>nk</c>)
/// point to a subkey list (<c>lf</c>/<c>lh</c> 8-byte entries, <c>li</c> 4-byte entries, <c>ri</c> lists of lists)
/// and a value list of value cells (<c>vk</c>).
/// </summary>
public static class RegistryHive
{
    private const int RootCellField = 0x24;
    private const int BinsStart = 0x1000;
    private const int CellSizeField = 4;
    private const int SignatureSize = 2;

    // nk: u16 flags at 2, subkey count at 0x14, subkey list at 0x1C, value count at 0x24, value list at 0x28,
    // u16 name length at 0x48, name at 0x4C. Flag 0x20 = name stored as Latin-1.
    private const int KeyFlagsField = 2;
    private const int SubkeyCountField = 0x14;
    private const int SubkeyListField = 0x1C;
    private const int ValueCountField = 0x24;
    private const int ValueListField = 0x28;
    private const int KeyNameLengthField = 0x48;
    private const int KeyNameField = 0x4C;
    private const int KeyCompressedName = 0x20;

    // vk: u16 name length at 2, u32 data length at 4, data offset at 8, type at 0xC, u16 flags at 0x10, name at 0x14.
    // Flag 1 = name stored as Latin-1. Data length bit 31 = data held in the offset field itself.
    private const int ValueNameLengthField = 2;
    private const int DataLengthField = 4;
    private const int DataOffsetField = 8;
    private const int ValueTypeField = 0xC;
    private const int ValueFlagsField = 0x10;
    private const int ValueNameField = 0x14;
    private const int ValueCompressedName = 1;
    private const uint InlineData = 0x80000000;

    private const int ListCountField = 2;
    private const int ListEntries = 4;
    private const int HashListEntrySize = 8;
    private const int IndexListEntrySize = 4;

    public static RegistryKey Read(byte[] data)
    {
        if (!data.AsSpan().StartsWith("regf"u8))
        {
            throw new InvalidDataException("not a regf hive");
        }

        return ReadKey(data, U32(data, RootCellField));
    }

    private static RegistryKey ReadKey(byte[] data, uint cell)
    {
        var at = CellData(cell);

        if (!data.AsSpan(at).StartsWith("nk"u8))
        {
            throw new InvalidDataException("no nk cell at " + cell);
        }

        var flags = U16(data, at + KeyFlagsField);
        var nameBytes = data.AsSpan(at + KeyNameField, U16(data, at + KeyNameLengthField));
        var name = Decode(nameBytes, (flags & KeyCompressedName) != 0);
        var values = new List<RegistryValue>();
        var valueCount = U32(data, at + ValueCountField);

        if (valueCount != 0)
        {
            var list = CellData(U32(data, at + ValueListField));

            for (var index = 0; index < valueCount; index++)
            {
                values.Add(ReadValue(data, U32(data, list + (index * IndexListEntrySize))));
            }
        }

        var subkeys = new List<RegistryKey>();

        if (U32(data, at + SubkeyCountField) != 0)
        {
            foreach (var child in SubkeyCells(data, U32(data, at + SubkeyListField)))
            {
                subkeys.Add(ReadKey(data, child));
            }
        }

        return new RegistryKey(name, values, subkeys);
    }

    private static RegistryValue ReadValue(byte[] data, uint cell)
    {
        var at = CellData(cell);

        if (!data.AsSpan(at).StartsWith("vk"u8))
        {
            throw new InvalidDataException("no vk cell at " + cell);
        }

        var nameBytes = data.AsSpan(at + ValueNameField, U16(data, at + ValueNameLengthField));
        var name = Decode(nameBytes, (U16(data, at + ValueFlagsField) & ValueCompressedName) != 0);
        var length = U32(data, at + DataLengthField);
        var offset = U32(data, at + DataOffsetField);
        byte[] value;

        if ((length & InlineData) != 0)
        {
            var inline = new byte[sizeof(uint)];
            BinaryPrimitives.WriteUInt32LittleEndian(inline, offset);
            value = inline[..(int)Math.Min(length & ~InlineData, sizeof(uint))];
        }
        else
        {
            var start = CellData(offset);
            value = data.AsSpan(start, (int)Math.Min(length, (uint)(data.Length - start))).ToArray();
        }

        return new RegistryValue(name, U32(data, at + ValueTypeField), value);
    }

    private static List<uint> SubkeyCells(byte[] data, uint listCell)
    {
        var at = CellData(listCell);
        var signature = data.AsSpan(at, SignatureSize);
        int count = U16(data, at + ListCountField);
        var cells = new List<uint>();

        if (signature.SequenceEqual("lf"u8) || signature.SequenceEqual("lh"u8))
        {
            for (var index = 0; index < count; index++)
            {
                cells.Add(U32(data, at + ListEntries + (index * HashListEntrySize)));
            }
        }
        else if (signature.SequenceEqual("li"u8) || signature.SequenceEqual("ri"u8))
        {
            for (var index = 0; index < count; index++)
            {
                cells.Add(U32(data, at + ListEntries + (index * IndexListEntrySize)));
            }

            if (signature.SequenceEqual("ri"u8))
            {
                return ExpandIndexRoot(data, cells);
            }
        }

        return cells;
    }

    private static List<uint> ExpandIndexRoot(byte[] data, List<uint> lists)
    {
        var cells = new List<uint>();

        foreach (var list in lists)
        {
            cells.AddRange(SubkeyCells(data, list));
        }

        return cells;
    }

    private static string Decode(ReadOnlySpan<byte> bytes, bool latin1)
    {
        if (latin1)
        {
            return Encoding.Latin1.GetString(bytes);
        }

        return Encoding.Unicode.GetString(bytes);
    }

    private static int CellData(uint cell)
    {
        return BinsStart + (int)cell + CellSizeField;
    }

    private static ushort U16(byte[] data, int at)
    {
        return BinaryPrimitives.ReadUInt16LittleEndian(data.AsSpan(at));
    }

    private static uint U32(byte[] data, int at)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(data.AsSpan(at));
    }
}
