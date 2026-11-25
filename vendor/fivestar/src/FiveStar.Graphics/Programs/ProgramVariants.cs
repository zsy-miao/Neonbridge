// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Graphics.Programs;

/// <summary>
/// The compiled forms of one translated program and the wave facts that decide which one a GPU runs. File names are
/// relative to the manifest's folder.
/// </summary>
/// <param name="Name">The program name.</param>
/// <param name="Stage">ps, vs, gs, ms or cs.</param>
/// <param name="Dxil">The native DXIL; null for a mesh program that compiles only through the fallback.</param>
/// <param name="ProgramWave">The console wave (32 or 64) of a program with wave emulation.</param>
/// <param name="Floor">The narrowest hardware wave that gives console results (pixel, vertex, geometry, mesh).</param>
/// <param name="WavePin">The lane count a compute program pins with <c>[WaveSize]</c>.</param>
/// <param name="GroupWave">The variant that runs on any wave size: of a wave64-pinned compute program, or the superstep form of a mesh program.</param>
/// <param name="MeshFallback">The compute + vertex-pulling replacement of a mesh program.</param>
public sealed record ProgramVariants(
    string Name,
    string Stage,
    string? Dxil,
    int? ProgramWave,
    int? Floor,
    int? WavePin,
    string? GroupWave,
    MeshFallbackFiles? MeshFallback);
