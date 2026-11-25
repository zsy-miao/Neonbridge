// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// A program's new cap hits since the last report (<see cref="WindowGuard.CapHits"/>): threads past the superstep block
/// cap, groups past the round bound, waves past the loop cap.
/// </summary>
internal readonly record struct CapHit(uint Id, uint Blocks, uint Rounds, uint Loops);
