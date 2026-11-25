// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;

namespace FiveStar.Formats.Rpf;

/// <summary>
/// Finds an entry by the first bytes of its inflated data and reads it as a chunk run up to a size the caller knows
/// (from the entry's own header). The run stops early at a chunk that is neither zlib nor a raw 64 KiB chunk.
/// </summary>
public sealed class ChunkRunReader
{
    // Stored bytes read and inflated bytes produced when matching the start of an entry.
    private const int ProbeInput = 256;
    private const int ProbeOutput = 64;

    private readonly IByteSource _source;
    private readonly ZlibStreamProbe _probe;
    private readonly ZlibInflater _inflater;

    public ChunkRunReader(IByteSource source, ZlibStreamProbe probe, ZlibInflater inflater)
    {
        _source = source;
        _probe = probe;
        _inflater = inflater;
    }

    /// <summary>The offset of the first aligned zlib stream whose inflated data starts with <paramref name="magic"/>, or -1.</summary>
    public long Find(ReadOnlySpan<byte> magic)
    {
        long position = 0;

        while (position < _source.Length)
        {
            var start = _probe.NextHeader(position, _source.Length);

            if (start < 0)
            {
                return -1;
            }

            _inflater.Inflate(_source.Read(start, ProbeInput), DeflateWrapper.Zlib, ProbeOutput);

            if (_inflater.Output.StartsWith(magic))
            {
                return start;
            }

            position = start + ZlibStreamProbe.Alignment;
        }

        return -1;
    }

    /// <summary>Reads up to <paramref name="size"/> inflated bytes from the chunk run at <paramref name="start"/>.</summary>
    public ChunkRun Read(long start, long size)
    {
        var data = new ArrayBufferWriter<byte>((int)Math.Min(size, Array.MaxLength));
        var position = start;

        while (data.WrittenCount < size && position < _source.Length)
        {
            var next = ReadChunk(position, size - data.WrittenCount, data);

            if (next < 0)
            {
                break;
            }

            position = next;
        }

        return new ChunkRun(start, data.WrittenSpan.ToArray(), position);
    }

    // Appends one chunk (64 KiB, or the `left` bytes at the end); returns the position of the next chunk, or -1.
    private long ReadChunk(long position, long left, ArrayBufferWriter<byte> data)
    {
        var want = (int)Math.Min(ZlibStreamProbe.ChunkSize, left);

        // A chunk longer than the bytes still wanted is cut: the caller asked for the start of the entry only.
        if (_probe.TryChunk(position, out var used) && _probe.Output.Length >= want)
        {
            data.Write(_probe.Output[..want]);

            return ZlibStreamProbe.Align(position + used);
        }

        var rawEnd = position + ZlibStreamProbe.ChunkSize;

        if (want == ZlibStreamProbe.ChunkSize && _probe.TryChunk(rawEnd, out _))
        {
            data.Write(_source.Read(position, want));

            return rawEnd;
        }

        return -1;
    }
}
