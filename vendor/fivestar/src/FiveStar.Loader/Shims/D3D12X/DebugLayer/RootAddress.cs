// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>A root descriptor address inside the transient pool (<see cref="TransientPoolWatch"/>).</summary>
internal readonly record struct RootAddress(ulong Address, nint Pipeline, bool Compute, long At);
