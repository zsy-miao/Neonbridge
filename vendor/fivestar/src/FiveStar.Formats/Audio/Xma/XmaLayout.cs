// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// The fixed parameters of an XMA2 stream (docs/xma.md). XMA2 is WMA Pro with the decode flags fixed at 0x10D6: a
/// length prefix on every frame, a dynamic range byte, up to four subframes per channel and 512-sample frames.
/// </summary>
public static class XmaLayout
{
    /// <summary>Bytes of a packet.</summary>
    public const int PacketBytes = 2048;

    /// <summary>Bits of a packet, its header included.</summary>
    public const int PacketBits = PacketBytes * 8;

    /// <summary>The packet header: frame count (6 bits), first frame offset (15), metadata (3), packet skip (8).</summary>
    public const int PacketHeaderBits = 32;

    public const int FrameCountBits = 6;
    public const int FrameOffsetBits = 15;
    public const int MetadataBits = 3;
    public const int SkipBits = 8;

    /// <summary>Bits of the length prefix of a frame (log2 of the packet size plus 4); the length counts the whole frame.</summary>
    public const int FrameLengthBits = 15;

    /// <summary>A frame length of all ones: no further frame starts in the packet.</summary>
    public const int NoFrame = (1 << FrameLengthBits) - 1;

    /// <summary>Samples per channel of a frame.</summary>
    public const int FrameSamples = 512;

    /// <summary>log2 of the largest subframe count, from the decode flags (bits 3 to 5 of 0x10D6).</summary>
    public const int SubframeCountLog2 = 2;

    public const int MaxSubframes = 1 << SubframeCountLog2;

    /// <summary>The shortest subframe: a quarter frame.</summary>
    public const int MinSubframeSamples = FrameSamples / MaxSubframes;

    /// <summary>The block sizes in use: 512, 256 and 128 samples.</summary>
    public const int BlockSizes = SubframeCountLog2 + 1;

    /// <summary>Channels of one XMA stream.</summary>
    public const int MaxChannels = 2;

    /// <summary>Bits of the dynamic range compression gain every frame carries.</summary>
    public const int DynamicRangeBits = 8;

    /// <summary>Bits of a frame's start and end trim counts (log2 of twice the frame size).</summary>
    public const int TrimBits = 10;

    /// <summary>The PCM depth the decoder scales to: output 1.0 is 2^15.</summary>
    public const int BitsPerSample = 16;

    /// <summary>The base quantisation step: 90 * bits per sample / 16.</summary>
    public const int BaseQuantStep = 90;

    /// <summary>Most scale factor bands of a block.</summary>
    public const int MaxBands = 29;
}
