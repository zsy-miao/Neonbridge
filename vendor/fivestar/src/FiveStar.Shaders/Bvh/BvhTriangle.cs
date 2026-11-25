// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;

namespace FiveStar.Shaders.Bvh;

/// <summary>One BLAS triangle: its vertices (already transformed) and the indices a hit reports.</summary>
/// <param name="V0">First vertex.</param>
/// <param name="V1">Second vertex.</param>
/// <param name="V2">Third vertex.</param>
/// <param name="GeometryIndex">Index of the geometry desc (24 bits).</param>
/// <param name="PrimitiveIndex">Index of the triangle within its geometry.</param>
/// <param name="GeometryFlags">The geometry flags byte stored beside the geometry index.</param>
public readonly record struct BvhTriangle(Vector3 V0, Vector3 V1, Vector3 V2, uint GeometryIndex, uint PrimitiveIndex, byte GeometryFlags);
