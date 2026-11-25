// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Rpf;

/// <summary>
/// Finds Bink 2 videos stored raw in an archive. A video starts on a 16-byte boundary with <c>KB2</c>, a revision
/// letter and eight u32 fields: size - 8, frame count, largest frame, frame count again, width, height, frame-rate
/// numerator and denominator. A candidate counts only when these fields are plausible and the file fits the archive.
/// </summary>
public sealed class BinkScanner
{
    private const int HeaderSize = 44;
    private const int RevisionOffset = 3;
    private const int FieldsOffset = 4;
    private const int SizeBias = 8;
    private const int Alignment = 16;
    private const byte FirstRevision = (byte)'a';
    private const byte LastRevision = (byte)'z';
    private const uint MaxFrames = 1_000_000;
    private const uint MinDimension = 16;
    private const uint MaxDimension = 8192;

    private readonly IByteSource _source;

    public BinkScanner(IByteSource source)
    {
        _source = source;
    }

    /// <summary>Gets the magic of a Bink 2 file.</summary>
    public static ReadOnlySpan<byte> Magic => "KB2"u8;

    /// <summary>Every video in archive order; a match is skipped over as a whole.</summary>
    public IEnumerable<BinkVideo> Scan()
    {
        long position = 0;

        while (true)
        {
            var found = ByteSearch.IndexOf(_source, Magic, position, _source.Length);

            if (found < 0)
            {
                yield break;
            }

            var video = TryRead(found);

            if (video is null)
            {
                position = found + 1;

                continue;
            }

            yield return video;

            position = found + video.Size;
        }
    }

    private BinkVideo? TryRead(long offset)
    {
        if (offset % Alignment != 0 || _source.Length < offset + HeaderSize)
        {
            return null;
        }

        var header = _source.Read(offset, HeaderSize);
        var revision = header[RevisionOffset];
        Span<uint> fields = stackalloc uint[(HeaderSize - FieldsOffset) / sizeof(uint)];

        for (var index = 0; index < fields.Length; index++)
        {
            fields[index] = BinaryPrimitives.ReadUInt32LittleEndian(header[(FieldsOffset + (index * sizeof(uint)))..]);
        }

        var video = new BinkVideo(
            offset,
            (long)fields[0] + SizeBias,
            (char)revision,
            fields[(int)Field.Frames],
            fields[(int)Field.Width],
            fields[(int)Field.Height],
            fields[(int)Field.RateNumerator],
            fields[(int)Field.RateDenominator]);

        if (revision < FirstRevision || revision > LastRevision || !IsPlausible(video, fields))
        {
            return null;
        }

        return video;
    }

    private bool IsPlausible(BinkVideo video, ReadOnlySpan<uint> fields)
    {
        var size = fields[(int)Field.Size];
        var largestFrame = fields[(int)Field.LargestFrame];

        if (video.Frames != fields[(int)Field.FramesAgain] || video.Frames == 0 || video.Frames >= MaxFrames)
        {
            return false;
        }

        if (video.Width < MinDimension || video.Width > MaxDimension || video.Height < MinDimension || video.Height > MaxDimension)
        {
            return false;
        }

        if (largestFrame == 0 || largestFrame >= size || video.Offset + video.Size > _source.Length)
        {
            return false;
        }

        return video.FrameRateNumerator != 0 && video.FrameRateDenominator != 0;
    }

    private enum Field
    {
        Size,
        Frames,
        LargestFrame,
        FramesAgain,
        Width,
        Height,
        RateNumerator,
        RateDenominator,
    }
}
