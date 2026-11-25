// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>An EXTRQ/INSERTQ bit field: its length (0 means 64) and the index of its lowest bit, each 6 bits.</summary>
internal readonly record struct BitFieldSpec(int Length, int Index);
