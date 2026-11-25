// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Mono XMA2 packets of silent frames built bit by bit: per packet a header (frame count, first frame offset 0) and
/// <see cref="FramesPerPacket"/> frames of <see cref="FrameBits"/> bits, the last one ending the packet's frames.
/// </summary>
internal static class SilentXmaPackets
{
    public const int FramesPerPacket = 4;
    public const int FrameBits = 4000;

    private const int PacketBits = 2048 * 8;

    public static byte[] Build(int packets)
    {
        var data = new byte[packets * PacketBits / 8];

        for (int packet = 0; packet < packets; packet++)
        {
            var bits = new List<bool>();
            Write(bits, FramesPerPacket, 6);
            Write(bits, 0, 15 + 3 + 8);

            for (int frame = 0; frame < FramesPerPacket; frame++)
            {
                SilentFrame(bits, frame < FramesPerPacket - 1);
            }

            // Pad with ones: a frame length of 0x7FFF ends the packet.
            for (int index = 0; index < PacketBits; index++)
            {
                if (index >= bits.Count || bits[index])
                {
                    data[(packet * PacketBits / 8) + (index / 8)] |= (byte)(0x80 >> (index % 8));
                }
            }
        }

        return data;
    }

    /// <summary>
    /// One 512-sample subframe that codes no coefficients, padded with fill bits: length (15), shared tiling (1),
    /// whole-frame subframe (1), dynamic range (8), no trim (1), fill present (1), fill size form (2 + 4 + 15), the fill,
    /// reserved (1), no coefficients (1), the spare bit and the more-frames bit.
    /// </summary>
    private static void SilentFrame(List<bool> bits, bool more)
    {
        const int fixedBits = 15 + 1 + 1 + 8 + 1 + 1 + 2 + 4 + 15 + 1 + 1 + 2;
        int fill = FrameBits - fixedBits;

        Write(bits, FrameBits, 15);
        Write(bits, 0b10, 2);
        Write(bits, 0x80, 8);
        Write(bits, 0b0100, 4);
        Write(bits, 15, 4);
        Write(bits, (uint)(fill - 1), 15);

        bits.AddRange(Enumerable.Repeat(false, fill));
        Write(bits, 0, 3);
        bits.Add(more);
    }

    private static void Write(List<bool> bits, uint value, int count)
    {
        for (int bit = count - 1; bit >= 0; bit--)
        {
            bits.Add(((value >> bit) & 1) != 0);
        }
    }
}
