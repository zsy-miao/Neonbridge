// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Bvh;

/// <summary>
/// The nodes of a built BVH4: box32 nodes, then triangle nodes. <see cref="Nodes"/> goes at the node offset given to
/// the builder from the BVH base; node pointers count from that base.
/// </summary>
/// <param name="Nodes">The node bytes.</param>
/// <param name="Root">The root node pointer (a box32 node), or <see cref="AmdBvhLayout.InvalidNode"/> for no triangles.</param>
/// <param name="BoxNodes">Number of box32 nodes.</param>
/// <param name="TriangleNodes">Number of triangle nodes.</param>
public sealed record Bvh4Image(byte[] Nodes, uint Root, int BoxNodes, int TriangleNodes);
