// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Texture;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf;

namespace FiveStar.Cli.Progress;

/// <summary>
/// Textures from the coverage of every <c>.ttpf</c> in a dump: readable ones are done; BCPack-blocked ones are grouped
/// by block-compression family (small families merged), unknown cube layouts and other layouts get a group each.
/// </summary>
internal static class TextureTally
{
    public const string Key = "textures";

    private const string Title = "Textures (.ttpf) that extract cleanly";
    private const double Height = 120;
    private const string OkStatus = "ok";
    private const string BcPackStatus = "bcpack_";
    private const string ApproximateStatus = "approx_";
    private const string ApproximateName = "BCPack approximate";
    private const string ApproximateNote = "mip 0 decodes through BCPack tables fitted to neighbouring mips; not byte-exact";
    private const string CubeStatus = "skip_cube";
    private const string BcPackPrefix = "BCPack ";
    private const string BcPackNote = "mip 0 touches a BCPack page; no decoder yet";

    // A BCPack family below this share of all BCPack textures joins the merged group.
    private const double SmallFamilyShare = 0.02;

    // Readable text for the status names of TextureStatus.Name; a prefix entry is followed by its detail value.
    private static readonly (string Status, string Text)[] StatusTexts =
    [
        ("skip_cube_mips", "cubes with mips"),
        ("skip_cube_span", "cubes whose span does not split into faces"),
        ("skip_size", "mip 0 larger than its span"),
        ("skip_unsupported_codec", "unknown page codec"),
        ("skip_mode", "layout mode "),
        ("skip_fmt_", "DXGI format "),
    ];

    public static ProgressArea Measure(string dump, TextWriter log)
    {
        var textures = new Tally<string>();
        var blockers = new Tally<(int Codec, string Format)>();

        foreach (var path in TextureReports.PageFiles(dump))
        {
            using var source = new MappedFileSource(path);
            var file = new TexturePageFile(source, new ZlibInflater());
            var report = new TextureCoverage(file, new TextureExtractor(file)).Measure();
            textures.AddAll(report.Textures);
            blockers.AddAll(report.Blockers);
            log.WriteLine($"textures: {Path.GetFileName(path)} {report.Textures.Total}");
        }

        var approximate = textures.MostCommon().Where(entry => entry.Key.StartsWith(ApproximateStatus, StringComparison.Ordinal)).Sum(entry => entry.Value);
        var groups = new List<ProgressGroup>
        {
            new("zlib / stored pages", textures[OkStatus], 0, 0, "mip 0 deswizzled from zlib or stored pages"),
            new(ApproximateName, 0, approximate, 0, ApproximateNote),
        };
        groups.AddRange(BcPackGroups(blockers));
        groups.AddRange(LayoutGroups(textures));

        return new ProgressArea(Key, Title, null, Height, groups.Where(group => group.Total > 0).ToList());
    }

    private static List<ProgressGroup> BcPackGroups(Tally<(int Codec, string Format)> blockers)
    {
        var families = new Tally<string>();

        foreach (var entry in blockers.MostCommon())
        {
            families.Add(entry.Key.Format.Split('_')[0], entry.Value);
        }

        var groups = new List<ProgressGroup>();
        var small = new List<string>();
        long smallCount = 0;

        foreach (var entry in families.MostCommon())
        {
            if (entry.Value < families.Total * SmallFamilyShare)
            {
                small.Add(entry.Key);
                smallCount += entry.Value;

                continue;
            }

            groups.Add(new ProgressGroup(BcPackPrefix + entry.Key, 0, 0, entry.Value, BcPackNote));
        }

        if (small.Count > 0)
        {
            groups.Add(new ProgressGroup(BcPackPrefix + string.Join('/', small), 0, 0, smallCount, BcPackNote));
        }

        return groups;
    }

    private static string Describe(string status)
    {
        foreach (var (prefix, text) in StatusTexts)
        {
            if (status.StartsWith(prefix, StringComparison.Ordinal))
            {
                return text + status[prefix.Length..];
            }
        }

        return status;
    }

    private static List<ProgressGroup> LayoutGroups(Tally<string> textures)
    {
        var cube = new List<string>();
        var other = new List<string>();
        long cubeCount = 0;
        long otherCount = 0;

        foreach (var entry in textures.MostCommon())
        {
            if (entry.Key == OkStatus || entry.Key.StartsWith(BcPackStatus, StringComparison.Ordinal) || entry.Key.StartsWith(ApproximateStatus, StringComparison.Ordinal))
            {
                continue;
            }

            if (entry.Key.StartsWith(CubeStatus, StringComparison.Ordinal))
            {
                cube.Add($"{Describe(entry.Key)} ({entry.Value})");
                cubeCount += entry.Value;
            }
            else
            {
                other.Add($"{Describe(entry.Key)} ({entry.Value})");
                otherCount += entry.Value;
            }
        }

        return
        [
            new ProgressGroup("cube layouts", 0, 0, cubeCount, string.Join(", ", cube)),
            new ProgressGroup("other", 0, 0, otherCount, string.Join(", ", other)),
        ];
    }
}
