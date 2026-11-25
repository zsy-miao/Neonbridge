// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>The 32-bit header of an XMA2 packet, big-endian bit order.</summary>
/// <param name="FrameCount">The frame count field; summed over a stream it equals the frames decoded.</param>
/// <param name="FirstFrameOffset">Bits after the header that still belong to the frame begun in an earlier packet.</param>
/// <param name="Metadata">Three bits the decoder does not use.</param>
/// <param name="SkipCount">Packets of other streams that follow before this stream's next packet.</param>
public readonly record struct XmaPacketHeader(int FrameCount, int FirstFrameOffset, int Metadata, int SkipCount)
{
    [HotPath]
    public static XmaPacketHeader Read(ReadOnlySpan<byte> packet)
    {
        var reader = new XmaBitReader(packet, 0, XmaLayout.PacketHeaderBits);
        var frames = (int)reader.Read(XmaLayout.FrameCountBits);
        var offset = (int)reader.Read(XmaLayout.FrameOffsetBits);
        var metadata = (int)reader.Read(XmaLayout.MetadataBits);

        return new XmaPacketHeader(frames, offset, metadata, (int)reader.Read(XmaLayout.SkipBits));
    }
}
