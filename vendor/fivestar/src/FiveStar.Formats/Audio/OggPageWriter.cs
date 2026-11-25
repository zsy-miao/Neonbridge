// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Writes the pages of one logical Ogg bitstream (RFC 3533). Every packet ends on the page it starts on, so a page
/// holds at most 255 lacing values.
/// </summary>
public sealed class OggPageWriter
{
    /// <summary>Header type flag of the first page.</summary>
    public const byte FirstPage = 0x02;

    /// <summary>Header type flag of the last page.</summary>
    public const byte LastPage = 0x04;

    /// <summary>Lacing values a page can hold.</summary>
    public const int MaxSegments = 255;

    private const int HeaderSize = 27;
    private const int FlagsOffset = 5;
    private const int GranuleOffset = 6;
    private const int SerialOffset = 14;
    private const int SequenceOffset = 18;
    private const int CrcOffset = 22;
    private const int SegmentCountOffset = 26;
    private const int LacingStep = 255;

    private readonly Stream _output;
    private readonly uint _serial;
    private uint _sequence;

    public OggPageWriter(Stream output, uint serial)
    {
        _output = output;
        _serial = serial;
    }

    /// <summary>Lacing values a packet of <paramref name="length"/> bytes takes.</summary>
    public static int Segments(int length) => (length / LacingStep) + 1;

    /// <summary>Writes one page holding whole packets.</summary>
    public void Write(IReadOnlyList<byte[]> packets, long granule, byte flags)
    {
        var segments = packets.Sum(packet => Segments(packet.Length));
        var bodySize = packets.Sum(packet => packet.Length);
        var page = new byte[HeaderSize + segments + bodySize];

        "OggS"u8.CopyTo(page);
        page[FlagsOffset] = flags;
        BinaryPrimitives.WriteInt64LittleEndian(page.AsSpan(GranuleOffset), granule);
        BinaryPrimitives.WriteUInt32LittleEndian(page.AsSpan(SerialOffset), _serial);
        BinaryPrimitives.WriteUInt32LittleEndian(page.AsSpan(SequenceOffset), _sequence++);
        page[SegmentCountOffset] = (byte)segments;

        var lacing = HeaderSize;
        var body = HeaderSize + segments;

        foreach (var packet in packets)
        {
            for (var left = packet.Length; left >= 0; left -= LacingStep)
            {
                page[lacing++] = (byte)Math.Min(left, LacingStep);

                if (left < LacingStep)
                {
                    break;
                }
            }

            packet.CopyTo(page, body);
            body += packet.Length;
        }

        BinaryPrimitives.WriteUInt32LittleEndian(page.AsSpan(CrcOffset), OggCrc.Compute(page));
        _output.Write(page);
    }
}
