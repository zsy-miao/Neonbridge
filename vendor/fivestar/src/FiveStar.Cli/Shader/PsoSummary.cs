// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Cli.Shader;

/// <summary>Prints the results of a sweep: counts per kind and result, wave-pinned compute programs and the largest error groups.</summary>
internal static partial class PsoSummary
{
    private const int GroupsShown = 20;
    private const int CountWidth = 7;
    private const int KindWidth = 8;
    private const int ColumnWidth = 12;
    private const int GroupTextLength = 160;

    private static readonly string[] Results = [PsoRecord.Accepted, PsoRecord.Rejected, PsoRecord.SetupFailed, PsoRecord.TimedOut, PsoRecord.OverMemory, PsoRecord.Crashed];

    public static void Print(IReadOnlyCollection<PsoRecord> records)
    {
        Console.WriteLine($"{"kind",-KindWidth}" + string.Concat(Results.Select(result => $"{result,ColumnWidth}")));

        foreach (var kind in records.Select(record => record.Kind).Distinct().Order(StringComparer.Ordinal))
        {
            var ofKind = records.Where(record => record.Kind == kind).ToList();
            var counts = Results.Select(result => Count(ofKind.Count(record => record.Result == result)).PadLeft(ColumnWidth));
            Console.WriteLine($"{kind,-KindWidth}" + string.Concat(counts));
        }

        foreach (var wave in records.Where(record => record.Wave is not null).GroupBy(record => record.Wave).OrderBy(group => group.Key))
        {
            var rejected = wave.Count(record => record.Result != PsoRecord.Accepted);
            Console.WriteLine($"cs pinned [WaveSize({wave.Key})]: {wave.Count()}, not accepted {rejected}");
        }

        var groups = records.Where(record => record.Result != PsoRecord.Accepted)
            .GroupBy(record => (record.Result, Text: GroupText(record)))
            .OrderByDescending(group => group.Count())
            .Take(GroupsShown);

        foreach (var group in groups)
        {
            var kinds = string.Join(",", group.Select(record => record.Kind).Distinct().Order(StringComparer.Ordinal));
            Console.WriteLine($"{Count(group.Count()).PadLeft(CountWidth)} {group.Key.Result} [{kinds}] {group.Key.Text}");
            Console.WriteLine($"{string.Empty.PadLeft(CountWidth)}   e.g. {group.First().Name}");
        }
    }

    /// <summary>The HRESULT and message with numbers replaced by #, so programs failing the same way group together.</summary>
    private static string GroupText(PsoRecord record)
    {
        var text = NumberPattern().Replace($"{record.Hresult} {record.Message}".Trim(), "#");

        return text[..Math.Min(text.Length, GroupTextLength)];
    }

    private static string Count(int value)
    {
        return value.ToString(CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"(?<!0x)\b\d+\b")]
    private static partial Regex NumberPattern();
}
