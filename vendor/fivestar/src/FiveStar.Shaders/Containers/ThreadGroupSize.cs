// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>A compute thread-group size (numthreads).</summary>
public readonly record struct ThreadGroupSize(int X, int Y, int Z);
