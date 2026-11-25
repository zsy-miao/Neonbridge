// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Tests.Support;

/// <summary>Reads the packets back out of an Ogg file (RFC 3533): each page's lacing values join segments into packets.</summary>
internal static class OggPackets
{
    private const int SegmentCountOffset = 26;
    private const int LacingOffset = 27;
    private const int FullSegment = 255;

    public static List<byte[]> Read(byte[] file)
    {
        var packets = new List<byte[]>();
        var current = new List<byte>();
        int position = 0;

        while (position < file.Length)
        {
            int segments = file[position + SegmentCountOffset];
            int body = position + LacingOffset + segments;

            for (int index = 0; index < segments; index++)
            {
                int size = file[position + LacingOffset + index];
                current.AddRange(file.AsSpan(body, size).ToArray());
                body += size;

                if (size < FullSegment)
                {
                    packets.Add([.. current]);
                    current.Clear();
                }
            }

            position = body;
        }

        return packets;
    }
}
