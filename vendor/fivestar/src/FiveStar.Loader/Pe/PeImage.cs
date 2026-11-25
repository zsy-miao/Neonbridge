// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Loader.Pe;

/// <summary>The header-level view of a PE32+ (AMD64) file on disk: what the loader needs to map it.</summary>
internal sealed record PeImage(
    byte[] Data,
    ushort Characteristics,
    ulong ImageBase,
    uint SizeOfImage,
    uint SizeOfHeaders,
    uint EntryPointRva,
    uint SectionAlignment,
    IReadOnlyList<DataDirectory> Directories,
    IReadOnlyList<PeSection> Sections)
{
    public bool IsDll => (Characteristics & PeFormat.FileDll) != 0;

    public bool RelocationsStripped => (Characteristics & PeFormat.FileRelocsStripped) != 0;

    public DataDirectory Directory(DataDirectoryIndex index)
    {
        int i = (int)index;

        if (i >= Directories.Count)
        {
            return default;
        }

        return Directories[i];
    }

    /// <summary>Parses the headers of <paramref name="data"/>; throws <see cref="BadImageFormatException"/> for anything but PE32+ AMD64.</summary>
    public static PeImage Parse(byte[] data)
    {
        int ntHeader = CheckSignatures(data);
        var fileHeader = data.AsSpan(ntHeader + PeFormat.NtSignatureSize);
        ushort machine = U16(fileHeader, PeFormat.MachineOffset);

        if (machine != PeFormat.MachineAmd64)
        {
            throw new BadImageFormatException($"machine 0x{machine:x} is not AMD64");
        }

        int optionalOffset = ntHeader + PeFormat.NtSignatureSize + PeFormat.FileHeaderSize;
        ushort optionalSize = U16(fileHeader, PeFormat.SizeOfOptionalHeaderOffset);
        var optional = data.AsSpan(optionalOffset, optionalSize);

        if (U16(optional, PeFormat.MagicOffset) != PeFormat.Pe32PlusMagic)
        {
            throw new BadImageFormatException("not PE32+");
        }

        var sections = ReadSections(data, optionalOffset + optionalSize, U16(fileHeader, PeFormat.NumberOfSectionsOffset));

        return new PeImage(
            data,
            U16(fileHeader, PeFormat.CharacteristicsOffset),
            BinaryPrimitives.ReadUInt64LittleEndian(optional[PeFormat.ImageBaseOffset..]),
            U32(optional, PeFormat.SizeOfImageOffset),
            U32(optional, PeFormat.SizeOfHeadersOffset),
            U32(optional, PeFormat.AddressOfEntryPointOffset),
            U32(optional, PeFormat.SectionAlignmentOffset),
            ReadDirectories(optional),
            sections);
    }

    private static int CheckSignatures(byte[] data)
    {
        if (data.Length < PeFormat.DosHeaderSize || BinaryPrimitives.ReadUInt16LittleEndian(data) != PeFormat.DosSignature)
        {
            throw new BadImageFormatException("no MZ header");
        }

        int ntHeader = BinaryPrimitives.ReadInt32LittleEndian(data.AsSpan(PeFormat.NewHeaderPointerOffset));
        bool fits = ntHeader > 0 && ntHeader + PeFormat.NtSignatureSize + PeFormat.FileHeaderSize <= data.Length;

        if (!fits || BinaryPrimitives.ReadUInt32LittleEndian(data.AsSpan(ntHeader)) != PeFormat.NtSignature)
        {
            throw new BadImageFormatException("no PE signature");
        }

        return ntHeader;
    }

    private static DataDirectory[] ReadDirectories(ReadOnlySpan<byte> optional)
    {
        uint count = Math.Min(U32(optional, PeFormat.NumberOfRvaAndSizesOffset), PeFormat.MaxDataDirectories);
        var directories = new DataDirectory[count];

        for (int i = 0; i < count; i++)
        {
            int offset = PeFormat.DataDirectoriesOffset + (i * PeFormat.DataDirectorySize);
            directories[i] = new DataDirectory(U32(optional, offset), U32(optional, offset + sizeof(uint)));
        }

        return directories;
    }

    private static PeSection[] ReadSections(byte[] data, int tableOffset, int count)
    {
        var sections = new PeSection[count];

        for (int i = 0; i < count; i++)
        {
            var header = data.AsSpan(tableOffset + (i * PeFormat.SectionHeaderSize), PeFormat.SectionHeaderSize);
            string name = Encoding.ASCII.GetString(header[..PeFormat.SectionNameSize]).TrimEnd('\0');

            sections[i] = new PeSection(
                name,
                U32(header, PeFormat.SectionVirtualAddressOffset),
                U32(header, PeFormat.SectionVirtualSizeOffset),
                U32(header, PeFormat.SectionPointerToRawDataOffset),
                U32(header, PeFormat.SectionSizeOfRawDataOffset),
                (SectionCharacteristics)U32(header, PeFormat.SectionCharacteristicsOffset));
        }

        return sections;
    }

    private static ushort U16(ReadOnlySpan<byte> span, int offset)
    {
        return BinaryPrimitives.ReadUInt16LittleEndian(span[offset..]);
    }

    private static uint U32(ReadOnlySpan<byte> span, int offset)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(span[offset..]);
    }
}
