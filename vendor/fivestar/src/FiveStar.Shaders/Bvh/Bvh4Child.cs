// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Bvh;

/// <summary>A child of a box while building: a box (by index) or a leaf (by position in the leaf order).</summary>
/// <param name="IsLeaf">Whether the child is a leaf (a triangle or an instance).</param>
/// <param name="Index">Box index, or the leaf's position in <see cref="Bvh4Tree.Order"/>.</param>
/// <param name="Bounds">The child's bounds.</param>
internal readonly record struct Bvh4Child(bool IsLeaf, int Index, BvhBounds Bounds);
