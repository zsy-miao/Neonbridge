// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Loader.Pe;

/// <summary>
/// Writes a minimal PE32+ AMD64 image file: DOS header, COFF and optional headers, section table and section bytes.
/// Sections are added in ascending RVA order; the loader uses it for generated TLS host DLLs.
/// </summary>
internal sealed class PeImageBuilder
{
    /// <summary>File and section alignment of the images it writes.</summary>
    public const int FileAlignment = 0x200;
    public const int SectionAlignment = 0x1000;

    private const byte LinkerVersion = 14;
    private const ushort OperatingSystemVersion = 6;
    private const ulong StackReserve = 0x100000;
    private const ulong StackCommit = 0x1000;
    private const ulong HeapReserve = 0x100000;
    private const ulong HeapCommit = 0x1000;
    private const ushort DllCharacteristics = PeFormat.DllHighEntropyVa | PeFormat.DllDynamicBase | PeFormat.DllNxCompat;
    private const int OptionalHeaderOffset = PeFormat.DosHeaderSize + PeFormat.NtSignatureSize + PeFormat.FileHeaderSize;
    private const int SectionTableOffset = OptionalHeaderOffset + PeFormat.OptionalHeaderSize;

    private readonly ulong _imageBase;
    private readonly ushort _characteristics;
    private readonly List<PeSectionContent> _sections = [];
    private readonly DataDirectory[] _directories = new DataDirectory[PeFormat.MaxDataDirectories];
    private uint _entryPointRva;

    public PeImageBuilder(ulong imageBase, ushort characteristics)
    {
        _imageBase = imageBase;
        _characteristics = characteristics;
    }

    public void SetEntryPoint(uint rva)
    {
        _entryPointRva = rva;
    }

    public void AddSection(PeSectionContent section)
    {
        _sections.Add(section);
    }

    public void SetDirectory(DataDirectoryIndex index, uint rva, uint size)
    {
        _directories[(int)index] = new DataDirectory(rva, size);
    }

    public byte[] Build()
    {
        int headersSize = Align(SectionTableOffset + (_sections.Count * PeFormat.SectionHeaderSize), FileAlignment);
        var rawPointers = new int[_sections.Count];
        int end = headersSize;

        for (int i = 0; i < _sections.Count; i++)
        {
            rawPointers[i] = end;
            end += Align(_sections[i].Content.Length, FileAlignment);
        }

        var file = new byte[end];
        WriteFileHeader(file);
        WriteOptionalHeader(file, headersSize);
        WriteSections(file, rawPointers);

        return file;
    }

    private static int Align(int value, int alignment)
    {
        return (value + alignment - 1) & ~(alignment - 1);
    }

    private void WriteFileHeader(byte[] file)
    {
        var span = file.AsSpan();
        int fileHeader = PeFormat.DosHeaderSize + PeFormat.NtSignatureSize;

        BinaryPrimitives.WriteUInt16LittleEndian(span, PeFormat.DosSignature);
        BinaryPrimitives.WriteInt32LittleEndian(span[PeFormat.NewHeaderPointerOffset..], PeFormat.DosHeaderSize);
        BinaryPrimitives.WriteUInt32LittleEndian(span[PeFormat.DosHeaderSize..], PeFormat.NtSignature);
        BinaryPrimitives.WriteUInt16LittleEndian(span[(fileHeader + PeFormat.MachineOffset)..], PeFormat.MachineAmd64);
        BinaryPrimitives.WriteUInt16LittleEndian(span[(fileHeader + PeFormat.NumberOfSectionsOffset)..], (ushort)_sections.Count);
        BinaryPrimitives.WriteUInt16LittleEndian(span[(fileHeader + PeFormat.SizeOfOptionalHeaderOffset)..], PeFormat.OptionalHeaderSize);

        BinaryPrimitives.WriteUInt16LittleEndian(span[(fileHeader + PeFormat.CharacteristicsOffset)..], _characteristics);
    }

    private void WriteOptionalHeader(byte[] file, int headersSize)
    {
        var header = file.AsSpan(OptionalHeaderOffset, PeFormat.OptionalHeaderSize);

        // Identification and sizes.
        BinaryPrimitives.WriteUInt16LittleEndian(header, PeFormat.Pe32PlusMagic);
        header[PeFormat.MajorLinkerVersionOffset] = LinkerVersion;
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SizeOfCodeOffset..], RawSize(true));
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SizeOfInitializedDataOffset..], RawSize(false));
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.AddressOfEntryPointOffset..], _entryPointRva);
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.BaseOfCodeOffset..], BaseOfCode());

        // Layout.
        BinaryPrimitives.WriteUInt64LittleEndian(header[PeFormat.ImageBaseOffset..], _imageBase);
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SectionAlignmentOffset..], SectionAlignment);
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.FileAlignmentOffset..], FileAlignment);
        BinaryPrimitives.WriteUInt16LittleEndian(header[PeFormat.MajorOperatingSystemVersionOffset..], OperatingSystemVersion);
        BinaryPrimitives.WriteUInt16LittleEndian(header[PeFormat.MajorSubsystemVersionOffset..], OperatingSystemVersion);
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SizeOfImageOffset..], SizeOfImage());

        // Process defaults.
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SizeOfHeadersOffset..], (uint)headersSize);
        BinaryPrimitives.WriteUInt16LittleEndian(header[PeFormat.SubsystemOffset..], PeFormat.SubsystemWindowsGui);
        BinaryPrimitives.WriteUInt16LittleEndian(header[PeFormat.DllCharacteristicsOffset..], DllCharacteristics);
        BinaryPrimitives.WriteUInt64LittleEndian(header[PeFormat.SizeOfStackReserveOffset..], StackReserve);
        BinaryPrimitives.WriteUInt64LittleEndian(header[PeFormat.SizeOfStackCommitOffset..], StackCommit);
        BinaryPrimitives.WriteUInt64LittleEndian(header[PeFormat.SizeOfHeapReserveOffset..], HeapReserve);

        BinaryPrimitives.WriteUInt64LittleEndian(header[PeFormat.SizeOfHeapCommitOffset..], HeapCommit);
        BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.NumberOfRvaAndSizesOffset..], PeFormat.MaxDataDirectories);
        WriteDirectories(header);
    }

    private void WriteDirectories(Span<byte> header)
    {
        for (int i = 0; i < _directories.Length; i++)
        {
            int offset = PeFormat.DataDirectoriesOffset + (i * PeFormat.DataDirectorySize);
            BinaryPrimitives.WriteUInt32LittleEndian(header[offset..], _directories[i].Rva);
            BinaryPrimitives.WriteUInt32LittleEndian(header[(offset + sizeof(uint))..], _directories[i].Size);
        }
    }

    private void WriteSections(byte[] file, int[] rawPointers)
    {
        for (int i = 0; i < _sections.Count; i++)
        {
            var section = _sections[i];
            var header = file.AsSpan(SectionTableOffset + (i * PeFormat.SectionHeaderSize), PeFormat.SectionHeaderSize);

            Encoding.ASCII.GetBytes(section.Name).CopyTo(header);
            BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SectionVirtualSizeOffset..], (uint)section.Content.Length);
            BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SectionVirtualAddressOffset..], section.Rva);
            BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SectionSizeOfRawDataOffset..], (uint)Align(section.Content.Length, FileAlignment));
            BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SectionPointerToRawDataOffset..], (uint)rawPointers[i]);
            BinaryPrimitives.WriteUInt32LittleEndian(header[PeFormat.SectionCharacteristicsOffset..], (uint)section.Characteristics);

            section.Content.CopyTo(file, rawPointers[i]);
        }
    }

    private uint RawSize(bool code)
    {
        uint total = 0;

        foreach (var section in _sections)
        {
            bool isCode = (section.Characteristics & SectionCharacteristics.ContainsCode) != 0;

            if (isCode == code)
            {
                total += (uint)Align(section.Content.Length, FileAlignment);
            }
        }

        return total;
    }

    private uint BaseOfCode()
    {
        foreach (var section in _sections)
        {
            if ((section.Characteristics & SectionCharacteristics.ContainsCode) != 0)
            {
                return section.Rva;
            }
        }

        return 0;
    }

    private uint SizeOfImage()
    {
        if (_sections.Count == 0)
        {
            return SectionAlignment;
        }

        var last = _sections[^1];

        return last.Rva + (uint)Align(Math.Max(last.Content.Length, 1), SectionAlignment);
    }
}
