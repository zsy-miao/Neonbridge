// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Analysis;

/// <summary>
/// Classifies translated programs by wave-size dependence (wave64 programs on wave32-only GPUs): programs without wave
/// or quad intrinsics are agnostic; lane-index and EXEC bookkeeping can be fixed mechanically; quad operations and
/// first-lane reads stay correct; lane-indexed data movement (readlane, permlane, bpermute, DPP rows) needs wave
/// emulation or a fixed wave size. Programs translated with wave emulation also carry their floor (the narrowest
/// hardware wave with console results) in their first line.
/// </summary>
public static partial class WaveUsageAnalyzer
{
    private const string EntryMarker = "main(";

    /// <summary>Every ok program of a corpus run with its classification.</summary>
    public static IReadOnlyList<WaveUsageEntry> Analyze(string outputFolder, string programFolder)
    {
        var entries = new List<WaveUsageEntry>();

        foreach (var result in ResultsFile.Read(Path.Combine(outputFolder, CorpusTranslator.ResultsName)))
        {
            if (result.Status is not ProgramStatus.Ok and not ProgramStatus.OkMeshFallback)
            {
                continue;
            }

            var programPath = Path.Combine(programFolder, result.Name + ".fssm");
            var hlslPath = Path.Combine(outputFolder, result.Name + ".hlsl");

            if (!File.Exists(programPath) || !File.Exists(hlslPath))
            {
                continue;
            }

            var wave = WaveSize(programPath);

            if (wave is not null)
            {
                entries.Add(Classify(result, wave.Value, File.ReadAllText(hlslPath)));
            }
        }

        return entries;
    }

    /// <summary>Classifies the code after the entry point (helpers above it are not counted).</summary>
    public static WaveUsageEntry Classify(ProgramResult result, int waveSize, string hlsl)
    {
        var data = DataOperations(hlsl);

        return new WaveUsageEntry(result.Name, result.Stage, waveSize, Dependence(hlsl), data.ToHashSet(), WaveFacts.Read(hlsl).Floor);
    }

    /// <summary>How the code after the entry point depends on the wave size.</summary>
    public static WaveDependence Dependence(string hlsl)
    {
        var data = DataOperations(hlsl);

        if (data.Any(IsLaneIndexed))
        {
            return WaveDependence.LaneIndexed;
        }

        if (data.Count > 0)
        {
            return WaveDependence.QuadAndFirstLane;
        }

        if (BookkeepingPattern().IsMatch(Body(hlsl)))
        {
            return WaveDependence.Bookkeeping;
        }

        return WaveDependence.Agnostic;
    }

    private static List<string> DataOperations(string hlsl)
    {
        return DataPattern().Matches(Body(hlsl)).Select(match => match.Groups[1].Value).ToList();
    }

    private static string Body(string hlsl)
    {
        var entry = hlsl.IndexOf(EntryMarker, StringComparison.Ordinal);

        if (entry < 0)
        {
            return string.Empty;
        }

        return hlsl[entry..];
    }

    private static int? WaveSize(string programPath)
    {
        try
        {
            return FssmHeader.Decode(File.ReadAllBytes(programPath)).WaveSize;
        }
        catch (InvalidDataException)
        {
            return null;
        }
    }

    /// <summary>The report label of a classification.</summary>
    public static string Label(WaveDependence dependence)
    {
        switch (dependence)
        {
            case WaveDependence.LaneIndexed:
                return "lane-indexed / wave-wide data";
            case WaveDependence.QuadAndFirstLane:
                return "quad + first-lane only";
            case WaveDependence.Bookkeeping:
                return "lane/exec bookkeeping only";
            default:
                return "wave-size agnostic";
        }
    }

    private static bool IsLaneIndexed(string operation)
    {
        string[] wavePrefixes = ["WaveMultiPrefix", "WavePrefix", "WaveActive"];

        return operation is "WaveReadLaneAt" or "vw_read" or "vw_read_fill" or "WaveMatch" || wavePrefixes.Any(prefix => operation.StartsWith(prefix, StringComparison.Ordinal));
    }

    [GeneratedRegex(@"\b(WaveReadLaneAt|vw_read|vw_read_fill|WaveReadLaneFirst|WaveActive(?:Sum|Product|Min|Max|BitAnd|BitOr|BitXor|AllEqual)|WavePrefix\w+|WaveMultiPrefix\w+|WaveMatch|QuadRead\w+|quad_perm|readfirstlane)\s*\(")]
    private static partial Regex DataPattern();

    [GeneratedRegex(@"\b(WaveActiveBallot|WaveActiveCountBits|WaveActiveAnyTrue|WaveActiveAllTrue|WaveGetLaneIndex|lane_bit|WaveIsFirstLane|vw_ballot|vw_count|vw_any|vw_lane)\s*\(")]
    private static partial Regex BookkeepingPattern();
}
