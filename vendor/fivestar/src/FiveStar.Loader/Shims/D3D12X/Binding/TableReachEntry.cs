// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>A user SGPR holding a table pointer and the records a pipeline's programs read past it (<see cref="TableReaches"/>).</summary>
internal readonly record struct TableReachEntry(int Sgpr, uint Records);
