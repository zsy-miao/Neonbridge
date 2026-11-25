// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Reference;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>One intersect query of a BVH case: the node pointer and the ray.</summary>
public sealed record BvhQuery(uint Node, BvhRay Ray);
