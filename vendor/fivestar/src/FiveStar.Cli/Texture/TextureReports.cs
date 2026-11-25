// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Cli.Archive;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf;

namespace FiveStar.Cli.Texture;

/// <summary>The texture commands: DDS extraction, coverage and storage census.</summary>
internal sealed class TextureReports
{
    private const int FileNameWidth = 36;
    private const int CountWidth = 6;
    private const int PercentWidth = 5;
    private const int StatusWidth = 28;
    private const int TotalWidth = 7;
    private const int GibWidth = 8;
    private const int PathWidth = 70;
    private const int BlockersShown = 12;
    private const int TwoDecimals = 2;
    private const int IndexDigits = 5;
    private const int HashDigits = 16;
    private const int MebibyteShift = 20;
    private const double BytesPerGibibyte = 1 << 30;
    private const double Percent = 100;

    private readonly TextWriter _output;

    public TextureReports(TextWriter output)
    {
        _output = output;
    }

    public void Extract(string path, string outputDirectory, CommandArguments arguments)
    {
        Directory.CreateDirectory(outputDirectory);
        using var source = new MappedFileSource(path);
        var file = new TexturePageFile(source, new ZlibInflater());
        var extractor = new TextureExtractor(file);

        var only = ParseIndices(arguments.Option("--only"));
        var limit = arguments.LongOption("--limit", 0);
        var writeDds = !arguments.HasFlag("--no-dds");
        var statuses = new Tally<string>();

        foreach (var descriptor in file.Descriptors)
        {
            if (only.Count > 0 && !only.Contains(descriptor.Index))
            {
                continue;
            }

            if (limit > 0 && statuses["ok"] >= limit)
            {
                break;
            }

            var (status, data, layout) = extractor.ReadMip0(descriptor);
            statuses.Add(status.Name);

            if (data is null || layout is null || !writeDds)
            {
                continue;
            }

            var dds = DdsWriter.Build(descriptor.Width, descriptor.Height, descriptor.Format, data, layout);
            File.WriteAllBytes(Path.Combine(outputDirectory, TextureName(file, descriptor, layout) + ".dds"), dds);
        }

        _output.WriteLine($"{Path.GetFileName(path)} {ReportText.CountMap(statuses)}");
    }

    public void Coverage(string root)
    {
        var textures = new Tally<string>();
        var spanBytes = new Tally<string>();
        var blockers = new Tally<(int Codec, string Format)>();

        foreach (var path in PageFiles(root))
        {
            using var source = new MappedFileSource(path);
            var file = new TexturePageFile(source, new ZlibInflater());
            var report = new TextureCoverage(file, new TextureExtractor(file)).Measure();

            PrintFileCoverage(path, report);
            textures.AddAll(report.Textures);
            spanBytes.AddAll(report.SpanBytes);
            blockers.AddAll(report.Blockers);
            _output.Flush();
        }

        PrintTotals(textures, spanBytes, blockers);
    }

    public void Codecs(string root)
    {
        var pages = new Tally<string>();
        var bytes = new Tally<string>();

        foreach (var path in PageFiles(root))
        {
            using var source = new MappedFileSource(path);
            var survey = new PageTableSurvey(source, new ZlibInflater());
            var file = new TexturePageFile(source, new ZlibInflater());
            var (filePages, fileBytes) = survey.StorageCensus(file);

            var kinds = new List<string>(filePages.Keys);
            kinds.Sort(StringComparer.Ordinal);
            var parts = new List<string>();

            foreach (var kind in kinds)
            {
                parts.Add($"{kind}:{filePages[kind]}/{fileBytes[kind] >> MebibyteShift}MB");
            }

            var name = Path.GetRelativePath(root, path);
            _output.WriteLine($"{name,-PathWidth} pages={survey.Entries().Count,CountWidth} {string.Join(' ', parts)}");
            pages.AddAll(filePages);
            bytes.AddAll(fileBytes);
        }

        var all = new List<string>(pages.Keys);
        all.Sort(StringComparer.Ordinal);
        var totals = new List<string>();

        foreach (var kind in all)
        {
            totals.Add($"{kind}:{pages[kind]} pages {DecimalText.Fixed(bytes[kind] / BytesPerGibibyte, TwoDecimals)}GB");
        }

        _output.WriteLine("TOTAL " + string.Join(' ', totals));
    }

    /// <summary>Every <c>.ttpf</c> below a folder, in ordinal path order.</summary>
    public static List<string> PageFiles(string root)
    {
        var files = new List<string>(Directory.GetFiles(root, "*.ttpf", SearchOption.AllDirectories));
        files.Sort(StringComparer.Ordinal);

        return files;
    }

    private void PrintFileCoverage(string path, CoverageReport report)
    {
        var count = report.Textures.Total;
        var ok = DecimalText.Fixed(report.Textures["ok"] / (double)count * Percent, 1);
        var bytesOk = DecimalText.Fixed(report.SpanBytes["ok"] / (double)Math.Max(report.SpanBytes.Total, 1) * Percent, 1);
        var others = new List<string>();

        foreach (var entry in report.Textures.MostCommon())
        {
            if (entry.Key != "ok")
            {
                others.Add($"{entry.Key}={entry.Value}");
            }
        }

        _output.WriteLine($"{Path.GetFileName(path),-FileNameWidth} {count,CountWidth} ok {ok,PercentWidth}% bytes-ok {bytesOk,PercentWidth}%  {string.Join(' ', others)}");
    }

    private void PrintTotals(Tally<string> textures, Tally<string> spanBytes, Tally<(int Codec, string Format)> blockers)
    {
        var count = textures.Total;
        var bytes = spanBytes.Total;
        var okShare = DecimalText.Fixed(textures["ok"] / (double)count * Percent, TwoDecimals);
        var okGib = DecimalText.Fixed(spanBytes["ok"] / BytesPerGibibyte, TwoDecimals);
        var allGib = DecimalText.Fixed(bytes / BytesPerGibibyte, TwoDecimals);
        var bytesShare = DecimalText.Fixed(spanBytes["ok"] / (double)bytes * Percent, TwoDecimals);

        _output.WriteLine($"TOTAL textures {count}: ok {textures["ok"]} ({okShare}%), span bytes ok {okGib} / {allGib} GiB ({bytesShare}%); approx_ statuses decode through fitted BCPack tables");

        foreach (var entry in textures.MostCommon())
        {
            var gib = DecimalText.Fixed(spanBytes[entry.Key] / BytesPerGibibyte, TwoDecimals);
            _output.WriteLine($"  {entry.Key,-StatusWidth} {entry.Value,TotalWidth} {gib,GibWidth} GiB");
        }

        var parts = new List<string>();

        foreach (var entry in blockers.MostCommon(BlockersShown))
        {
            parts.Add($"(({entry.Key.Codec}, {ReportText.Quote(entry.Key.Format)}), {entry.Value})");
        }

        _output.WriteLine($"blocked by codec/format: [{string.Join(", ", parts)}]");
    }

    private static string TextureName(TexturePageFile file, TextureDescriptor descriptor, TextureLayout layout)
    {
        var index = descriptor.Index.ToString("D" + IndexDigits.ToString(CultureInfo.InvariantCulture), CultureInfo.InvariantCulture);
        var hash = ReportText.Hex((long)file.Hashes[descriptor.Index], HashDigits);

        return $"{index}_{hash}_{descriptor.Width}x{descriptor.Height}_{layout.Format.Name}";
    }

    private static HashSet<int> ParseIndices(string? list)
    {
        var indices = new HashSet<int>();

        if (list is null)
        {
            return indices;
        }

        foreach (var part in list.Split(',', StringSplitOptions.RemoveEmptyEntries))
        {
            indices.Add(int.Parse(part, CultureInfo.InvariantCulture));
        }

        return indices;
    }
}
