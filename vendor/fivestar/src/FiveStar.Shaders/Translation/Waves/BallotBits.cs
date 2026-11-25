// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// Reads a lane's own bit of a ballot from the lane-mask bool the ballot took, before the superstep form cuts the
/// blocks: <c>lane_bit(lo, hi)</c> of two SGPR halves that hold <c>vw_ballot(m).x</c> and <c>.y</c> is <c>m</c> while
/// neither half nor <c>m</c> has been written since, because every thread of the console wave deposits its bit. The
/// ballot then often has no other reader and <see cref="DeadWaveOperations"/> drops it: in
/// MSNC_VS_LinearDepthSkin_f1f2249c most of the ballots left were read only this way. Within one block, in order.
/// </summary>
internal static partial class BallotBits
{
    /// <summary>Replaces such reads in <paramref name="blocks"/>; how many.</summary>
    public static int Substitute(List<CodeBlock> blocks)
    {
        int replaced = 0;

        foreach (var lines in blocks.Select(block => block.Lines))
        {
            replaced += SubstituteBlock(lines);
        }

        return replaced;
    }

    private static int SubstituteBlock(List<string> lines)
    {
        // SGPR half -> (mask bool, half) it holds.
        var halves = new Dictionary<string, (string Mask, char Half)>(StringComparer.Ordinal);
        int replaced = 0;

        for (int i = 0; i < lines.Count; i++)
        {
            string line = Replace(lines[i], halves, ref replaced);
            lines[i] = line;
            Invalidate(line, halves);

            var ballot = BallotPattern().Match(line);
            var copy = i + 1 < lines.Count ? CopyPattern().Match(lines[i + 1]) : Match.Empty;

            if (ballot.Success && copy.Success && copy.Groups["value"].Value == ballot.Groups["temp"].Value)
            {
                halves.Remove(copy.Groups["name"].Value);
                Invalidate(lines[i + 1], halves);
                halves[copy.Groups["name"].Value] = (ballot.Groups["mask"].Value, ballot.Groups["half"].Value[0]);
                i++;
            }
        }

        return replaced;
    }

    private static string Replace(string line, Dictionary<string, (string Mask, char Half)> halves, ref int replaced)
    {
        int count = 0;
        string result = LaneBitPattern().Replace(line, call =>
        {
            if (halves.TryGetValue(call.Groups["lo"].Value, out var lo) && halves.TryGetValue(call.Groups["hi"].Value, out var hi)
                && lo.Mask == hi.Mask && lo.Half == 'x' && hi.Half == 'y')
            {
                count++;

                return $"({lo.Mask})";
            }

            return call.Value;
        });
        replaced += count;

        return result;
    }

    /// <summary>Forgets every half the line writes, and every half whose mask bool it writes.</summary>
    private static void Invalidate(string line, Dictionary<string, (string Mask, char Half)> halves)
    {
        foreach (Match write in AssignmentPattern().Matches(line))
        {
            string name = write.Groups["name"].Value;
            halves.Remove(name);

            foreach (var key in halves.Where(pair => pair.Value.Mask == name).Select(pair => pair.Key).ToList())
            {
                halves.Remove(key);
            }
        }
    }

    [GeneratedRegex(@"^\s*uint (?<temp>_t\d+) = vw_ballot\((?<mask>[A-Za-z_]\w*)\)\.(?<half>[xy]);\s*$")]
    private static partial Regex BallotPattern();

    [GeneratedRegex(@"^\s*(?<name>[A-Za-z_]\w*) = (?<value>_t\d+);\s*$")]
    private static partial Regex CopyPattern();

    [GeneratedRegex(@"\blane_bit\((?<lo>[A-Za-z_]\w*), (?<hi>[A-Za-z_]\w*)\)")]
    private static partial Regex LaneBitPattern();

    [GeneratedRegex(@"(?<![.\w])(?<name>[A-Za-z_]\w*)\s*(?:[-+*/&|^]|<<|>>)?=(?!=)")]
    private static partial Regex AssignmentPattern();
}
