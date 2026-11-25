// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>One block of a classic PSO map: struct hash, file offset and length.</summary>
public readonly record struct ClassicPsoBlock(uint StructHash, uint Offset, uint Length);
