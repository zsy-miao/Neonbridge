// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Audio;

/// <summary>
/// The fields of a stream format both layouts share. A <see cref="AwcChunkTag.Format"/> chunk is 0x20 bytes: u32
/// samples, i32 loop point, u16 sample rate, i16 headroom, unknown fields, the codec byte at +0x1C and the channel
/// count at +0x1D.
/// </summary>
/// <param name="Samples">Decoded samples of all channels together: frames times <paramref name="Channels"/>.</param>
/// <param name="SampleRate">Sample rate in Hz.</param>
/// <param name="Headroom">Headroom in hundredths of a decibel.</param>
/// <param name="Codec">The codec byte; see <see cref="AwcCodec"/>.</param>
/// <param name="Channels">Channel count (1 or 2).</param>
public sealed record AwcFormat(uint Samples, uint SampleRate, short Headroom, byte Codec, byte Channels)
{
    /// <summary>Size of a single-stream format chunk.</summary>
    public const int ChunkSize = 0x20;

    private const int RateOffset = 8;
    private const int HeadroomOffset = 10;
    private const int CodecOffset = 0x1C;
    private const int ChannelsOffset = 0x1D;

    /// <summary>Gets the samples per channel.</summary>
    public uint Frames => Samples / Math.Max((uint)Channels, 1u);

    /// <summary>Reads a <see cref="AwcChunkTag.Format"/> chunk, or null when it is too short or has no sample rate.</summary>
    public static AwcFormat? Read(ReadOnlySpan<byte> chunk)
    {
        if (chunk.Length < ChunkSize)
        {
            return null;
        }

        var rate = BinaryPrimitives.ReadUInt16LittleEndian(chunk[RateOffset..]);

        if (rate == 0)
        {
            return null;
        }

        return new AwcFormat(
            BinaryPrimitives.ReadUInt32LittleEndian(chunk),
            rate,
            BinaryPrimitives.ReadInt16LittleEndian(chunk[HeadroomOffset..]),
            chunk[CodecOffset],
            chunk[ChannelsOffset]);
    }
}
