// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Tests.Assembly;

/// <summary>Reads the <c>.text</c> section of the ELF64 object llvm-mc writes.</summary>
public static class ElfText
{
    // ELF64 header: e_shoff at 0x28, e_shentsize / e_shnum / e_shstrndx at 0x3A; section header: sh_name at 0,
    // sh_offset at 0x18, sh_size at 0x20.
    private const int SectionTableField = 0x28;
    private const int SectionEntrySizeField = 0x3A;
    private const int SectionCountField = 0x3C;
    private const int NameTableIndexField = 0x3E;
    private const int OffsetField = 0x18;
    private const int SizeField = 0x20;

    public static byte[] Read(byte[] elf)
    {
        var table = (int)BinaryPrimitives.ReadUInt64LittleEndian(elf.AsSpan(SectionTableField));
        var entrySize = BinaryPrimitives.ReadUInt16LittleEndian(elf.AsSpan(SectionEntrySizeField));
        var count = BinaryPrimitives.ReadUInt16LittleEndian(elf.AsSpan(SectionCountField));
        var namesIndex = BinaryPrimitives.ReadUInt16LittleEndian(elf.AsSpan(NameTableIndexField));
        var names = (int)BinaryPrimitives.ReadUInt64LittleEndian(elf.AsSpan(table + (namesIndex * entrySize) + OffsetField));

        for (var section = 0; section < count; section++)
        {
            var header = table + (section * entrySize);
            var name = (int)BinaryPrimitives.ReadUInt32LittleEndian(elf.AsSpan(header));

            if (Name(elf, names + name) != ".text")
            {
                continue;
            }

            var offset = (int)BinaryPrimitives.ReadUInt64LittleEndian(elf.AsSpan(header + OffsetField));
            var size = (int)BinaryPrimitives.ReadUInt64LittleEndian(elf.AsSpan(header + SizeField));

            return elf.AsSpan(offset, size).ToArray();
        }

        throw new InvalidDataException("no .text section in the llvm-mc object");
    }

    private static string Name(byte[] elf, int start)
    {
        var end = Array.IndexOf(elf, (byte)0, start);

        return Encoding.ASCII.GetString(elf, start, end - start);
    }
}
