// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Bvh;

/// <summary>Where the leaf nodes of a built tree are: after the boxes, one node per leaf in leaf order.</summary>
/// <param name="Start">Byte offset of the first leaf node from the first box.</param>
/// <param name="NodeSize">Bytes per leaf node.</param>
/// <param name="Type">The node type a pointer to a leaf carries.</param>
internal readonly record struct Bvh4Leaves(int Start, int NodeSize, uint Type);
