// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Tests.Support;

/// <summary>Writes an export directory (ordinal base 1, names sorted) for a block placed at a given RVA.</summary>
internal static class ExportTableWriter
{
    public static byte[] Write(string dllName, IReadOnlyList<(string Name, uint Rva)> exports, uint baseRva)
    {
        var sorted = exports.OrderBy(e => e.Name, StringComparer.Ordinal).ToList();
        int count = sorted.Count;
        int functions = ExportFormat.DirectorySize;
        int names = functions + (4 * count);
        int ordinals = names + (4 * count);
        int strings = ordinals + (2 * count);

        var bytes = new List<byte>(new byte[strings]);
        var span = new byte[strings];

        for (int i = 0; i < count; i++)
        {
            BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(functions + (4 * i)), sorted[i].Rva);
            BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(names + (4 * i)), baseRva + (uint)bytes.Count);
            BinaryPrimitives.WriteUInt16LittleEndian(span.AsSpan(ordinals + (2 * i)), (ushort)i);
            bytes.AddRange(Encoding.ASCII.GetBytes(sorted[i].Name + "\0"));
        }

        uint dllNameRva = baseRva + (uint)bytes.Count;
        bytes.AddRange(Encoding.ASCII.GetBytes(dllName + "\0"));
        BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(ExportFormat.NameOffset), dllNameRva);
        BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(ExportFormat.OrdinalBaseOffset), 1);
        BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(ExportFormat.NumberOfFunctionsOffset), (uint)count);
        BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(ExportFormat.NumberOfNamesOffset), (uint)count);

        BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(ExportFormat.AddressOfFunctionsOffset), baseRva + (uint)functions);
        BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(ExportFormat.AddressOfNamesOffset), baseRva + (uint)names);
        BinaryPrimitives.WriteUInt32LittleEndian(span.AsSpan(ExportFormat.AddressOfNameOrdinalsOffset), baseRva + (uint)ordinals);

        var result = bytes.ToArray();
        span.CopyTo(result, 0);

        return result;
    }
}
