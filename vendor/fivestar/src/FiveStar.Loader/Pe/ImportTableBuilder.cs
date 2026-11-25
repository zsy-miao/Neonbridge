// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Loader.Pe;

/// <summary>
/// Writes an import table (imports by name) for a block that will sit at a given RVA: the descriptor array, then per
/// module its lookup table and IAT, then the hint/name entries, then the module names.
/// </summary>
internal sealed class ImportTableBuilder
{
    /// <summary>Each module has an import lookup table and an IAT of the same size.</summary>
    private const int TablesPerModule = 2;

    private readonly List<(string Module, List<string> Symbols)> _modules = [];

    public void Add(string module, string symbol)
    {
        foreach (var (name, symbols) in _modules)
        {
            if (string.Equals(name, module, StringComparison.OrdinalIgnoreCase))
            {
                symbols.Add(symbol);

                return;
            }
        }

        _modules.Add((module, [symbol]));
    }

    public ImportTableImage Build(uint baseRva)
    {
        var bytes = new List<byte>();
        int descriptorsSize = (_modules.Count + 1) * ImportFormat.DescriptorSize;
        bytes.AddRange(new byte[descriptorsSize]);
        var slots = new Dictionary<string, uint>(StringComparer.OrdinalIgnoreCase);
        var thunkTables = new List<(int Lookup, int Iat)>();

        // Lookup tables and IATs, zero-terminated, filled in once the names are placed.
        foreach (var (_, symbols) in _modules)
        {
            int tableSize = (symbols.Count + 1) * ImportFormat.ThunkSize;
            thunkTables.Add((bytes.Count, bytes.Count + tableSize));
            bytes.AddRange(new byte[tableSize * TablesPerModule]);
        }

        var buffer = WriteNames(bytes, thunkTables, baseRva, slots);

        return new ImportTableImage(buffer, baseRva, (uint)descriptorsSize, slots);
    }

    private static uint AppendString(List<byte> bytes, string text, int prefixSize, uint baseRva)
    {
        // A hint/name entry starts on an even address.
        if (prefixSize != 0 && bytes.Count % ImportFormat.HintSize != 0)
        {
            bytes.Add(0);
        }

        uint rva = baseRva + (uint)bytes.Count;
        bytes.AddRange(new byte[prefixSize]);
        bytes.AddRange(Encoding.ASCII.GetBytes(text));
        bytes.Add(0);

        return rva;
    }

    private byte[] WriteNames(List<byte> bytes, List<(int Lookup, int Iat)> thunkTables, uint baseRva, Dictionary<string, uint> slots)
    {
        var thunks = new List<(int Offset, ulong Value)>();
        var descriptors = new List<(int Index, uint Lookup, uint Name, uint Iat)>();

        for (int m = 0; m < _modules.Count; m++)
        {
            var (module, symbols) = _modules[m];
            var (lookup, iat) = thunkTables[m];

            for (int s = 0; s < symbols.Count; s++)
            {
                uint hintName = AppendString(bytes, symbols[s], ImportFormat.HintSize, baseRva);
                thunks.Add((lookup + (s * ImportFormat.ThunkSize), hintName));
                thunks.Add((iat + (s * ImportFormat.ThunkSize), hintName));
                slots[$"{module}!{symbols[s]}"] = baseRva + (uint)(iat + (s * ImportFormat.ThunkSize));
            }

            uint nameRva = AppendString(bytes, module, 0, baseRva);
            descriptors.Add((m, baseRva + (uint)lookup, nameRva, baseRva + (uint)iat));
        }

        var buffer = bytes.ToArray();
        var span = buffer.AsSpan();

        foreach (var (offset, value) in thunks)
        {
            BinaryPrimitives.WriteUInt64LittleEndian(span[offset..], value);
        }

        foreach (var (index, lookup, name, iat) in descriptors)
        {
            int descriptor = index * ImportFormat.DescriptorSize;
            BinaryPrimitives.WriteUInt32LittleEndian(span[(descriptor + ImportFormat.OriginalFirstThunkOffset)..], lookup);
            BinaryPrimitives.WriteUInt32LittleEndian(span[(descriptor + ImportFormat.NameOffset)..], name);
            BinaryPrimitives.WriteUInt32LittleEndian(span[(descriptor + ImportFormat.FirstThunkOffset)..], iat);
        }

        return buffer;
    }
}
