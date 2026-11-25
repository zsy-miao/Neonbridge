// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Containers;

/// <summary>A DXBC shader container: a header with a content hash and a table of chunks.</summary>
public sealed class DxbcContainer
{
    /// <summary>The container magic at offset 0.</summary>
    public const string Magic = "DXBC";

    /// <summary>Offset of the 16-byte content hash (header field after the magic).</summary>
    public const int HashOffset = 4;

    /// <summary>Size of the content hash.</summary>
    public const int HashSize = 16;

    /// <summary>Offset of the u32 total size of the container.</summary>
    public const int TotalSizeOffset = 0x18;

    /// <summary>Size of the header up to the chunk offsets.</summary>
    public const int HeaderSize = 0x20;

    // DXBC header: u32 chunk count @0x1C, then one u32 chunk offset per chunk from 0x20.
    private const int ChunkCountOffset = 0x1C;
    private const int ChunkOffsetsStart = 0x20;

    // Chunk header: four-character code, u32 payload size.
    private const int FourCcSize = 4;
    private const int ChunkHeaderSize = 8;
    private const int DwordSize = 4;

    private readonly List<DxbcChunk> _chunks;

    private DxbcContainer(List<DxbcChunk> chunks, ReadOnlyMemory<byte> hash)
    {
        _chunks = chunks;
        Hash = hash;
    }

    public IReadOnlyList<DxbcChunk> Chunks => _chunks;

    public ReadOnlyMemory<byte> Hash { get; }

    /// <summary>True when the bytes start with the DXBC magic.</summary>
    public static bool HasMagic(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length < FourCcSize)
        {
            return false;
        }

        return Encoding.Latin1.GetString(bytes[..FourCcSize]) == Magic;
    }

    /// <summary>Reads the chunk table of a container.</summary>
    public static DxbcContainer Parse(ReadOnlyMemory<byte> bytes)
    {
        var span = bytes.Span;

        if (!HasMagic(span) || span.Length < HeaderSize)
        {
            throw new InvalidDataException("not a DXBC container");
        }

        var count = BinaryPrimitives.ReadInt32LittleEndian(span[ChunkCountOffset..]);

        if (count < 0 || count > (span.Length - HeaderSize) / DwordSize)
        {
            throw new InvalidDataException("chunk table outside the container");
        }

        var chunks = new List<DxbcChunk>(count);

        for (var index = 0; index < count; index++)
        {
            var offset = BinaryPrimitives.ReadInt32LittleEndian(span[(ChunkOffsetsStart + (index * DwordSize))..]);

            if (offset < ChunkOffsetsStart + (count * DwordSize) || offset > span.Length - ChunkHeaderSize)
            {
                throw new InvalidDataException("chunk header outside the container");
            }

            var fourCc = Encoding.Latin1.GetString(span.Slice(offset, FourCcSize));
            var size = BinaryPrimitives.ReadInt32LittleEndian(span[(offset + FourCcSize)..]);

            if (size < 0)
            {
                throw new InvalidDataException("negative chunk size");
            }

            var start = offset + ChunkHeaderSize;
            var length = Math.Min(size, bytes.Length - start);
            chunks.Add(new DxbcChunk(fourCc, bytes.Slice(start, length)));
        }

        return new DxbcContainer(chunks, bytes.Slice(HashOffset, HashSize));
    }

    /// <summary>The last chunk with this code, or null.</summary>
    public DxbcChunk? Find(string fourCc)
    {
        return _chunks.LastOrDefault(chunk => chunk.FourCc == fourCc);
    }

    /// <summary>The first chunk with this code, or null.</summary>
    public DxbcChunk? FindFirst(string fourCc)
    {
        return _chunks.FirstOrDefault(chunk => chunk.FourCc == fourCc);
    }
}
