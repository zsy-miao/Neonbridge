// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// A streamed texture page file (<c>.ttpf</c>): an <c>RDS5</c> header and page table mapping file pages to a virtual
/// address space, then an <c>ETPF</c> section of texture descriptors and name hashes. Pages decode on demand
/// (stored, zlib, or BCPack BC1, BC4 and BC5); pages of the other BCPack codecs read as missing.
/// </summary>
public sealed class TexturePageFile
{
    // RDS5 header: magic, u32 entry count (one more than the real pages: a trailing sentinel slot), four u32.
    private const int HeaderSize = 0x18;
    private const int CountField = 4;
    private const int EntrySize = 8;
    private const int OffsetMask = 0xFFFFFF;
    private const int FileOffsetShift = 8;
    private const int FlagShift = 24;
    private const uint VirtualPageMask = 0xFFFFF;
    private const long VirtualPageSize = 0x4000;
    private const int SubPageShift = 6;
    private const int SubPageMask = 3;
    private const long SubPageSize = 0x1000;
    private const int TagShift = 20;

    // ETPF: starts at the first 256-byte boundary after the real entries; magic, u32 634, u32 count, u32 size.
    private const int EtpfAlignment = 0x100;
    private const int EtpfHeaderSize = 16;
    private const int DescriptorSize = 16;
    private const int CountOffset = 8;

    private const int MaxCachedPages = 64;

    private readonly IByteSource _source;
    private readonly ZlibInflater _inflater;
    private readonly Dictionary<int, byte[]?> _cache = [];
    private readonly long[] _addresses;

    public TexturePageFile(IByteSource source, ZlibInflater inflater)
    {
        _source = source;
        _inflater = inflater;
        var header = source.Read(0, HeaderSize);

        if (header.Length < HeaderSize || !header.StartsWith(Magic))
        {
            throw new InvalidDataException("not an RDS5 texture page file");
        }

        var entries = (int)BinaryPrimitives.ReadUInt32LittleEndian(header[CountField..]) - 1;
        Pages = ReadPages(Math.Max(entries, 0));
        _addresses = new long[Pages.Count];

        for (var index = 0; index < Pages.Count; index++)
        {
            _addresses[index] = Pages[index].VirtualAddress;
        }

        EtpfOffset = (HeaderSize + ((long)EntrySize * entries) + EtpfAlignment - 1) & ~(long)(EtpfAlignment - 1);
        (Descriptors, Hashes) = ReadEtpf();
    }

    public static ReadOnlySpan<byte> Magic => "RDS5"u8;

    public static ReadOnlySpan<byte> EtpfMagic => "ETPF"u8;

    /// <summary>Gets the pages that have stored bytes, in page-table order.</summary>
    public IReadOnlyList<TexturePage> Pages { get; }

    public IReadOnlyList<TextureDescriptor> Descriptors { get; }

    /// <summary>Gets the 64-bit name hash of each descriptor.</summary>
    public IReadOnlyList<ulong> Hashes { get; }

    public long EtpfOffset { get; }

    /// <summary>The index of the last page whose address is at most <paramref name="address"/> (binary search), or -1.</summary>
    public int PageAt(long address)
    {
        var low = 0;
        var high = _addresses.Length;

        while (low < high)
        {
            var middle = (low + high) >> 1;

            if (address < _addresses[middle])
            {
                high = middle;
            }
            else
            {
                low = middle + 1;
            }
        }

        return low - 1;
    }

    /// <summary>A page's decoded bytes: stored, inflated or BCPack blocks; null for a page that cannot be decoded.</summary>
    public byte[]? PageData(int index)
    {
        if (_cache.TryGetValue(index, out var cached))
        {
            return cached;
        }

        var page = Pages[index];
        var raw = _source.Read(page.FileOffset, (int)page.FileLength);
        byte[]? data = null;

        if (page.IsZlib)
        {
            var result = _inflater.Inflate(raw, DeflateWrapper.Zlib, int.MaxValue);

            if (result.Status == InflateStatus.Invalid)
            {
                throw new InvalidDataException($"page at {page.FileOffset:x} is not valid zlib");
            }

            data = _inflater.Output.ToArray();
        }
        else if (page.Codec == 0)
        {
            data = raw.ToArray();
        }
        else if (page.IsDecodable)
        {
            data = DecodeBcPack(page, raw);
        }

        if (_cache.Count > MaxCachedPages)
        {
            _cache.Clear();
        }

        _cache[index] = data;

        return data;
    }

    /// <summary>
    /// The bytes of [<paramref name="address"/>, + <paramref name="size"/>) in the virtual address space. Holes read
    /// as zero; bytes on pages that cannot be decoded also read as zero and are counted in <c>Missing</c>.
    /// </summary>
    public (byte[] Data, long Missing) ReadVirtual(long address, int size)
    {
        var output = new byte[size];
        long missing = 0;
        var position = address;
        var end = address + size;

        while (position < end)
        {
            var index = PageAt(position);

            if (index < 0)
            {
                throw new InvalidDataException("address before the first page");
            }

            var page = Pages[index];
            var data = PageData(index);
            var length = data is null ? page.EffectiveVirtualSize : data.Length;

            if (position >= page.VirtualAddress + length)
            {
                var next = index + 1 < _addresses.Length ? _addresses[index + 1] : end;
                position = Math.Min(next, end);

                continue;
            }

            var take = Math.Min(page.VirtualAddress + length, end) - position;

            if (data is null)
            {
                missing += take;
            }
            else
            {
                data.AsSpan((int)(position - page.VirtualAddress), (int)take).CopyTo(output.AsSpan((int)(position - address)));
            }

            position += take;
        }

        return (output, missing);
    }

    private List<TexturePage> ReadPages(int entries)
    {
        var table = _source.Read(HeaderSize, EntrySize * entries);
        var offsets = new SortedSet<long> { _source.Length };

        for (var index = 0; index < entries; index++)
        {
            offsets.Add(FileOffset(BinaryPrimitives.ReadUInt32LittleEndian(table[(index * EntrySize)..])));
        }

        var nextOffset = NextOffsets(offsets);
        var all = new List<TexturePage>(entries);

        for (var index = 0; index < entries; index++)
        {
            var low = BinaryPrimitives.ReadUInt32LittleEndian(table[(index * EntrySize)..]);
            var high = BinaryPrimitives.ReadUInt32LittleEndian(table[((index * EntrySize) + sizeof(uint))..]);
            var flag = (byte)(low >> FlagShift);
            var fileOffset = FileOffset(low);

            var length = nextOffset.GetValueOrDefault(fileOffset, fileOffset) - fileOffset;
            var virtualAddress = ((high & VirtualPageMask) * VirtualPageSize) + (((flag >> SubPageShift) & SubPageMask) * SubPageSize);
            all.Add(new TexturePage(virtualAddress, flag, fileOffset, length, 0, high >> TagShift));
        }

        var pages = new List<TexturePage>(entries);

        for (var index = 0; index < all.Count; index++)
        {
            var page = all[index];

            if (index + 1 < all.Count)
            {
                page = page with { VirtualSize = all[index + 1].VirtualAddress - page.VirtualAddress };
            }

            if (page.FileLength > 0)
            {
                pages.Add(page);
            }
        }

        return pages;
    }

    private static byte[] DecodeBcPack(TexturePage page, ReadOnlySpan<byte> stored)
    {
        var blocks = new byte[page.VirtualSize];
        var result = BcPackPageDecoder.Decode(page.Codec, stored, page.VirtualSize, blocks);

        if (!result.Success)
        {
            throw new InvalidDataException($"BCPack codec {page.Codec} page at {page.FileOffset:x}: {result.Describe()}");
        }

        return blocks;
    }

    private static long FileOffset(uint low)
    {
        return (long)(low & OffsetMask) << FileOffsetShift;
    }

    private static Dictionary<long, long> NextOffsets(SortedSet<long> offsets)
    {
        var next = new Dictionary<long, long>(offsets.Count);
        long? previous = null;

        foreach (var offset in offsets)
        {
            if (previous.HasValue)
            {
                next[previous.Value] = offset;
            }

            previous = offset;
        }

        return next;
    }

    private (List<TextureDescriptor> Descriptors, List<ulong> Hashes) ReadEtpf()
    {
        var header = _source.Read(EtpfOffset, EtpfHeaderSize);

        if (header.Length < EtpfHeaderSize || !header.StartsWith(EtpfMagic))
        {
            throw new InvalidDataException("no ETPF section at " + EtpfOffset);
        }

        var count = (int)BinaryPrimitives.ReadUInt32LittleEndian(header[CountOffset..]);
        var table = _source.Read(EtpfOffset + EtpfHeaderSize, count * (DescriptorSize + sizeof(ulong)));
        var descriptors = new List<TextureDescriptor>(count);
        var hashes = new List<ulong>(count);

        for (var index = 0; index < count; index++)
        {
            descriptors.Add(TextureDescriptorReader.Read(table[(index * DescriptorSize)..], index));
            hashes.Add(BinaryPrimitives.ReadUInt64LittleEndian(table[((count * DescriptorSize) + (index * sizeof(ulong)))..]));
        }

        return (descriptors, hashes);
    }
}
