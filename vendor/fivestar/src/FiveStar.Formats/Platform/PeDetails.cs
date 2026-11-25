// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Formats.Platform;

/// <summary>
/// Reads the export names, CodeView PDB path and top-level resource types of a PE image (layouts from the PE/COFF
/// specification: export directory table, debug directory, resource directory table).
/// </summary>
public static class PeDetails
{
    // Export directory: number of functions at 20, number of names at 24, address/name/ordinal table RVAs at 28/32/36.
    private const int OrdinalBaseField = 16;
    private const int FunctionCountField = 20;
    private const int NameCountField = 24;
    private const int FunctionTableField = 28;
    private const int NameTableField = 32;
    private const int OrdinalTableField = 36;

    // Debug directory entries are 28 bytes; type at 12, data size at 16, raw data pointer at 24. Type 2 = CodeView.
    private const int DebugEntrySize = 28;
    private const int DebugTypeField = 12;
    private const int DebugSizeField = 16;
    private const int DebugPointerField = 24;
    private const uint CodeViewType = 2;
    private const int RsdsNameOffset = 24;
    private const int Nb10NameOffset = 16;

    // Resource directory table: named entry count at 12, id entry count at 14, 8-byte entries from 16.
    private const int NamedCountField = 12;
    private const int IdCountField = 14;
    private const int ResourceEntriesOffset = 16;
    private const int ResourceEntrySize = 8;
    private const uint NameIsString = 0x80000000;

    /// <summary>Named exports in name-table order, then exports by ordinal only (as <c>#N</c>).</summary>
    public static IReadOnlyList<string> Exports(PeImage image)
    {
        var names = new List<string>();
        var (rva, _) = image.Directory(PeDirectory.Export);

        if (rva == 0)
        {
            return names;
        }

        var ordinalBase = image.U32At(rva + OrdinalBaseField);
        var functionCount = image.U32At(rva + FunctionCountField);
        var nameCount = image.U32At(rva + NameCountField);

        var functions = image.U32At(rva + FunctionTableField);
        var nameTable = image.U32At(rva + NameTableField);
        var ordinals = image.U32At(rva + OrdinalTableField);
        var named = new HashSet<uint>();

        for (uint index = 0; index < nameCount; index++)
        {
            names.Add(image.AsciiAt(image.U32At(nameTable + (index * sizeof(uint)))) ?? string.Empty);
            named.Add(image.U32At(ordinals + (index * sizeof(ushort))) & ushort.MaxValue);
        }

        for (uint index = 0; index < functionCount; index++)
        {
            if (!named.Contains(index) && image.U32At(functions + (index * sizeof(uint))) != 0)
            {
                names.Add("#" + (index + ordinalBase).ToString(CultureInfo.InvariantCulture));
            }
        }

        return names;
    }

    /// <summary>The PDB paths of the CodeView debug entries (RSDS or NB10).</summary>
    public static IReadOnlyList<string> PdbPaths(PeImage image)
    {
        var paths = new List<string>();
        var (rva, size) = image.Directory(PeDirectory.Debug);
        var start = rva == 0 ? null : image.Offset(rva);

        if (!start.HasValue)
        {
            return paths;
        }

        for (var entry = 0; entry + DebugEntrySize <= size; entry += DebugEntrySize)
        {
            var at = (int)start.Value + entry;

            if (image.U32(at + DebugTypeField) != CodeViewType)
            {
                continue;
            }

            var data = image.Data.AsSpan((int)image.U32(at + DebugPointerField), (int)image.U32(at + DebugSizeField));
            var path = CodeViewPath(data);

            if (path is not null)
            {
                paths.Add(path);
            }
        }

        return paths;
    }

    /// <summary>The top-level resource types: a name, or the type id.</summary>
    public static IReadOnlyList<object> ResourceTypes(PeImage image)
    {
        var types = new List<object>();
        var (rva, _) = image.Directory(PeDirectory.Resource);
        var start = rva == 0 ? null : image.Offset(rva);

        if (!start.HasValue)
        {
            return types;
        }

        var root = (int)start.Value;
        var count = image.U16(root + NamedCountField) + image.U16(root + IdCountField);

        for (var index = 0; index < count; index++)
        {
            var name = image.U32(root + ResourceEntriesOffset + (index * ResourceEntrySize));

            if ((name & NameIsString) == 0)
            {
                types.Add(name);

                continue;
            }

            var at = root + (int)(name & ~NameIsString);
            var length = image.U16(at);
            types.Add(Encoding.Unicode.GetString(image.Data.AsSpan(at + sizeof(ushort), length * sizeof(char))));
        }

        return types;
    }

    private static string? CodeViewPath(ReadOnlySpan<byte> data)
    {
        var nameOffset = 0;

        if (data.StartsWith("RSDS"u8))
        {
            nameOffset = RsdsNameOffset;
        }
        else if (data.StartsWith("NB10"u8))
        {
            nameOffset = Nb10NameOffset;
        }

        if (nameOffset == 0 || data.Length < nameOffset)
        {
            return null;
        }

        var name = data[nameOffset..].TrimEnd((byte)0);

        return Encoding.UTF8.GetString(name);
    }
}
