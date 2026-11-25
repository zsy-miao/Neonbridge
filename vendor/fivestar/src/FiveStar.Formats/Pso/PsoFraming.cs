// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Formats.Pso;

/// <summary>
/// The IFF-like framing of PSO files: a 4-letter tag and a big-endian u32 size (header included) per chunk, ending
/// with <c>CHKS</c>, whose first big-endian u32 is the size of the whole file.
/// </summary>
public static class PsoFraming
{
    /// <summary>Bytes of a chunk header: tag and size.</summary>
    public const int ChunkHeaderSize = 8;

    private const int TagSize = 4;
    private const int ChecksumSizeField = 4;

    /// <summary>The tag of the closing checksum chunk.</summary>
    public static ReadOnlySpan<byte> ChecksumTag => "CHKS"u8;

    /// <summary>The tag that opens every PSO file.</summary>
    public static ReadOnlySpan<byte> FileTag => "PSIN"u8;

    /// <summary>The chunks up to and including <c>CHKS</c>, stopping early at a chunk that is not well-formed.</summary>
    public static IReadOnlyList<PsoChunk> Chunks(ReadOnlySpan<byte> data)
    {
        var chunks = new List<PsoChunk>();
        var offset = 0;

        while (offset + ChunkHeaderSize <= data.Length)
        {
            var tag = data.Slice(offset, TagSize);
            var size = BinaryPrimitives.ReadUInt32BigEndian(data[(offset + TagSize)..]);

            if (!IsLetters(tag) || size < ChunkHeaderSize)
            {
                break;
            }

            chunks.Add(new PsoChunk(Encoding.ASCII.GetString(tag), offset, (int)Math.Min(size, int.MaxValue)));

            if (tag.SequenceEqual(ChecksumTag))
            {
                break;
            }

            offset += (int)Math.Min(size, int.MaxValue - offset);
        }

        return chunks;
    }

    /// <summary>
    /// The file size declared by <c>CHKS</c>, or null when the chunks so far end without one or break the framing.
    /// When only the <c>CHKS</c> header is in view, its end stands in for the size.
    /// </summary>
    public static long? DeclaredSize(ReadOnlySpan<byte> data)
    {
        long offset = 0;

        while (offset + ChunkHeaderSize <= data.Length)
        {
            var tag = data.Slice((int)offset, TagSize);
            var size = BinaryPrimitives.ReadUInt32BigEndian(data[((int)offset + TagSize)..]);

            if (!IsLetters(tag) || size < ChunkHeaderSize)
            {
                return null;
            }

            if (tag.SequenceEqual(ChecksumTag))
            {
                return DeclaredByChecksum(data, offset, size);
            }

            offset += size;
        }

        return null;
    }

    /// <summary>True when every byte is an ASCII letter (and there is at least one).</summary>
    public static bool IsLetters(ReadOnlySpan<byte> tag)
    {
        if (tag.IsEmpty)
        {
            return false;
        }

        foreach (var value in tag)
        {
            if (!char.IsAsciiLetter((char)value))
            {
                return false;
            }
        }

        return true;
    }

    private static long DeclaredByChecksum(ReadOnlySpan<byte> data, long offset, uint size)
    {
        if (offset + ChunkHeaderSize + ChecksumSizeField <= data.Length)
        {
            return BinaryPrimitives.ReadUInt32BigEndian(data[((int)offset + ChunkHeaderSize)..]);
        }

        return offset + size;
    }
}
