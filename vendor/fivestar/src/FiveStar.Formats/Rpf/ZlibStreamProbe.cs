// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;

namespace FiveStar.Formats.Rpf;

/// <summary>
/// Finds and inflates the zlib streams stored in an RPF9 archive's data area. A stream starts on a 16-byte boundary
/// with one of the four zlib headers <c>78 01</c>, <c>78 5E</c>, <c>78 9C</c>, <c>78 DA</c> and counts only when it
/// inflates completely within a 16 MiB input window.
/// </summary>
public sealed class ZlibStreamProbe
{
    /// <summary>Entries are stored in chunks of 64 KiB inflated data.</summary>
    public const int ChunkSize = 64 << 10;

    /// <summary>Streams and chunks start on 16-byte boundaries.</summary>
    public const int Alignment = 16;

    /// <summary>The most output accepted from the first stream of an entry.</summary>
    public const int MaxStreamOutput = 64 << 20;

    private const int InputWindow = 16 << 20;
    private const byte ZlibFirstByte = 0x78;
    private const int HeaderLength = 2;

    // The FLG byte of each zlib compression level: 01 fastest, 5E fast, 9C default, DA best.
    private const byte LevelFastest = 0x01;
    private const byte LevelFast = 0x5E;
    private const byte LevelDefault = 0x9C;
    private const byte LevelBest = 0xDA;

    private readonly IByteSource _source;
    private readonly ZlibInflater _inflater;

    public ZlibStreamProbe(IByteSource source, ZlibInflater inflater)
    {
        _source = source;
        _inflater = inflater;
    }

    /// <summary>Gets the output of the last successful inflate.</summary>
    public ReadOnlySpan<byte> Output => _inflater.Output;

    /// <summary>Rounds an offset up to the next 16-byte boundary.</summary>
    public static long Align(long offset)
    {
        return (offset + Alignment - 1) / Alignment * Alignment;
    }

    /// <summary>The first aligned offset at or after <paramref name="from"/> holding a zlib header inside [from, end), or -1.</summary>
    public long NextHeader(long from, long end)
    {
        const int Window = 16 << 20;
        var position = Align(from);
        end = Math.Min(end, _source.Length);

        while (position + HeaderLength <= end)
        {
            var take = (int)Math.Min(Window, end - position);
            var window = _source.Read(position, take);

            for (var index = 0; index + HeaderLength <= window.Length; index += Alignment)
            {
                if (window[index] == ZlibFirstByte && IsLevelByte(window[index + 1]))
                {
                    return position + index;
                }
            }

            position += Align(take);
        }

        return -1;
    }

    /// <summary>Inflates the stream at <paramref name="offset"/>; true when it completes with output, giving the bytes it used.</summary>
    public bool TryInflate(long offset, int maxOutput, out int used)
    {
        var input = _source.Read(offset, InputWindow);
        var result = _inflater.Inflate(input, DeflateWrapper.Zlib, maxOutput);
        used = result.Consumed;

        return result.Status == InflateStatus.Complete && _inflater.Output.Length > 0;
    }

    /// <summary>Inflates one chunk at <paramref name="offset"/>: a zlib stream of at most 64 KiB output.</summary>
    public bool TryChunk(long offset, out int used)
    {
        var head = _source.Read(offset, HeaderLength);

        if (head.Length < HeaderLength || head[0] != ZlibFirstByte || !IsLevelByte(head[1]))
        {
            used = 0;

            return false;
        }

        return TryInflate(offset, ChunkSize, out used);
    }

    private static bool IsLevelByte(byte value)
    {
        return value == LevelBest || value == LevelDefault || value == LevelFast || value == LevelFastest;
    }
}
