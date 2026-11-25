// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>The export directory of a mapped image, read once after relocation (so import cycles resolve).</summary>
internal sealed unsafe class ExportTable
{
    private readonly nint _base;
    private readonly DataDirectory _directory;
    private readonly Dictionary<string, uint> _ordinalsByName = new(StringComparer.Ordinal);
    private readonly Dictionary<uint, uint> _rvasByOrdinal = [];

    public ExportTable(nint imageBase, DataDirectory directory)
    {
        _base = imageBase;
        _directory = directory;

        if (directory.IsPresent)
        {
            Read();
        }
    }

    /// <summary>Every export in ordinal order, with its first name.</summary>
    public IReadOnlyList<ExportEntry> Entries()
    {
        var names = new Dictionary<uint, string>();

        foreach (var (name, ordinal) in _ordinalsByName)
        {
            names.TryAdd(ordinal, name);
        }

        var entries = new List<ExportEntry>();

        foreach (var (ordinal, rva) in _rvasByOrdinal.OrderBy(pair => pair.Key))
        {
            names.TryGetValue(ordinal, out var name);
            entries.Add(new ExportEntry(ordinal, name, rva, ForwarderAt(rva)));
        }

        return entries;
    }

    /// <summary>Looks an export up by name, or by ordinal when <paramref name="name"/> is null.</summary>
    public ExportTarget? Find(string? name, uint ordinal)
    {
        if (name is not null && !_ordinalsByName.TryGetValue(name, out ordinal))
        {
            return null;
        }

        if (!_rvasByOrdinal.TryGetValue(ordinal, out var rva))
        {
            return null;
        }

        string? forwarder = ForwarderAt(rva);

        if (forwarder is not null)
        {
            return new ExportTarget(0, forwarder);
        }

        return new ExportTarget(_base + (nint)rva, null);
    }

    public static string ReadAnsi(nint address)
    {
        return Marshal.PtrToStringAnsi(address) ?? string.Empty;
    }

    private void Read()
    {
        byte* directory = (byte*)(_base + (nint)_directory.Rva);
        uint ordinalBase = *(uint*)(directory + ExportFormat.OrdinalBaseOffset);
        uint functionCount = *(uint*)(directory + ExportFormat.NumberOfFunctionsOffset);
        uint nameCount = *(uint*)(directory + ExportFormat.NumberOfNamesOffset);
        uint* functions = (uint*)(_base + *(uint*)(directory + ExportFormat.AddressOfFunctionsOffset));
        uint* names = (uint*)(_base + *(uint*)(directory + ExportFormat.AddressOfNamesOffset));

        ushort* nameOrdinals = (ushort*)(_base + *(uint*)(directory + ExportFormat.AddressOfNameOrdinalsOffset));

        for (uint i = 0; i < functionCount; i++)
        {
            if (functions[i] != 0)
            {
                _rvasByOrdinal[ordinalBase + i] = functions[i];
            }
        }

        for (uint i = 0; i < nameCount; i++)
        {
            _ordinalsByName[ReadAnsi(_base + (nint)names[i])] = ordinalBase + nameOrdinals[i];
        }
    }

    /// <summary>An export RVA inside the export directory is a forwarder string.</summary>
    private string? ForwarderAt(uint rva)
    {
        bool inside = rva >= _directory.Rva && rva < _directory.Rva + _directory.Size;

        if (!inside)
        {
            return null;
        }

        return ReadAnsi(_base + (nint)rva);
    }
}
