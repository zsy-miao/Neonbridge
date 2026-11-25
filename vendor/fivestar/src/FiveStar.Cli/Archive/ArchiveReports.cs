// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Rpf;

namespace FiveStar.Cli.Archive;

/// <summary>The archive survey commands: header tables, zlib sampling, entry carving and Bink carving.</summary>
internal sealed class ArchiveReports
{
    private const int NameWidth = 42;
    private const int RelativeNameWidth = 55;
    private const int SizeWidth = 12;
    private const int HexWidth = 7;
    private const int CountWidth = 5;
    private const int FirstDataWidth = 10;
    private const int RatioWidth = 8;
    private const int WordDigits = 8;
    private const int OffsetDigits = 10;
    private const int PrintedWords = 12;
    private const int FirstDataProbe = 0x20000;
    private const int EntropyProbe = 0x1000;
    private const uint LowHalfMask = 0xFFFF;
    private const double BytesPerMebibyte = 1 << 20;
    private const double BytesPerGibibyte = 1 << 30;
    private const double Percent = 100;
    private const int DefaultSamples = 24;
    private const int BinkSizeWidth = 11;
    private const int BinkOffsetDigits = 10;
    private const int FpsDecimals = 3;
    private const int TwoDecimals = 2;
    private const int KindsShown = 8;

    private readonly TextWriter _output;

    public ArchiveReports(TextWriter output)
    {
        _output = output;
    }

    /// <summary>One row of header fields per archive, smallest archive first.</summary>
    public void Headers(string root)
    {
        var files = ArchiveFiles(root);
        files.Sort((left, right) => new FileInfo(left).Length.CompareTo(new FileInfo(right).Length));
        _output.WriteLine(
            $"{"file",-NameWidth} {"size",SizeWidth} {"u04",HexWidth} {"u08",HexWidth} {"u0c",HexWidth} {"cnt",CountWidth} {"u20",CountWidth} keyid    u2c      firstdata  u0c/cnt  u08/cnt");

        foreach (var path in files)
        {
            using var source = new MappedFileSource(path);
            var header = RpfHeader.Read(source.Read(0, RpfHeader.Size));
            var firstData = FirstNonZero(source, header.HeaderEnd);
            var first = firstData.HasValue ? DecimalText.Hex(firstData.Value) : "None";
            var nameRatio = DecimalText.Fixed((double)header.NameTableSize / header.EntryCount, 1);
            var tocRatio = DecimalText.Fixed((double)header.TocSize / header.EntryCount, 1);

            _output.WriteLine(
                $"{Path.GetFileName(path),-NameWidth} {source.Length,SizeWidth} {DecimalText.Hex(header.HeaderEnd),HexWidth} {DecimalText.Hex(header.TocSize),HexWidth} {DecimalText.Hex(header.NameTableSize),HexWidth} {header.EntryCount,CountWidth} {header.Unknown20 & LowHalfMask,CountWidth} {ReportText.Hex(header.KeySelector, WordDigits)} {ReportText.Hex(header.Version, WordDigits)} {first,FirstDataWidth} {nameRatio,RatioWidth} {tocRatio,RatioWidth}");
        }
    }

    /// <summary>The first twelve header words and the entropy of the next 4 KiB, per archive in path order.</summary>
    public void Words(string root)
    {
        var files = ArchiveFiles(root);
        files.Sort(StringComparer.Ordinal);

        foreach (var path in files)
        {
            using var source = new MappedFileSource(path);
            var header = RpfHeader.Read(source.Read(0, RpfHeader.Size));
            var words = new List<string>(PrintedWords);

            for (var index = 0; index < PrintedWords; index++)
            {
                words.Add(ReportText.Hex(header.Words[index], WordDigits));
            }

            var entropy = ByteEntropy.BitsPerByte(source.Read(RpfHeader.Size, EntropyProbe));
            var name = Path.GetRelativePath(root, path);
            _output.WriteLine($"{name,-RelativeNameWidth} size={source.Length,SizeWidth} {string.Join(' ', words)} ent(0x40..)={DecimalText.Fixed(entropy, TwoDecimals)}");
        }
    }

    public void Stats(string path, CommandArguments arguments)
    {
        using var source = new MappedFileSource(path);
        var sampler = new ZlibSampler(source, new ZlibStreamProbe(source, new ZlibInflater()));
        var report = sampler.Sample((int)arguments.LongOption("--samples", DefaultSamples));
        var kinds = new List<KeyValuePair<string, long>>();

        foreach (var entry in report.Kinds.MostCommon(KindsShown))
        {
            kinds.Add(new KeyValuePair<string, long>(BlobClassifier.Tag(entry.Key), entry.Value));
        }

        var size = DecimalText.Fixed(source.Length / BytesPerGibibyte, TwoDecimals);
        var share = DecimalText.Fixed(report.CoveredFraction * Percent, 1);
        _output.WriteLine($"{Path.GetFileName(path)} {size} GiB: ~{share}% of sampled bytes are plain zlib streams; kinds {ReportText.CountMap(kinds)}");
    }

    public void Carve(string path, string outputDirectory, CommandArguments arguments)
    {
        Directory.CreateDirectory(outputDirectory);
        using var source = new MappedFileSource(path);
        var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));
        var limit = arguments.LongOption("--limit-mb", 0) * (long)BytesPerMebibyte;

        long count = 0;
        long total = 0;
        long raw = 0;

        foreach (var blob in carver.Carve(limit))
        {
            var name = $"{ReportText.Hex(blob.Offset, OffsetDigits)}.{BlobClassifier.Tag(blob.Kind)}.bin";
            File.WriteAllBytes(Path.Combine(outputDirectory, name), blob.Data);
            count++;
            total += blob.Data.Length;
            raw += blob.RawChunks;
        }

        _output.WriteLine($"{count} blobs, {DecimalText.Fixed(total / BytesPerMebibyte, 1)} MiB inflated, {raw} raw chunks");
    }

    public void Bink(string path, string outputDirectory, CommandArguments arguments)
    {
        using var source = new MappedFileSource(path);
        var listOnly = arguments.HasFlag("--list");
        long count = 0;
        long total = 0;

        foreach (var video in new BinkScanner(source).Scan())
        {
            count++;
            total += video.Size;
            var fps = DecimalText.Fixed(video.FramesPerSecond, FpsDecimals);
            _output.WriteLine($"0x{ReportText.Hex(video.Offset, BinkOffsetDigits)} {video.Size,BinkSizeWidth} KB2{video.Revision} {video.Width}x{video.Height} {video.Frames} frames @ {fps} fps");

            if (!listOnly)
            {
                Directory.CreateDirectory(outputDirectory);
                var name = $"movie_{ReportText.Hex(video.Offset, OffsetDigits)}.bk2";
                File.WriteAllBytes(Path.Combine(outputDirectory, name), source.Read(video.Offset, (int)video.Size).ToArray());
            }
        }

        _output.WriteLine($"{count} videos, {DecimalText.Fixed(total / BytesPerGibibyte, TwoDecimals)} GiB of {DecimalText.Fixed(source.Length / BytesPerGibibyte, TwoDecimals)} GiB archive");
    }

    private static List<string> ArchiveFiles(string root)
    {
        var files = new List<string>(Directory.GetFiles(root, "*.rpf"));
        var packs = Path.Combine(root, "pack");

        if (Directory.Exists(packs))
        {
            foreach (var pack in Directory.GetDirectories(packs))
            {
                files.AddRange(Directory.GetFiles(pack, "*.rpf"));
            }
        }

        return files;
    }

    private static long? FirstNonZero(IByteSource source, long offset)
    {
        var tail = source.Read(offset, FirstDataProbe);
        var index = tail.IndexOfAnyExcept((byte)0);

        if (index < 0)
        {
            return null;
        }

        return offset + index;
    }
}
