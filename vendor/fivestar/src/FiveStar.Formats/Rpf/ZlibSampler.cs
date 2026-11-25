// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Formats.Rpf;

/// <summary>
/// Estimates how much of an archive is readable zlib by inflating every stream in evenly spread 8 MiB windows
/// (window starts aligned to 4 KiB, the first at 0 and the last at the end).
/// </summary>
public sealed class ZlibSampler
{
    private const int WindowSize = 8 << 20;
    private const int WindowAlignment = 4096;

    private readonly IByteSource _source;
    private readonly ZlibStreamProbe _probe;

    public ZlibSampler(IByteSource source, ZlibStreamProbe probe)
    {
        _source = source;
        _probe = probe;
    }

    public ZlibSampleReport Sample(int samples)
    {
        var size = _source.Length;
        var count = (int)Math.Max(1, Math.Min(samples, size / WindowSize));
        long covered = 0;
        long total = 0;
        var kinds = new Tally<BlobKind>();

        for (var index = 0; index < count; index++)
        {
            long start = 0;

            if (size > WindowSize)
            {
                start = (size - WindowSize) * index / Math.Max(1, count - 1) / WindowAlignment * WindowAlignment;
            }

            var end = Math.Min(size, start + WindowSize);
            total += end - start;
            covered += SampleWindow(start, end, kinds);
        }

        var fraction = total == 0 ? 0.0 : (double)covered / total;

        return new ZlibSampleReport(fraction, kinds);
    }

    private long SampleWindow(long start, long end, Tally<BlobKind> kinds)
    {
        long covered = 0;
        var position = ZlibStreamProbe.Align(start);

        while (position < end)
        {
            var found = _probe.NextHeader(position, end);

            if (found < 0)
            {
                break;
            }

            if (!_probe.TryInflate(found, ZlibStreamProbe.MaxStreamOutput, out var used))
            {
                position = found + 1;

                continue;
            }

            covered += Math.Min(used, end - found);
            kinds.Add(BlobClassifier.Classify(_probe.Output));
            position = found + used;
        }

        return covered;
    }
}
