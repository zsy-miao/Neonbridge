// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Analysis;

/// <summary>
/// What a translated program says about wave sizes: the console wave and the floor of an emulated program (its first
/// line, see <c>WaveOps.FloorLine</c>) and the lane count a compute program pins with <c>[WaveSize(n)]</c>.
/// </summary>
public sealed partial record WaveFacts(int? ProgramWave, int? Floor, int? WavePin)
{
    public static WaveFacts Read(string hlsl)
    {
        int? wave = null;
        int? floor = null;
        int? pin = null;
        var line = FloorPattern().Match(hlsl);

        if (line.Success)
        {
            wave = Number(line.Groups["wave"].Value);
            floor = Number(line.Groups["floor"].Value);
        }

        var pinned = PinPattern().Match(hlsl);

        if (pinned.Success)
        {
            pin = Number(pinned.Groups["lanes"].Value);
        }

        return new WaveFacts(wave, floor, pin);
    }

    private static int Number(string text)
    {
        return int.Parse(text, CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"^// program wave (?<wave>\d+); console results on hardware waves >= (?<floor>\d+) lanes", RegexOptions.Multiline)]
    private static partial Regex FloorPattern();

    [GeneratedRegex(@"^\[WaveSize\((?<lanes>\d+)\)\]", RegexOptions.Multiline)]
    private static partial Regex PinPattern();
}
