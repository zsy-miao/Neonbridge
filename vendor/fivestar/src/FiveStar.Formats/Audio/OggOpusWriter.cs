// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Wraps the packets of a mono or stereo Opus track in an Ogg Opus file (RFC 7845): an identification header page, a
/// comment header page, then audio pages of about one second. The decoder drops the first <see cref="PreSkip"/>
/// samples, and the last page's granule position trims the rest to the header's sample count per channel.
/// </summary>
public static class OggOpusWriter
{
    private const int HeadSize = 19;
    private const int VersionOffset = 8;
    private const int ChannelsOffset = 9;
    private const int PreSkipOffset = 10;
    private const int InputRateOffset = 12;
    private const byte Version = 1;
    private const string Vendor = "Five Star Recomp";

    /// <summary>
    /// Samples the decoder drops at the start: the delay of the reference encoder. The decoded audio of the dump is
    /// silent up to about this sample; the containers do not store the value.
    /// </summary>
    public const ushort PreSkip = 312;

    public static void Write(Stream output, IByteSource source, AwcTrack track, uint serial)
    {
        var pages = new OggPageWriter(output, serial);
        pages.Write([Head(track.Format)], 0, OggPageWriter.FirstPage);
        pages.Write([Tags()], 0, 0);

        var end = Math.Min(PreSkip + track.Format.Frames, track.PacketSamples);
        var page = new List<byte[]>();
        var segments = 0;
        long granule = 0;
        long pageStart = 0;

        for (var index = 0; index < track.Packets.Count; index++)
        {
            var packet = track.Packets[index];
            var full = segments + OggPageWriter.Segments(packet.Length) > OggPageWriter.MaxSegments
                || (granule - pageStart >= OpusPacket.SampleRate && granule < end);

            if (full)
            {
                pages.Write(page, granule, 0);
                page.Clear();
                segments = 0;
                pageStart = granule;
            }

            page.Add(source.Read(packet.Offset, packet.Length).ToArray());
            segments += OggPageWriter.Segments(packet.Length);
            granule += packet.Samples;
        }

        pages.Write(page, Math.Max(end, pageStart), OggPageWriter.LastPage);
    }

    private static byte[] Head(AwcFormat format)
    {
        var head = new byte[HeadSize];
        "OpusHead"u8.CopyTo(head);
        head[VersionOffset] = Version;
        head[ChannelsOffset] = format.Channels;
        BinaryPrimitives.WriteUInt16LittleEndian(head.AsSpan(PreSkipOffset), PreSkip);
        BinaryPrimitives.WriteUInt32LittleEndian(head.AsSpan(InputRateOffset), format.SampleRate);

        return head;
    }

    private static byte[] Tags()
    {
        var vendor = Encoding.ASCII.GetBytes(Vendor);
        var tags = new byte["OpusTags"u8.Length + sizeof(uint) + vendor.Length + sizeof(uint)];
        "OpusTags"u8.CopyTo(tags);
        BinaryPrimitives.WriteInt32LittleEndian(tags.AsSpan("OpusTags"u8.Length), vendor.Length);
        vendor.CopyTo(tags, "OpusTags"u8.Length + sizeof(uint));

        return tags;
    }
}
