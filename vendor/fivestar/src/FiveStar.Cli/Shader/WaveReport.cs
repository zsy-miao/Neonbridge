// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Analysis;

namespace FiveStar.Cli.Shader;

/// <summary><c>fivestar shader waves</c>: how wave-size dependent the translated programs of a corpus run are.</summary>
internal static class WaveReport
{
    private const int Success = 0;
    private const int Wave64 = 64;
    private const int StageWidth = 3;
    private const int LabelWidth = 28;
    private const int CountWidth = 6;
    private const int TwoPaths = 2;

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string>());

        if (arguments.Positional.Count != TwoPaths)
        {
            throw new ArgumentException("waves <out> <fssm-folder>");
        }

        var entries = WaveUsageAnalyzer.Analyze(arguments.Positional[0], arguments.Positional[1]);
        var groups = entries.GroupBy(entry => (Stage: entry.Stage ?? "?", entry.WaveSize, entry.Dependence))
            .OrderBy(group => group.Key.Stage, StringComparer.Ordinal).ThenBy(group => group.Key.WaveSize).ThenBy(group => group.Key.Dependence);

        foreach (var group in groups)
        {
            var label = WaveUsageAnalyzer.Label(group.Key.Dependence);
            var count = group.Count().ToString(System.Globalization.CultureInfo.InvariantCulture).PadLeft(CountWidth);
            Console.WriteLine($"{group.Key.Stage.PadRight(StageWidth)} wave{group.Key.WaveSize}  {label.PadRight(LabelWidth)} {count}   e.g. {group.First().Name}");
        }

        var wave64 = entries.Where(entry => entry.WaveSize == Wave64).ToList();
        var laneIndexed = wave64.Count(entry => entry.Dependence == WaveDependence.LaneIndexed);
        Console.WriteLine($"wave64 total {wave64.Count} | lane-indexed (need wave64 emulation or [WaveSize(64)]) {laneIndexed}");

        var operations = wave64.SelectMany(entry => entry.DataOperations).GroupBy(operation => operation)
            .OrderByDescending(group => group.Count()).Select(group => $"{group.Key} {group.Count()}");
        Console.WriteLine("cross-lane operations in wave64 programs (programs): " + string.Join(", ", operations));
        ReportFloors(entries);

        return Success;
    }

    /// <summary>For programs translated with wave emulation: how many give console results on each hardware wave size.</summary>
    private static void ReportFloors(IReadOnlyList<WaveUsageEntry> entries)
    {
        var emulated = entries.Where(entry => entry.Floor is not null && entry.Dependence == WaveDependence.LaneIndexed).ToList();

        if (emulated.Count == 0)
        {
            return;
        }

        Console.WriteLine("lane-indexed programs with wave emulation, by the narrowest hardware wave with console results:");

        foreach (var group in emulated.GroupBy(entry => (Stage: entry.Stage ?? "?", entry.WaveSize)).OrderBy(group => group.Key.Stage, StringComparer.Ordinal).ThenBy(group => group.Key.WaveSize))
        {
            var floors = group.GroupBy(entry => entry.Floor!.Value).OrderBy(floor => floor.Key).Select(floor => $">={floor.Key}: {floor.Count()}");
            Console.WriteLine($"{group.Key.Stage.PadRight(StageWidth)} wave{group.Key.WaveSize}  {string.Join(", ", floors)}");
        }
    }
}
