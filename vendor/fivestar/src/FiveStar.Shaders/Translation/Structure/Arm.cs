// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>One arm of a choice: the scope-local node it starts at and the successor positions that lead there.</summary>
internal sealed record Arm(int Target, List<int> Indices);
