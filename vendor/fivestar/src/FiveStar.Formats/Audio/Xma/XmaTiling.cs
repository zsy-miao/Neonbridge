// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>
/// Reads how a frame splits into subframes per channel. Either all channels share one tiling (a leading 1 bit), or the
/// tiling is built one length at a time: each step names the channels, among those with the fewest samples so far,
/// that take the next subframe length.
/// </summary>
internal static class XmaTiling
{
    /// <summary>A subframe length code: 0 is a whole frame, 1 then one more bit picks a half (0) or a quarter (1).</summary>
    private const int ShortLengthBits = XmaLayout.SubframeCountLog2 - 1;

    /// <summary>Reads the tiling into the channels; false when the frame is broken.</summary>
    [HotPath]
    public static bool Read(ref XmaBitReader reader, XmaChannel[] channels, int count)
    {
        Span<bool> takes = stackalloc bool[XmaLayout.MaxChannels];
        Span<int> filled = stackalloc int[XmaLayout.MaxChannels];
        var shared = reader.ReadBit();
        var waiting = count;
        var least = 0;

        for (var index = 0; index < count; index++)
        {
            channels[index].SubframeCount = 0;
        }

        while (least < XmaLayout.FrameSamples)
        {
            ChooseChannels(ref reader, takes, filled, count, least, shared || waiting == 1);

            var length = ReadLength(ref reader, least);

            if (length == 0)
            {
                return false;
            }

            least += length;

            if (!Assign(channels, takes, filled, count, length, ref least, ref waiting))
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>Marks the channels at the fewest samples that take the next subframe.</summary>
    [HotPath]
    private static void ChooseChannels(ref XmaBitReader reader, scoped Span<bool> takes, scoped Span<int> filled, int count, int least, bool all)
    {
        var lastSlot = least == XmaLayout.FrameSamples - XmaLayout.MinSubframeSamples;

        for (var index = 0; index < count; index++)
        {
            if (filled[index] != least)
            {
                takes[index] = false;
            }
            else if (all || lastSlot)
            {
                takes[index] = true;
            }
            else
            {
                takes[index] = reader.ReadBit();
            }
        }
    }

    /// <summary>
    /// Adds the subframe to the chosen channels and finds the new fewest-samples count. <paramref name="waiting"/> counts
    /// the channels at that count: the chosen ones are already in it, unless a channel that was left out is now behind.
    /// </summary>
    [HotPath]
    private static bool Assign(XmaChannel[] channels, Span<bool> takes, Span<int> filled, int count, int length, ref int least, ref int waiting)
    {
        for (var index = 0; index < count; index++)
        {
            var channel = channels[index];

            if (takes[index])
            {
                if (channel.SubframeCount == XmaLayout.MaxSubframes || filled[index] + length > XmaLayout.FrameSamples)
                {
                    return false;
                }

                channel.SubframeLengths[channel.SubframeCount++] = length;
                filled[index] += length;
            }
            else if (filled[index] <= least)
            {
                if (filled[index] < least)
                {
                    least = filled[index];
                    waiting = 0;
                }

                waiting++;
            }
        }

        return true;
    }

    /// <summary>The next subframe length; the last quarter of a frame needs no bits. Returns 0 when out of range.</summary>
    [HotPath]
    private static int ReadLength(ref XmaBitReader reader, int offset)
    {
        if (offset == XmaLayout.FrameSamples - XmaLayout.MinSubframeSamples)
        {
            return XmaLayout.MinSubframeSamples;
        }

        var shift = 0;

        if (reader.ReadBit())
        {
            shift = 1 + (int)reader.Read(ShortLengthBits);
        }

        return XmaLayout.FrameSamples >> shift;
    }
}
