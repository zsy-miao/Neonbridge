// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Audio.Xma;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>XMA2 packets built bit by bit: the packet header, and silent frames that run across a packet boundary.</summary>
public sealed class XmaTests
{
    private const int PacketBits = 2048 * 8;
    private const int PayloadBits = PacketBits - 32;

    [Fact]
    public void A_packet_header_splits_into_its_four_fields()
    {
        var packet = new BitWriter().Bits(13, 6).Bits(0x1234, 15).Bits(5, 3).Bits(200, 8).ToPacket();

        Assert.Equal(new XmaPacketHeader(13, 0x1234, 5, 200), XmaPacketHeader.Read(packet));
    }

    [Fact]
    public void Frames_decode_to_silence_and_a_frame_cut_by_the_packet_end_finishes_in_the_next_packet()
    {
        const int firstLength = 16_000;
        const int splitLength = 1_000;
        var carried = splitLength - (PayloadBits - firstLength);

        var first = new BitWriter().Bits(2, 6).Bits(0, 15).Bits(0, 3).Bits(0, 8);
        SilentFrame(first, firstLength, more: true);
        SilentFrame(first, splitLength, more: true);

        var second = new BitWriter().Bits(2, 6).Bits((uint)carried, 15).Bits(0, 3).Bits(0, 8);
        second.Append(first.Overflow(PacketBits));
        SilentFrame(second, 64, more: false);

        var decoder = new XmaStreamDecoder(1, 32_000);
        var output = new float[decoder.MaxPacketOutput];
        output.AsSpan().Fill(1f);

        Assert.Equal(1, decoder.DecodePacket(first.ToPacket(), output));
        Assert.Equal(2, decoder.DecodePacket(second.ToPacket(), output));
        Assert.Equal(3, decoder.Frames);
        Assert.Equal(0, decoder.BadFrames);
        Assert.Equal(0, decoder.LostFrames);
        Assert.All(output.AsSpan(0, 2 * 512).ToArray(), sample => Assert.Equal(0f, sample));
    }

    /// <summary>
    /// A mono frame of one 512-sample subframe that codes no coefficients, padded to <paramref name="length"/> bits
    /// with the subframe's fill bits: length (15), shared tiling (1), whole-frame subframe (1), dynamic range (8), no
    /// trim (1), fill present (1), fill size form (2 + 4 + 15 bits), the fill, reserved (1), no coefficients (1), then
    /// one spare bit and the more-frames bit.
    /// </summary>
    private static void SilentFrame(BitWriter writer, int length, bool more)
    {
        const int fixedBits = 15 + 1 + 1 + 8 + 1 + 1 + 2 + 4 + 15 + 1 + 1 + 2;
        var fill = length - fixedBits;

        writer.Bits((uint)length, 15).Bits(1, 1).Bits(0, 1).Bits(0x80, 8).Bits(0, 1);
        writer.Bits(1, 1).Bits(0, 2).Bits(15, 4).Bits((uint)(fill - 1), 15).Zeros(fill);
        writer.Bits(0, 1).Bits(0, 1).Bits(0, 1).Bit(more);
    }

    private sealed class BitWriter
    {
        private readonly List<bool> _bits = [];

        public BitWriter Bits(uint value, int count)
        {
            for (var bit = count - 1; bit >= 0; bit--)
            {
                _bits.Add(((value >> bit) & 1) != 0);
            }

            return this;
        }

        public BitWriter Bit(bool value)
        {
            _bits.Add(value);

            return this;
        }

        public BitWriter Zeros(int count)
        {
            _bits.AddRange(Enumerable.Repeat(false, count));

            return this;
        }

        public void Append(IEnumerable<bool> bits) => _bits.AddRange(bits);

        /// <summary>The bits past one packet; they continue in the next packet.</summary>
        public IEnumerable<bool> Overflow(int packetBits) => _bits.Skip(packetBits).ToList();

        /// <summary>The first packet's bits, padded with ones (the frame length 0x7FFF ends a packet).</summary>
        public byte[] ToPacket()
        {
            var packet = new byte[PacketBits / 8];

            for (var index = 0; index < PacketBits; index++)
            {
                if (index >= _bits.Count || _bits[index])
                {
                    packet[index / 8] |= (byte)(0x80 >> (index % 8));
                }
            }

            return packet;
        }
    }
}
