// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>
/// Builds <c>status.json</c>: shaders from a translate run, textures and archives from a dump, every other area and
/// the still-needed list from the manual file. An area with no new source keeps its value from the previous status.
/// </summary>
internal sealed class StatusCollector
{
    private readonly TextWriter _log;

    public StatusCollector(TextWriter log)
    {
        _log = log;
    }

    public ProgressStatus Collect(string? shaders, string? dump, string? manual, ProgressStatus? previous)
    {
        var fresh = new List<ProgressArea>();

        if (shaders is not null)
        {
            fresh.Add(ShaderTally.Measure(shaders));
        }

        if (dump is not null)
        {
            fresh.Add(TextureTally.Measure(dump, _log));
            fresh.Add(ArchiveTally.Measure(dump, _log));
        }

        ProgressStatus? manualStatus = null;

        if (manual is not null)
        {
            manualStatus = StatusFile.Read(manual);
            fresh.AddRange(manualStatus.Areas);
        }

        var stillNeeded = manualStatus?.StillNeeded ?? previous?.StillNeeded;

        return new ProgressStatus(Merge(fresh, previous), stillNeeded);
    }

    /// <summary>The previous areas in their order with new ones in place of those with the same key, then the other new ones.</summary>
    private static List<ProgressArea> Merge(List<ProgressArea> fresh, ProgressStatus? previous)
    {
        if (previous is null)
        {
            return fresh;
        }

        var byKey = fresh.ToDictionary(area => area.Key, StringComparer.Ordinal);
        var areas = new List<ProgressArea>();

        foreach (var area in previous.Areas)
        {
            areas.Add(byKey.GetValueOrDefault(area.Key, area));
        }

        var kept = new HashSet<string>(previous.Areas.Select(area => area.Key), StringComparer.Ordinal);
        areas.AddRange(fresh.Where(area => !kept.Contains(area.Key)));

        return areas;
    }
}
