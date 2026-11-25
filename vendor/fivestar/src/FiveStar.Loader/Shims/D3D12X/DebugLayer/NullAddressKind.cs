// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>What <see cref="NullAddressCheck"/> dropped.</summary>
internal enum NullAddressKind
{
    /// <summary>A root CBV, SRV or UAV address (the user-data ring, the argument clamp's views).</summary>
    RootDescriptor,

    /// <summary>An ExecuteIndirect argument or count buffer that is missing or read past its end.</summary>
    IndirectBuffer,

    /// <summary>A write of the shim's own: a bottom-of-pipe value, a split buffer copy.</summary>
    OwnWrite,
}
