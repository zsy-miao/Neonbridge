// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>The cap counts last seen per slot, one array per kind (<see cref="WindowGuard.CapHits"/>).</summary>
internal sealed record CapCounts(uint[] Blocks, uint[] Rounds, uint[] Loops);
