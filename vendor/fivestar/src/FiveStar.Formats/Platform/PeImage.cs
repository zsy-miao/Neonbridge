// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Formats.Platform;

/// <summary>
/// The headers of a PE image (layout from the Microsoft PE/COFF specification): file header, optional header
/// (PE32 or PE32+), data directories and section table, with RVA-to-offset mapping.
/// </summary>
public sealed class PeImage
{
    // DOS header: e_lfanew at 0x3C. COFF file header follows the 4-byte PE signature.
    private const int NewHeaderPointer = 0x3C;
    private const int SignatureSize = 4;
    private const int FileHeaderSize = 20;
    private const int SectionCountField = 2;
    private const int TimeStampField = 4;
    private const int OptionalHeaderSizeField = 16;

    // Optional header fields (PE32 / PE32+).
    private const ushort Pe32PlusMagic = 0x20B;
    private const int EntryPointField = 16;
    private const int ImageBase32Field = 28;
    private const int ImageBase64Field = 24;
    private const int FileAlignmentField = 36;
    private const int SubsystemField = 68;
    private const int DllCharacteristicsField = 70;
    private const int DirectoryCount32Field = 92;
    private const int DirectoryCount64Field = 108;
    private const int Directories32Field = 96;
    private const int Directories64Field = 112;
    private const int DirectorySize = 8;

    private const int SectionHeaderSize = 40;
    private const int SectionNameSize = 8;
    private const int VirtualSizeField = 8;
    private const int VirtualAddressField = 12;
    private const int RawSizeField = 16;
    private const int RawOffsetField = 20;

    // Raw offsets are rounded down to 512 bytes when the file alignment is at least that (as the loader does).
    private const uint MinFileAlignment = 0x200;

    private readonly byte[] _data;
    private readonly int _directories;
    private readonly int _directoryCount;

    public PeImage(byte[] data)
    {
        _data = data;
        var header = (int)BinaryPrimitives.ReadUInt32LittleEndian(data.AsSpan(NewHeaderPointer));

        if (!data.AsSpan(header).StartsWith("PE\0\0"u8))
        {
            throw new InvalidDataException("no PE signature");
        }

        // COFF file header.
        var file = header + SignatureSize;
        Machine = U16(file);
        TimeStamp = U32(file + TimeStampField);

        // Optional header: PE32+ widens the image base and moves the data directories.
        var optional = file + FileHeaderSize;
        Is64Bit = U16(optional) == Pe32PlusMagic;
        EntryPoint = U32(optional + EntryPointField);
        ImageBase = U32(optional + ImageBase32Field);

        FileAlignment = U32(optional + FileAlignmentField);
        Subsystem = U16(optional + SubsystemField);
        DllCharacteristics = U16(optional + DllCharacteristicsField);

        var countField = DirectoryCount32Field;
        _directories = optional + Directories32Field;

        if (Is64Bit)
        {
            ImageBase = BinaryPrimitives.ReadUInt64LittleEndian(data.AsSpan(optional + ImageBase64Field));
            countField = DirectoryCount64Field;
            _directories = optional + Directories64Field;
        }

        _directoryCount = (int)U32(optional + countField);
        Sections = ReadSections(optional + U16(file + OptionalHeaderSizeField), U16(file + SectionCountField));
    }

    public ushort Machine { get; }

    public uint TimeStamp { get; }

    public uint EntryPoint { get; }

    public ulong ImageBase { get; }

    public uint FileAlignment { get; }

    public ushort Subsystem { get; }

    public ushort DllCharacteristics { get; }

    public bool Is64Bit { get; }

    public IReadOnlyList<PeSection> Sections { get; }

    public byte[] Data => _data;

    /// <summary>The RVA and size of a data directory (0, 0 when absent).</summary>
    public (uint Rva, uint Size) Directory(PeDirectory directory)
    {
        var index = (int)directory;

        if (index >= _directoryCount)
        {
            return (0, 0);
        }

        var at = _directories + (index * DirectorySize);

        return (U32(at), U32(at + sizeof(uint)));
    }

    /// <summary>The file offset of an RVA, or null when no section (or the headers) holds it.</summary>
    public long? Offset(uint rva)
    {
        foreach (var section in Sections)
        {
            var size = Math.Max(section.VirtualSize, section.RawSize);

            if (rva >= section.VirtualAddress && rva < section.VirtualAddress + size)
            {
                return RawStart(section) + (rva - section.VirtualAddress);
            }
        }

        if (Sections.Count == 0 || rva < Sections[0].RawOffset)
        {
            return rva;
        }

        return null;
    }

    /// <summary>The raw start of a section, aligned as the loader aligns it.</summary>
    public long RawStart(PeSection section)
    {
        if (FileAlignment < MinFileAlignment)
        {
            return section.RawOffset;
        }

        return section.RawOffset / MinFileAlignment * MinFileAlignment;
    }

    /// <summary>The NUL-terminated ASCII string at an RVA, or null when the RVA is outside the file.</summary>
    public string? AsciiAt(uint rva)
    {
        var offset = Offset(rva);

        if (!offset.HasValue || offset.Value >= _data.Length)
        {
            return null;
        }

        var span = _data.AsSpan((int)offset.Value);
        var end = span.IndexOf((byte)0);

        if (end < 0)
        {
            end = span.Length;
        }

        return Encoding.Latin1.GetString(span[..end]);
    }

    public uint U32At(uint rva)
    {
        var offset = Offset(rva) ?? throw new InvalidDataException("RVA outside the image");

        return U32((int)offset);
    }

    public ulong U64At(uint rva)
    {
        var offset = Offset(rva) ?? throw new InvalidDataException("RVA outside the image");

        return BinaryPrimitives.ReadUInt64LittleEndian(_data.AsSpan((int)offset));
    }

    public ushort U16(int offset)
    {
        return BinaryPrimitives.ReadUInt16LittleEndian(_data.AsSpan(offset));
    }

    public uint U32(int offset)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(_data.AsSpan(offset));
    }

    private List<PeSection> ReadSections(int table, int count)
    {
        var sections = new List<PeSection>(count);

        for (var index = 0; index < count; index++)
        {
            var at = table + (index * SectionHeaderSize);
            var name = _data.AsSpan(at, SectionNameSize).TrimEnd((byte)0);
            sections.Add(new PeSection(
                Encoding.UTF8.GetString(name),
                U32(at + VirtualSizeField),
                U32(at + VirtualAddressField),
                U32(at + RawSizeField),
                U32(at + RawOffsetField)));
        }

        return sections;
    }
}
