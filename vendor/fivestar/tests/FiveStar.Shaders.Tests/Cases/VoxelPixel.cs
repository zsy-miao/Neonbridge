// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// What the voxelization model expects at one pixel: don't care (centre within the tolerance of an edge), or the axis
/// id, interpolated texcoord and normal of the covering triangle.
/// </summary>
public sealed record VoxelPixel(bool NearEdge, uint Axis, double S, double T, double[] Normal)
{
    public static VoxelPixel Edge { get; } = new(true, 0, 0, 0, []);
}
