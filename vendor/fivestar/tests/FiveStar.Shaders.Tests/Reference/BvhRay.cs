// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>One ray of an intersect query: extent (t max), origin, direction and inverse direction, all f32 values.</summary>
public sealed record BvhRay(double Extent, double[] Origin, double[] Direction, double[] InverseDirection);
