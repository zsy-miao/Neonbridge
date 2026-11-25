// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using System.Text.Json;
using FiveStar.Formats.Common;
using FiveStar.Formats.Pso;

namespace FiveStar.Cli.Archive;

/// <summary>Hashes every word of a list with joaat and reports which harvested hashes it names.</summary>
internal sealed class HashCracker
{
    private const int Success = 0;
    private const int WordsShown = 25;
    private const char FirstSeparatorControl = '\u001c';
    private const char LastSeparatorControl = '\u001f';

    // A hash in several categories counts in the last one listed here.
    private static readonly string[] CategoryOrder = ["value", "enumval", "enum", "member", "struct"];
    private static readonly string[] ReportOrder = ["struct", "member", "enum", "enumval", "value"];

    private readonly TextWriter _output;

    public HashCracker(TextWriter output)
    {
        _output = output;
    }

    public int Run(string harvestPath, string wordsPath, string? outputPath)
    {
        var categories = ReadCategories(harvestPath);
        var hits = new OrderedMap<uint, string>();

        foreach (var line in File.ReadLines(wordsPath, Encoding.UTF8))
        {
            var word = Strip(line);

            if (word.Length == 0)
            {
                continue;
            }

            var hash = Joaat.Hash(word);

            if (categories.ContainsKey(hash) && !hits.ContainsKey(hash))
            {
                hits.Set(hash, word);
            }
        }

        PrintSummary(categories, hits);

        if (outputPath is not null)
        {
            WriteHits(outputPath, categories, hits);
        }

        return Success;
    }

    private static OrderedMap<uint, string> ReadCategories(string harvestPath)
    {
        using var json = JsonDocument.Parse(File.ReadAllText(harvestPath));
        var categories = new OrderedMap<uint, string>();

        foreach (var category in CategoryOrder)
        {
            if (!json.RootElement.TryGetProperty(category, out var hashes))
            {
                continue;
            }

            foreach (var entry in hashes.EnumerateObject())
            {
                categories.Set(uint.Parse(entry.Name, NumberStyles.HexNumber, CultureInfo.InvariantCulture), category);
            }
        }

        return categories;
    }

    private void PrintSummary(OrderedMap<uint, string> categories, OrderedMap<uint, string> hits)
    {
        var byCategory = new Tally<string>();

        foreach (var hash in hits.Keys)
        {
            byCategory.Add(categories[hash]);
        }

        _output.WriteLine($"cracked {hits.Count} of {categories.Count} {ReportText.CountMap(byCategory)}");

        foreach (var category in ReportOrder)
        {
            var words = new List<string>();

            for (var index = 0; index < hits.Count; index++)
            {
                if (categories[hits.Keys[index]] == category)
                {
                    words.Add(hits.Values[index]);
                }
            }

            words.Sort(StringComparer.Ordinal);
            var quoted = new List<string>();

            foreach (var word in words.Take(WordsShown))
            {
                quoted.Add(ReportText.Quote(word));
            }

            _output.WriteLine($"  {category} [{string.Join(", ", quoted)}]");
        }
    }

    private static void WriteHits(string outputPath, OrderedMap<uint, string> categories, OrderedMap<uint, string> hits)
    {
        var hashes = new List<uint>(hits.Keys);
        hashes.Sort();
        var text = new StringBuilder();

        foreach (var hash in hashes)
        {
            text.Append(hash.ToString("x8", CultureInfo.InvariantCulture)).Append('\t');
            text.Append(categories[hash]).Append('\t').Append(hits[hash]).Append('\n');
        }

        File.WriteAllText(outputPath, text.ToString());
    }

    // Trims white space as the word lists expect: Unicode white space plus the separator controls U+001C..U+001F.
    private static string Strip(string line)
    {
        var start = 0;
        var end = line.Length;

        while (start < end && IsSpace(line[start]))
        {
            start++;
        }

        while (end > start && IsSpace(line[end - 1]))
        {
            end--;
        }

        return line[start..end];
    }

    private static bool IsSpace(char character)
    {
        return char.IsWhiteSpace(character) || (character >= FirstSeparatorControl && character <= LastSeparatorControl);
    }
}
