// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Cli.Archive;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Models;
using FiveStar.Formats.Rpf;

namespace FiveStar.Cli.Model;

/// <summary>The <c>fivestar model</c> reports: drawable coverage of an archive and OBJ export.</summary>
internal sealed class ModelReports
{
    private const long BytesPerMebibyte = 1 << 20;
    private const int OffsetDigits = 10;
    private const int ReasonsShown = 12;
    private const int ReasonLength = 48;
    private const int CountWidth = 6;
    private const double Percent = 100;
    private const string HexPrefix = "0x";

    private readonly TextWriter _output;

    public ModelReports(TextWriter output)
    {
        _output = output;
    }

    /// <summary>Counts drawables and geometries that decode, per level of detail, and the reasons the rest do not.</summary>
    public void Scan(string path, CommandArguments arguments)
    {
        using var source = new WindowedFileSource(path, WindowedFileSource.DefaultWindowBytes);
        var tally = new ModelTally();
        var list = arguments.HasFlag("--list");

        foreach (var carved in Drawables(source, arguments))
        {
            tally.Add(carved);

            if (list)
            {
                var lod0 = carved.Drawable.Lod(0);
                _output.WriteLine($"{carved.Offset:x10} {carved.Drawable.Name} lod0 {lod0.Count} geometries, {lod0.Sum(geometry => geometry.TriangleCount)} triangles, {carved.Drawable.Problems.Count} problems");
            }
        }

        Summary(tally);
    }

    /// <summary>Writes one OBJ per drawable (one level of detail) whose name contains the --name text.</summary>
    public void Export(string path, string outputDirectory, CommandArguments arguments)
    {
        using var source = new WindowedFileSource(path, WindowedFileSource.DefaultWindowBytes);
        var filter = arguments.Option("--name") ?? string.Empty;
        var lod = (int)arguments.LongOption("--lod", 0);
        var written = 0;
        Directory.CreateDirectory(outputDirectory);

        foreach (var carved in Drawables(source, arguments))
        {
            var geometries = carved.Drawable.Lod(lod);

            if (geometries.Count == 0 || !carved.Drawable.Name.Contains(filter, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            var file = Path.Combine(outputDirectory, $"{carved.Offset.ToString("x" + OffsetDigits, CultureInfo.InvariantCulture)}_{FileName(carved.Drawable.Name)}.obj");
            using var writer = new StreamWriter(file);
            ObjWriter.Write(writer, carved.Drawable, geometries);
            _output.WriteLine($"{file}: {geometries.Count} geometries, {geometries.Sum(geometry => geometry.VertexCount)} vertices, {geometries.Sum(geometry => geometry.TriangleCount)} triangles");
            written++;
        }

        _output.WriteLine($"{written} OBJ files");
    }

    /// <summary>
    /// The drawables of the archive from --from (hex offset of an entry) up to --limit-mb, carved one at a time: the
    /// source may be a <see cref="WindowedFileSource"/>, whose spans last only until its next read.
    /// </summary>
    public static IEnumerable<CarvedDrawable> Drawables(IByteSource source, CommandArguments arguments)
    {
        var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));
        var from = HexOption(arguments.Option("--from"));
        var limit = arguments.LongOption("--limit-mb", 0) * BytesPerMebibyte;

        return DrawableScanner.Scan(carver.CarveFrom(from, limit));
    }

    public static long HexOption(string? value)
    {
        if (value is null)
        {
            return 0;
        }

        var digits = value;

        if (digits.StartsWith(HexPrefix, StringComparison.OrdinalIgnoreCase))
        {
            digits = digits[HexPrefix.Length..];
        }

        return long.Parse(digits, NumberStyles.HexNumber, CultureInfo.InvariantCulture);
    }

    private void Summary(ModelTally tally)
    {
        _output.WriteLine($"resources with drawables: {tally.Resources} ({tally.WithoutPhysical} without a physical segment)");
        _output.WriteLine($"drawables: {tally.Drawables}, every geometry decoded: {tally.CompleteDrawables} ({Share(tally.CompleteDrawables, tally.Drawables)})");

        for (var lod = 0; lod < DrawableReader.LevelsOfDetail; lod++)
        {
            var decoded = tally.Decoded[lod];
            var total = decoded + tally.Failed[lod];
            _output.WriteLine($"lod {lod}: {decoded} of {total} geometries decoded ({Share(decoded, total)}), {tally.Triangles[lod]} triangles");
        }

        foreach (var (reason, count) in tally.Reasons.OrderByDescending(pair => pair.Value).Take(ReasonsShown))
        {
            _output.WriteLine($"  {count,CountWidth}  {reason}");
        }
    }

    private static string Share(long part, long whole)
    {
        if (whole == 0)
        {
            return "-";
        }

        return (part * Percent / whole).ToString("F1", CultureInfo.InvariantCulture) + "%";
    }

    private static string FileName(string name)
    {
        var invalid = Path.GetInvalidFileNameChars();

        return new string(name.Select(character => Safe(character, invalid)).ToArray());
    }

    private static char Safe(char character, char[] invalid)
    {
        if (invalid.Contains(character) || character == '#')
        {
            return '_';
        }

        return character;
    }

    /// <summary>A problem message without its offsets and counts, so equal causes group.</summary>
    public static string Reason(string message)
    {
        var words = message.Split(' ').Select(Generalise);
        var reason = string.Join(' ', words);

        if (reason.Length > ReasonLength)
        {
            return reason[..ReasonLength];
        }

        return reason;
    }

    private static string Generalise(string word)
    {
        if (word.Any(char.IsDigit))
        {
            return "N";
        }

        return word;
    }
}
