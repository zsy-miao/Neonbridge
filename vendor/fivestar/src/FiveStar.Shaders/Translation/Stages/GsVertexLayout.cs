// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// The GSVS vertex record size (dwords) of a legacy geometry program, from its LDS writes: vertex k of a lane's record
/// is written between the k-th and (k+1)-th GS_EMIT at dword offsets [k*V, (k+1)*V) from the record base.
/// </summary>
internal static partial class GsVertexLayout
{
    private const int DwordBits = 32;
    private const int BytesPerDword = 4;
    private const int Vec4 = 4;

    public static int VertexDwords(IReadOnlyList<Instruction> instructions)
    {
        var segments = new Dictionary<int, List<int>>();
        var segment = 0;

        foreach (var instruction in instructions)
        {
            var mnemonic = instruction.Mnemonic;

            if (mnemonic == "s_sendmsg" && instruction.Text.Contains("GS_OP_EMIT", StringComparison.Ordinal))
            {
                segment++;
                continue;
            }

            if (!mnemonic.StartsWith("ds_write", StringComparison.Ordinal))
            {
                continue;
            }

            if (!segments.TryGetValue(segment, out var dwords))
            {
                dwords = [];
                segments[segment] = dwords;
            }

            dwords.AddRange(WrittenDwords(mnemonic, instruction.Text));
        }

        return RecordSize(segments);
    }

    private static List<int> WrittenDwords(string mnemonic, string text)
    {
        if (mnemonic.StartsWith("ds_write2st64", StringComparison.Ordinal))
        {
            throw new UnsupportedProgramException("GS output via ds_write2st64");
        }

        var pair = WritePairPattern().Match(mnemonic);

        if (pair.Success)
        {
            var unit = int.Parse(pair.Groups[1].Value, CultureInfo.InvariantCulture) / DwordBits;
            var first = Field(Offset0Pattern(), text);
            var second = Field(Offset1Pattern(), text);

            return [.. Range(first * unit, unit), .. Range(second * unit, unit)];
        }

        var single = WritePattern().Match(mnemonic);

        if (!single.Success)
        {
            throw new UnsupportedProgramException($"GS output via {mnemonic}");
        }

        var offset = Field(OffsetPattern(), text);

        if (offset % BytesPerDword != 0)
        {
            throw new UnsupportedProgramException("unaligned GS output write");
        }

        return Range(offset / BytesPerDword, int.Parse(single.Groups[1].Value, CultureInfo.InvariantCulture) / DwordBits);
    }

    private static int RecordSize(Dictionary<int, List<int>> segments)
    {
        if (segments.Count == 0)
        {
            throw new UnsupportedProgramException("GS without outputs");
        }

        var later = segments.Where(pair => pair.Key >= 1).Select(pair => pair.Value.Min() / pair.Key).ToList();
        int size;

        if (later.Count > 0)
        {
            size = later.Min();
        }
        else
        {
            size = (segments[0].Max() + Vec4) / Vec4 * Vec4;
        }

        foreach (var (vertex, dwords) in segments)
        {
            if (!dwords.All(dword => vertex * size <= dword && dword < (vertex + 1) * size))
            {
                throw new UnsupportedProgramException("GS output writes do not match a vertex-major GSVS layout");
            }
        }

        return size;
    }

    private static List<int> Range(int start, int count)
    {
        return Enumerable.Range(start, count).ToList();
    }

    private static int Field(Regex pattern, string text)
    {
        var match = pattern.Match(text);

        if (!match.Success)
        {
            return 0;
        }

        return int.Parse(match.Groups[1].Value, CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"^ds_write2_b(32|64)$")]
    private static partial Regex WritePairPattern();

    [GeneratedRegex(@"^ds_write_b(32|64|96|128)$")]
    private static partial Regex WritePattern();

    [GeneratedRegex(@"offset0:(\d+)")]
    private static partial Regex Offset0Pattern();

    [GeneratedRegex(@"offset1:(\d+)")]
    private static partial Regex Offset1Pattern();

    [GeneratedRegex(@"offset:(\d+)")]
    private static partial Regex OffsetPattern();
}
