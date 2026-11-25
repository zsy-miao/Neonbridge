// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>The VOP3P per-source bits: half select for the low and high results, and negation per result half.</summary>
internal sealed record PackedLanes(List<int> Select, List<int> SelectHigh, List<int> NegateLow, List<int> NegateHigh);
