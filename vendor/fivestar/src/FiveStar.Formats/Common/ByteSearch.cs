// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Common;

/// <summary>Pattern search over a byte source, window by window.</summary>
public static class ByteSearch
{
    private const int WindowSize = 64 << 20;

    /// <summary>The first offset in [<paramref name="start"/>, <paramref name="end"/>) where the whole pattern sits, or -1.</summary>
    public static long IndexOf(IByteSource source, ReadOnlySpan<byte> pattern, long start, long end)
    {
        end = Math.Min(end, source.Length);
        var position = start;

        while (position + pattern.Length <= end)
        {
            var take = (int)Math.Min(WindowSize, end - position);
            var window = source.Read(position, take);
            var hit = window.IndexOf(pattern);

            if (hit >= 0)
            {
                return position + hit;
            }

            if (position + take >= end)
            {
                break;
            }

            position += take - pattern.Length + 1;
        }

        return -1;
    }
}
