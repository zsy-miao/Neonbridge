// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// A drawable found in a resource: its name, root offset, shaders, geometries by level of detail, skeleton, and for a
/// vehicle fragment its wheel cylinders.
/// </summary>
public sealed record Drawable(
    string Name,
    int Root,
    IReadOnlyList<ShaderTextures> Shaders,
    IReadOnlyList<MeshGeometry> Geometries,
    IReadOnlyList<GeometryProblem> Problems,
    Skeleton Skeleton,
    IReadOnlyList<WheelBound> Wheels)
{
    /// <summary>The geometries of one level of detail.</summary>
    public IReadOnlyList<MeshGeometry> Lod(int lod)
    {
        return Geometries.Where(geometry => geometry.Lod == lod).ToList();
    }
}
