// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Rpf;

/// <summary>The plain 0x40-byte header of an RPF9 archive, read as sixteen little-endian u32 words.</summary>
/// <param name="Words">The sixteen header words, word <c>k</c> at offset <c>4 * k</c>.</param>
public sealed record RpfHeader(uint[] Words)
{
    /// <summary>Bytes of the plain header.</summary>
    public const int Size = 0x40;

    /// <summary>Header words.</summary>
    public const int WordCount = Size / sizeof(uint);

    // Word index of each field (offset / 4).
    private const int HeaderEndWord = 1;
    private const int TocSizeWord = 2;
    private const int NameTableSizeWord = 3;
    private const int Unknown20Word = 8;
    private const int EntryCountWord = 9;
    private const int KeySelectorWord = 10;
    private const int VersionWord = 11;

    /// <summary>The magic at +0x00.</summary>
    public static ReadOnlySpan<byte> Magic => "RPF9"u8;

    /// <summary>Gets +0x04: the end of the header area; data starts here or at the next 4 KiB.</summary>
    public uint HeaderEnd => Words[HeaderEndWord];

    /// <summary>Gets +0x08: size of the encrypted table of contents.</summary>
    public uint TocSize => Words[TocSizeWord];

    /// <summary>Gets +0x0C: size of the name table.</summary>
    public uint NameTableSize => Words[NameTableSizeWord];

    /// <summary>Gets +0x20: low 16 bits hold a small value (3 to 7) of unknown meaning.</summary>
    public uint Unknown20 => Words[Unknown20Word];

    /// <summary>Gets +0x24: number of entries.</summary>
    public uint EntryCount => Words[EntryCountWord];

    /// <summary>Gets +0x28: selects the key of the table of contents.</summary>
    public uint KeySelector => Words[KeySelectorWord];

    /// <summary>Gets +0x2C: 0x00090F01 or 0x00090F02.</summary>
    public uint Version => Words[VersionWord];

    public static RpfHeader Read(ReadOnlySpan<byte> data)
    {
        var words = new uint[WordCount];

        for (var index = 0; index < WordCount; index++)
        {
            words[index] = BinaryPrimitives.ReadUInt32LittleEndian(data[(index * sizeof(uint))..]);
        }

        return new RpfHeader(words);
    }
}
