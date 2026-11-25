// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Audio;

/// <summary>
/// A <see cref="AwcChunkTag.StreamFormat"/> chunk: u32 block count, u32 block size, u32 channel count, then 16 bytes
/// per channel (u32 id, u32 samples, i16 headroom, u16 sample rate, u8 codec, u8 channel count, u16 unknown).
/// </summary>
/// <param name="BlockCount">Number of blocks in the data chunk; the last may be cut short.</param>
/// <param name="BlockSize">Size of a block.</param>
/// <param name="Channels">The channels in table order.</param>
public sealed record AwcStreamFormat(int BlockCount, int BlockSize, IReadOnlyList<AwcChannel> Channels)
{
    private const int HeaderSize = 12;
    private const int ChannelSize = 16;
    private const int BlockSizeOffset = 4;
    private const int ChannelCountOffset = 8;
    private const int SamplesOffset = 4;
    private const int HeadroomOffset = 8;
    private const int RateOffset = 10;
    private const int CodecOffset = 12;
    private const int MaxChannels = 64;

    /// <summary>Reads the chunk, or null when its sizes do not add up.</summary>
    public static AwcStreamFormat? Read(ReadOnlySpan<byte> chunk)
    {
        if (chunk.Length < HeaderSize)
        {
            return null;
        }

        var blockCount = BinaryPrimitives.ReadInt32LittleEndian(chunk);
        var blockSize = BinaryPrimitives.ReadInt32LittleEndian(chunk[BlockSizeOffset..]);
        var channelCount = BinaryPrimitives.ReadInt32LittleEndian(chunk[ChannelCountOffset..]);

        if (blockCount <= 0 || blockSize <= 0 || channelCount <= 0 || channelCount > MaxChannels)
        {
            return null;
        }

        if (chunk.Length < HeaderSize + (channelCount * ChannelSize))
        {
            return null;
        }

        var channels = new AwcChannel[channelCount];

        for (var index = 0; index < channelCount; index++)
        {
            var entry = chunk[(HeaderSize + (index * ChannelSize))..];
            var format = new AwcFormat(
                BinaryPrimitives.ReadUInt32LittleEndian(entry[SamplesOffset..]),
                BinaryPrimitives.ReadUInt16LittleEndian(entry[RateOffset..]),
                BinaryPrimitives.ReadInt16LittleEndian(entry[HeadroomOffset..]),
                entry[CodecOffset],
                entry[CodecOffset + 1]);
            channels[index] = new AwcChannel(BinaryPrimitives.ReadUInt32LittleEndian(entry), format);
        }

        return new AwcStreamFormat(blockCount, blockSize, channels);
    }
}
