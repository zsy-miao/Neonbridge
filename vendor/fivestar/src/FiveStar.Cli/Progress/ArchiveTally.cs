// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Rpf;

namespace FiveStar.Cli.Progress;

/// <summary>
/// Archive bytes readable without keys, per <c>.rpf</c> of a dump in GiB: the sampled share inside complete zlib
/// streams, or the share inside raw Bink 2 videos when that is larger. Archives below a size threshold share one group.
/// </summary>
internal static class ArchiveTally
{
    public const string Key = "archives";

    private const string Title = "Archive bytes readable without keys (sampled; table of contents still encrypted)";
    private const string Unit = " GiB";
    private const double Height = 130;
    private const int Samples = 24;
    private const double BytesPerGibibyte = 1 << 30;
    private const double SmallArchiveGibibytes = 0.2;
    private const int Decimals = 2;

    // Bink scanning reads the whole file, so it only runs where sampling found less than this share of zlib.
    private const double BinkScanBelow = 0.5;

    public static ProgressArea Measure(string dump, TextWriter log)
    {
        var files = Directory.GetFiles(dump, "*.rpf", SearchOption.AllDirectories)
            .Select(path => new FileInfo(path))
            .Where(file => file.Length > 0)
            .OrderByDescending(file => file.Length)
            .ToList();
        var groups = new List<ProgressGroup>();
        double smallDone = 0;
        double smallSize = 0;
        var smallCount = 0;

        foreach (var file in files)
        {
            var (readable, note) = Readable(file.FullName);
            var size = file.Length / BytesPerGibibyte;
            log.WriteLine($"archives: {file.Name} {readable:0.000}");

            if (size < SmallArchiveGibibytes)
            {
                smallDone += size * readable;
                smallSize += size;
                smallCount++;

                continue;
            }

            groups.Add(Group(file.Name, size, size * readable, note));
        }

        if (smallCount > 0)
        {
            groups.Add(Group($"other ({smallCount} small)", smallSize, smallDone, null));
        }

        return new ProgressArea(Key, Title, Unit, Height, groups);
    }

    private static ProgressGroup Group(string name, double size, double readable, string? note)
    {
        var done = Math.Round(readable, Decimals);

        return new ProgressGroup(name, done, 0, Math.Round(size - done, Decimals), note);
    }

    private static (double Readable, string? Note) Readable(string path)
    {
        using var source = new MappedFileSource(path);
        var zlib = new ZlibSampler(source, new ZlibStreamProbe(source, new ZlibInflater())).Sample(Samples).CoveredFraction;

        if (zlib >= BinkScanBelow)
        {
            return (zlib, null);
        }

        long videoBytes = 0;
        var videos = 0;

        foreach (var video in new BinkScanner(source).Scan())
        {
            videoBytes += video.Size;
            videos++;
        }

        var bink = (double)videoBytes / source.Length;

        if (bink <= zlib)
        {
            return (zlib, null);
        }

        return (bink, $"{videos} raw Bink 2 videos");
    }
}
