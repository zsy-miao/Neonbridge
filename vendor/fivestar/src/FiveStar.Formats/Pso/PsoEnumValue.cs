// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>One value of an enum: its name hash and integer value.</summary>
public readonly record struct PsoEnumValue(uint Hash, long Value);
