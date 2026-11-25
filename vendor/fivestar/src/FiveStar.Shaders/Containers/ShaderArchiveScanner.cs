// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;
using System.IO.Compression;
using System.Text;

namespace FiveStar.Shaders.Containers;

/// <summary>
/// Finds shader records: in the plaintext SGD5 tables of shaders.rpf (a table starts at a 4 KB page beginning with
/// "SGD5" and runs to the next one, the last to the end of the file), or anywhere in a file such as the game
/// executable. A record is <c>u8 name length (incl. NUL), name, NUL, u8 tag</c>, then either
/// <c>u32 csize, u32 constant, u32 csize, u32 dsize, raw deflate[csize]</c> decompressing to a DXBC container of dsize
/// bytes, or (stored) <c>u32 size, DXBC[size]</c>.
/// </summary>
public static class ShaderArchiveScanner
{
    /// <summary>Region name of the records found in the game executable.</summary>
    public const string ExecutableRegion = "exe";

    private const string TableMagic = "SGD5";
    private const int PageSize = 0x1000;
    private const string RegionPrefix = "sgd";

    // Name: at least 3 and at most 254 printable ASCII characters; the length byte is at least 5.
    private const int MinLengthByte = 5;
    private const int MinNameLength = 3;
    private const int MaxNameLength = 254;
    private const byte FirstPrintable = 0x20;
    private const byte LastPrintable = 0x7E;

    // Tag byte range of shader records (0x32, 0x3C, 0x40, 0x41, 0x42 are seen).
    private const byte FirstTag = 0x30;
    private const byte LastTag = 0x4F;

    // Header after the name: u8 tag @0, u32 csize @1, u32 constant @5, u32 csize @9, u32 dsize @13; data @17.
    // Stored: u8 tag @0, u32 size @1; the DXBC container @5.
    private const int CompressedSizeField = 1;
    private const int ConstantField = 5;
    private const int CompressedSizeCopyField = 9;
    private const int DecompressedSizeField = 13;
    private const int RecordHeaderSize = 17;
    private const int StoredHeaderSize = 5;
    private const int MaxCompressedSize = 0x4000000;
    private const int MaxDecompressedSize = 0x10000000;

    /// <summary>Every record of the SGD5 tables that decompresses to a DXBC container of the declared size, in file order.</summary>
    public static IReadOnlyList<ShaderArchiveRecord> Scan(byte[] archive)
    {
        var tables = FindTables(archive);
        var records = new List<ShaderArchiveRecord>();

        for (var index = 0; index < tables.Count; index++)
        {
            var start = tables[index];
            var end = index + 1 < tables.Count ? tables[index + 1] : archive.Length;
            var region = RegionPrefix + (index + 1).ToString(CultureInfo.InvariantCulture);
            ScanRange(archive, region, start, end, records);
        }

        return records;
    }

    /// <summary>Every record anywhere in <paramref name="file"/> (the game executable embeds a few), in file order.</summary>
    public static IReadOnlyList<ShaderArchiveRecord> ScanFile(byte[] file, string region)
    {
        var records = new List<ShaderArchiveRecord>();
        ScanRange(file, region, 0, file.Length, records);

        return records;
    }

    private static List<int> FindTables(byte[] archive)
    {
        var magic = Encoding.Latin1.GetBytes(TableMagic);
        var tables = new List<int>();

        for (var page = 0; page + magic.Length <= archive.Length; page += PageSize)
        {
            if (archive.AsSpan(page, magic.Length).SequenceEqual(magic))
            {
                tables.Add(page);
            }
        }

        return tables;
    }

    /// <summary>
    /// Tries a record at every position. A record's payload is skipped; a candidate that is no record moves on by one
    /// byte only, since its tag byte may be the length byte of a real record (a name of 47 characters has the
    /// length byte '0').
    /// </summary>
    private static void ScanRange(byte[] file, string region, int start, int end, List<ShaderArchiveRecord> records)
    {
        var data = new ReadOnlyMemory<byte>(file, start, end - start);
        var position = 0;

        while (position < data.Length)
        {
            var nameLength = MatchName(data.Span, position);

            if (nameLength < 0)
            {
                position++;
                continue;
            }

            var tagAt = position + 1 + nameLength + 1;
            var record = TryRead(data, region, start, position, nameLength, tagAt, out var payloadEnd);

            if (record is null)
            {
                position++;
                continue;
            }

            records.Add(record);
            position = payloadEnd;
        }
    }

    /// <summary>Length of the printable name at position + 1 when a record header starts here, else -1.</summary>
    private static int MatchName(ReadOnlySpan<byte> data, int position)
    {
        if (data[position] < MinLengthByte)
        {
            return -1;
        }

        var run = 0;

        while (position + 1 + run < data.Length && run <= MaxNameLength && IsPrintable(data[position + 1 + run]))
        {
            run++;
        }

        var nulAt = position + 1 + run;
        var tagAt = nulAt + 1;

        if (run < MinNameLength || run > MaxNameLength || tagAt >= data.Length || data[nulAt] != 0)
        {
            return -1;
        }

        if (data[tagAt] < FirstTag || data[tagAt] > LastTag)
        {
            return -1;
        }

        return run;
    }

    private static bool IsPrintable(byte value)
    {
        return value >= FirstPrintable && value <= LastPrintable;
    }

    private static ShaderArchiveRecord? TryRead(
        ReadOnlyMemory<byte> data, string region, int tableStart, int position, int nameLength, int tagAt, out int payloadEnd)
    {
        var span = data.Span;
        payloadEnd = 0;

        if (span[position] != nameLength + 1 || tagAt + StoredHeaderSize + DxbcContainer.HeaderSize > data.Length)
        {
            return null;
        }

        var name = Encoding.Latin1.GetString(span.Slice(position + 1, nameLength));
        var stored = TryReadStored(data, tagAt);

        if (stored is not null)
        {
            payloadEnd = tagAt + StoredHeaderSize + stored.Length;

            return Record(region, tableStart, position, tagAt, span[tagAt], name, stored.Length, stored.Length, 0, stored);
        }

        if (tagAt + RecordHeaderSize > data.Length)
        {
            return null;
        }

        var compressed = BinaryPrimitives.ReadInt32LittleEndian(span[(tagAt + CompressedSizeField)..]);
        var copy = BinaryPrimitives.ReadInt32LittleEndian(span[(tagAt + CompressedSizeCopyField)..]);
        var size = BinaryPrimitives.ReadInt32LittleEndian(span[(tagAt + DecompressedSizeField)..]);

        if (compressed != copy || compressed <= 0 || compressed >= MaxCompressedSize || size <= 0 || size >= MaxDecompressedSize)
        {
            return null;
        }

        var payloadStart = tagAt + RecordHeaderSize;
        var payload = data.Slice(payloadStart, Math.Min(compressed, data.Length - payloadStart));
        var container = Inflate(payload, size);

        if (container is null || container.Length != size || !DxbcContainer.HasMagic(container))
        {
            return null;
        }

        var constant = BinaryPrimitives.ReadUInt32LittleEndian(span[(tagAt + ConstantField)..]);
        payloadEnd = payloadStart + payload.Length;

        return Record(region, tableStart, position, tagAt, span[tagAt], name, compressed, size, constant, container);
    }

    /// <summary>The DXBC container right after the size field when its own total size equals that field, else null.</summary>
    private static byte[]? TryReadStored(ReadOnlyMemory<byte> data, int tagAt)
    {
        var span = data.Span;
        var size = BinaryPrimitives.ReadInt32LittleEndian(span[(tagAt + CompressedSizeField)..]);
        var start = tagAt + StoredHeaderSize;

        if (size <= DxbcContainer.HeaderSize || size > data.Length - start || !DxbcContainer.HasMagic(span[start..]))
        {
            return null;
        }

        if (BinaryPrimitives.ReadInt32LittleEndian(span[(start + DxbcContainer.TotalSizeOffset)..]) != size)
        {
            return null;
        }

        var chunks = BinaryPrimitives.ReadInt32LittleEndian(span[(start + DxbcContainer.HeaderSize - sizeof(int))..]);

        if (chunks < 0 || chunks > (size - DxbcContainer.HeaderSize) / sizeof(int))
        {
            return null;
        }

        var container = span.Slice(start, size).ToArray();

        try
        {
            DxbcContainer.Parse(container);
        }
        catch (ArgumentOutOfRangeException)
        {
            return null;
        }

        return container;
    }

    private static ShaderArchiveRecord Record(
        string region, int tableStart, int position, int tagAt, byte tag, string name, int compressed, int size, uint constant, byte[] container)
    {
        var chunks = DxbcContainer.Parse(container).Chunks.Select(chunk => chunk.FourCc).ToList();

        return new ShaderArchiveRecord(
            region, tableStart + position, tableStart + tagAt, tag, name, compressed, size, constant, container, chunks);
    }

    private static byte[]? Inflate(ReadOnlyMemory<byte> payload, int expected)
    {
        try
        {
            using var input = new MemoryStream(payload.ToArray());
            using var deflate = new DeflateStream(input, CompressionMode.Decompress);
            using var output = new MemoryStream(expected);
            deflate.CopyTo(output);

            return output.ToArray();
        }
        catch (InvalidDataException)
        {
            return null;
        }
    }
}
