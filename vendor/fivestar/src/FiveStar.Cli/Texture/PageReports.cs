// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf;

namespace FiveStar.Cli.Texture;

/// <summary>Page-level texture reports: the page-table sample dump.</summary>
internal sealed class PageReports
{
    private const int ExamplesPerFlag = 3;
    private const int EntropyProbe = 4096;
    private const int HeadBytes = 12;
    private const int FlagDigits = 2;
    private const int TwoDecimals = 2;

    private readonly TextWriter _output;

    public PageReports(TextWriter output)
    {
        _output = output;
    }

    /// <summary>The first three entries of each flag value with their storage kind, then the count per (flag, kind).</summary>
    public void Pages(string path)
    {
        using var source = new MappedFileSource(path);
        var survey = new PageTableSurvey(source, new ZlibInflater());
        var entries = survey.Entries();
        var shown = new Dictionary<byte, int>();
        var kinds = new Tally<(byte Flag, string Kind)>();

        for (var index = 0; index < entries.Count; index++)
        {
            var entry = entries[index];
            var stored = StoredBytes(source, entries, index);
            var kind = survey.Probe(stored);
            kinds.Add((entry.Flag, kind.Split(' ')[0]));
            var seen = shown.GetValueOrDefault(entry.Flag);

            if (seen >= ExamplesPerFlag)
            {
                continue;
            }

            shown[entry.Flag] = seen + 1;
            var next = index + 1 < entries.Count ? entries[index + 1].VirtualPage : 0;
            var entropy = DecimalText.Fixed(ByteEntropy.BitsPerByte(stored[..Math.Min(EntropyProbe, stored.Length)]), TwoDecimals);
            var head = Convert.ToHexStringLower(stored[..Math.Min(HeadBytes, stored.Length)]);
            _output.WriteLine($"#{index} flag=0x{ReportText.Hex(entry.Flag, FlagDigits)} off={DecimalText.Hex(entry.FileOffset)} len={DecimalText.Hex(stored.Length)} hi20={DecimalText.Hex(entry.Tag)} vstep={next - entry.VirtualPage} ent={entropy} {kind} head={SpacedHex(head)}");
        }

        var parts = new List<string>();

        foreach (var pair in kinds.MostCommon())
        {
            parts.Add($"({pair.Key.Flag}, {ReportText.Quote(pair.Key.Kind)}): {pair.Value}");
        }

        _output.WriteLine($"Counter({{{string.Join(", ", parts)}}})");
    }

    // An entry's stored bytes run to the next entry's offset; a later entry that does not move forward ends the
    // range at the end of the file.
    private static ReadOnlySpan<byte> StoredBytes(IByteSource source, IReadOnlyList<PageTableEntry> entries, int index)
    {
        var offset = entries[index].FileOffset;
        var end = source.Length;

        if (index + 1 < entries.Count && entries[index + 1].FileOffset >= offset)
        {
            end = entries[index + 1].FileOffset;
        }

        return source.Read(offset, (int)Math.Clamp(end - offset, 0, int.MaxValue));
    }

    private static string SpacedHex(string hex)
    {
        var pairs = new List<string>();

        for (var index = 0; index + 1 < hex.Length; index += FlagDigits)
        {
            pairs.Add(hex.Substring(index, FlagDigits));
        }

        return string.Join(' ', pairs);
    }
}
