// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>A mesh shader's limits: vertices and primitives per group, topology (1 line, 2 triangle), shared bytes.</summary>
public sealed record MeshState(int MaxVertices, int MaxPrimitives, int Topology, int SharedBytes);
