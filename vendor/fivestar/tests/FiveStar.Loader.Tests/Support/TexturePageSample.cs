// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.IO.Compression;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Just enough of a <c>.ttpf</c> page file (docs/textures.md) to fetch one texture's bytes: the RDS5 page table, the
/// ETPF descriptor addresses and stored or zlib pages. A texture's bytes run from its address to the next texture's.
/// </summary>
internal sealed class TexturePageSample : IDisposable
{
    private readonly FileStream _file;
    private readonly List<(long Address, long Offset, long Length, byte Flag)> _pages = [];
    private readonly SortedSet<long> _starts = [];
    private readonly long _etpf;

    public TexturePageSample(string path)
    {
        _file = File.OpenRead(path);
        int entries = (int)BinaryPrimitives.ReadUInt32LittleEndian(Read(4, 4)) - 1;
        ReadPages(entries);
        _etpf = (0x18 + (8L * entries) + 0xFF) & ~0xFFL;
        int count = (int)BinaryPrimitives.ReadUInt32LittleEndian(Read(_etpf + 8, 4));
        byte[] descriptors = Read(_etpf + 16, 16 * count);

        for (int index = 0; index < count; index++)
        {
            _starts.Add((BinaryPrimitives.ReadUInt32LittleEndian(descriptors.AsSpan((16 * index) + 8)) & 0x1FFFFF) * 0x1000L);
        }
    }

    /// <summary>The bytes from a texture's address to the next texture's, or null when a page is BCPack-coded.</summary>
    public byte[]? Texture(int index)
    {
        long start = (BinaryPrimitives.ReadUInt32LittleEndian(Read(_etpf + 16 + (16 * index) + 8, 4)) & 0x1FFFFF) * 0x1000L;
        long end = _starts.GetViewBetween(start + 1, long.MaxValue).Min;
        var output = new byte[end - start];

        for (int page = 0; page < _pages.Count; page++)
        {
            var (address, offset, length, flag) = _pages[page];
            long pageEnd = page + 1 < _pages.Count ? _pages[page + 1].Address : long.MaxValue;

            if (length == 0 || address >= end || pageEnd <= start)
            {
                continue;
            }

            byte[]? data = Decode(offset, length, flag);

            if (data is null)
            {
                return null;
            }

            long from = Math.Max(start, address);
            long to = Math.Min(end, address + data.Length);
            data.AsSpan((int)(from - address), (int)(to - from)).CopyTo(output.AsSpan((int)(from - start)));
        }

        return output;
    }

    public void Dispose()
    {
        _file.Dispose();
    }

    private void ReadPages(int entries)
    {
        byte[] table = Read(0x18, 8 * entries);
        var offsets = new SortedSet<long> { _file.Length };

        for (int index = 0; index < entries; index++)
        {
            offsets.Add((BinaryPrimitives.ReadUInt32LittleEndian(table.AsSpan(8 * index)) & 0xFFFFFF) << 8);
        }

        for (int index = 0; index < entries; index++)
        {
            uint low = BinaryPrimitives.ReadUInt32LittleEndian(table.AsSpan(8 * index));
            uint high = BinaryPrimitives.ReadUInt32LittleEndian(table.AsSpan((8 * index) + 4));
            long offset = (low & 0xFFFFFF) << 8;
            long length = offsets.GetViewBetween(offset + 1, long.MaxValue).Min - offset;
            byte flag = (byte)(low >> 24);
            _pages.Add((((high & 0xFFFFF) * 0x4000L) + (((flag >> 6) & 3) * 0x1000L), offset, length, flag));
        }
    }

    private byte[]? Decode(long offset, long length, byte flag)
    {
        byte[] raw = Read(offset, (int)length);

        if ((flag & 0x20) != 0)
        {
            using var zlib = new ZLibStream(new MemoryStream(raw), CompressionMode.Decompress);
            using var inflated = new MemoryStream();
            zlib.CopyTo(inflated);

            return inflated.ToArray();
        }

        if (((flag >> 2) & 7) != 0)
        {
            return null;
        }

        return raw;
    }

    private byte[] Read(long offset, int length)
    {
        var buffer = new byte[length];
        _file.Position = offset;
        _file.ReadExactly(buffer);

        return buffer;
    }
}
