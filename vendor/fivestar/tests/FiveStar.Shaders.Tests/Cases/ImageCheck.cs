// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>Compares rendered targets with an expected image and the pipeline statistics with expected counts.</summary>
public static class ImageCheck
{
    /// <summary>
    /// Every pixel of every target against <paramref name="expected"/>(x, y, target): four expected words, or null when
    /// the pixel must keep the clear value.
    /// </summary>
    public static List<string> Compare(WarpResult result, int width, int height, Func<int, int, int, Expected[]?> expected)
    {
        var mismatches = new List<string>();

        for (var target = 0; target < result.Targets.Count; target++)
        {
            for (var y = 0; y < height; y++)
            {
                for (var x = 0; x < width; x++)
                {
                    var actual = result.Pixel(target, x, y);
                    var wanted = expected(x, y, target) ?? [.. Enumerable.Repeat(Expected.Exact(Framebuffer.ClearUint), 4)];

                    if (!wanted.Zip(actual).All(pair => pair.First.Matches(pair.Second)))
                    {
                        mismatches.Add(string.Create(
                            CultureInfo.InvariantCulture,
                            $"pixel ({x},{y}) rt{target} expected {string.Join(" ", wanted.Select(value => value.ToString()))} actual {string.Join(" ", actual.Select(Expected.Hex))}"));
                    }
                }
            }
        }

        return mismatches;
    }

    /// <summary>The statistics that differ from <paramref name="expected"/> (name, count).</summary>
    public static IEnumerable<string> Statistics(PipelineStatistics? actual, params (string Name, ulong Count)[] expected)
    {
        if (actual is null)
        {
            yield return "no pipeline statistics";

            yield break;
        }

        foreach (var (name, count) in expected)
        {
            var value = Read(actual, name);

            if (value != count)
            {
                yield return string.Create(CultureInfo.InvariantCulture, $"pipeline statistic {name}: expected {count}, actual {value}");
            }
        }
    }

    private static ulong Read(PipelineStatistics statistics, string name)
    {
        switch (name)
        {
            case "GSInvocations":
                return statistics.GeometryInvocations;
            case "GSPrimitives":
                return statistics.GeometryPrimitives;
            case "CInvocations":
                return statistics.ClipperInvocations;
            default:
                throw new ArgumentOutOfRangeException(nameof(name));
        }
    }
}
