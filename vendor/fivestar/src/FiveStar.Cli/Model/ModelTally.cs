// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Models;

namespace FiveStar.Cli.Model;

/// <summary>Counts for <c>model scan</c>: resources, drawables, geometries per level of detail and problem reasons.</summary>
internal sealed class ModelTally
{
    private readonly HashSet<long> _resources = [];
    private readonly HashSet<long> _withoutPhysical = [];

    public int Resources => _resources.Count;

    public int WithoutPhysical => _withoutPhysical.Count;

    public int Drawables { get; private set; }

    public int CompleteDrawables { get; private set; }

    public long[] Decoded { get; } = new long[DrawableReader.LevelsOfDetail];

    public long[] Failed { get; } = new long[DrawableReader.LevelsOfDetail];

    public long[] Triangles { get; } = new long[DrawableReader.LevelsOfDetail];

    public Dictionary<string, int> Reasons { get; } = new(StringComparer.Ordinal);

    public void Add(CarvedDrawable carved)
    {
        _resources.Add(carved.Offset);

        if (!carved.HasPhysical)
        {
            _withoutPhysical.Add(carved.Offset);
        }

        Drawables++;

        if (carved.Drawable.Problems.Count == 0)
        {
            CompleteDrawables++;
        }

        foreach (var geometry in carved.Drawable.Geometries)
        {
            Decoded[geometry.Lod]++;
            Triangles[geometry.Lod] += geometry.TriangleCount;
        }

        foreach (var problem in carved.Drawable.Problems)
        {
            Failed[problem.Lod]++;
            var reason = ModelReports.Reason(problem.Reason);
            Reasons[reason] = Reasons.GetValueOrDefault(reason) + 1;
        }
    }
}
