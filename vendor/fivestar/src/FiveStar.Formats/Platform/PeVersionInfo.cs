// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Platform;

/// <summary>
/// Reads the string tables of a PE image's version resource (<c>RT_VERSION</c>). Every node of
/// <c>VS_VERSIONINFO</c> is <c>{u16 length, u16 value length, u16 type, UTF-16 key, padding to 4, value, padding,
/// children}</c>; the value length counts bytes for binary values (type 0) and UTF-16 units for text (type 1).
/// </summary>
public static class PeVersionInfo
{
    private const uint VersionResourceType = 16;
    private const int NodeHeaderSize = 6;
    private const int ValueLengthField = 2;
    private const int TypeField = 4;
    private const int NodeAlignment = 4;
    private const int TextType = 1;
    private const int ResourceEntriesOffset = 16;
    private const int NamedCountField = 12;
    private const int IdCountField = 14;
    private const int ResourceEntrySize = 8;
    private const uint SubdirectoryFlag = 0x80000000;

    /// <summary>Each string table as an ordered map of key to value.</summary>
    public static IReadOnlyList<OrderedMap<string, string>> StringTables(PeImage image)
    {
        var tables = new List<OrderedMap<string, string>>();
        var (rva, _) = image.Directory(PeDirectory.Resource);
        var root = rva == 0 ? null : image.Offset(rva);

        if (!root.HasValue)
        {
            return tables;
        }

        var data = VersionData(image, (int)root.Value);

        if (data is null)
        {
            return tables;
        }

        foreach (var section in Children(data.Value.Span, 0))
        {
            if (Key(data.Value.Span, section) != "StringFileInfo")
            {
                continue;
            }

            foreach (var table in Children(data.Value.Span, section))
            {
                tables.Add(ReadStrings(data.Value.Span, table));
            }
        }

        return tables;
    }

    private static OrderedMap<string, string> ReadStrings(ReadOnlySpan<byte> data, int table)
    {
        var strings = new OrderedMap<string, string>();

        foreach (var entry in Children(data, table))
        {
            var key = Key(data, entry);
            var start = ValueStart(data, entry);
            var units = BitConverter.ToUInt16(data[(entry + ValueLengthField)..]);
            var length = Math.Min(units * sizeof(char), data.Length - start);
            var text = Encoding.Unicode.GetString(data.Slice(start, Math.Max(length, 0)));

            var end = text.IndexOf('\0', StringComparison.Ordinal);
            var value = end < 0 ? text : text[..end];
            strings.Set(key, value);
        }

        return strings;
    }

    private static List<int> Children(ReadOnlySpan<byte> data, int node)
    {
        var children = new List<int>();
        var nodeEnd = node + BitConverter.ToUInt16(data[node..]);
        var valueLength = BitConverter.ToUInt16(data[(node + ValueLengthField)..]);
        var type = BitConverter.ToUInt16(data[(node + TypeField)..]);
        var valueBytes = type == TextType ? valueLength * sizeof(char) : valueLength;
        var child = Align(ValueStart(data, node) + valueBytes);

        while (child + NodeHeaderSize <= Math.Min(nodeEnd, data.Length))
        {
            var length = BitConverter.ToUInt16(data[child..]);

            if (length == 0)
            {
                break;
            }

            children.Add(child);
            child = Align(child + length);
        }

        return children;
    }

    private static string Key(ReadOnlySpan<byte> data, int node)
    {
        var start = node + NodeHeaderSize;
        var end = start;

        while (end + 1 < data.Length && (data[end] != 0 || data[end + 1] != 0))
        {
            end += sizeof(char);
        }

        return Encoding.Unicode.GetString(data[start..end]);
    }

    private static int ValueStart(ReadOnlySpan<byte> data, int node)
    {
        var key = Key(data, node);

        return Align(node + NodeHeaderSize + ((key.Length + 1) * sizeof(char)));
    }

    private static int Align(int offset)
    {
        return (offset + NodeAlignment - 1) / NodeAlignment * NodeAlignment;
    }

    // RT_VERSION -> first name -> first language -> data entry {RVA, size}.
    private static ReadOnlyMemory<byte>? VersionData(PeImage image, int root)
    {
        var names = Entry(image, root, root, VersionResourceType);
        var language = names.HasValue ? FirstTarget(image, names.Value) : null;

        if (!language.HasValue || (language.Value & SubdirectoryFlag) == 0)
        {
            return null;
        }

        var leaf = FirstTarget(image, root + (int)(language.Value & ~SubdirectoryFlag));

        if (!leaf.HasValue || (leaf.Value & SubdirectoryFlag) != 0)
        {
            return null;
        }

        var dataEntry = root + (int)leaf.Value;
        var offset = image.Offset(image.U32(dataEntry));

        if (!offset.HasValue)
        {
            return null;
        }

        return image.Data.AsMemory((int)offset.Value, (int)image.U32(dataEntry + sizeof(uint)));
    }

    private static int? Entry(PeImage image, int root, int directory, uint id)
    {
        var count = image.U16(directory + NamedCountField) + image.U16(directory + IdCountField);

        for (var index = 0; index < count; index++)
        {
            var at = directory + ResourceEntriesOffset + (index * ResourceEntrySize);

            if (image.U32(at) == id && (image.U32(at + sizeof(uint)) & SubdirectoryFlag) != 0)
            {
                return root + (int)(image.U32(at + sizeof(uint)) & ~SubdirectoryFlag);
            }
        }

        return null;
    }

    // The raw target of a directory's first entry: a subdirectory (flag set) or a data entry, relative to the root.
    private static uint? FirstTarget(PeImage image, int directory)
    {
        var count = image.U16(directory + NamedCountField) + image.U16(directory + IdCountField);

        if (count == 0)
        {
            return null;
        }

        return image.U32(directory + ResourceEntriesOffset + sizeof(uint));
    }
}
