// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Graphics.Programs;

/// <summary>The DXIL files of a mesh program's compute + vertex-pulling fallback, relative to the manifest's folder.</summary>
public sealed record MeshFallbackFiles(string Compute, string Vertex)
{
    /// <summary>The compute shader pinned to <see cref="PinnedLanes"/>-lane waves (the console wave); runs with <see cref="Vertex"/>.</summary>
    public string? PinnedCompute { get; init; }

    /// <summary>The wave size <see cref="PinnedCompute"/> pins.</summary>
    public int? PinnedLanes { get; init; }

    /// <summary>The fallback of the program's superstep form: console results on any wave.</summary>
    public MeshFallbackFiles? Supersteps { get; init; }
}
