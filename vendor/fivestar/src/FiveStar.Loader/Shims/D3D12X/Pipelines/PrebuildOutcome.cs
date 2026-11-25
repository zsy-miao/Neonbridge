// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>What the journal prebuild did with one pipeline (<see cref="JournalPrebuild"/>).</summary>
internal enum PrebuildOutcome
{
    /// <summary>The driver built it and the cache stored it.</summary>
    Built,

    /// <summary>The cache had it, or this boot's prebuild took it already.</summary>
    Cached,

    /// <summary>The compile budget had no room now: tried again in a later pass.</summary>
    GivenBack,

    /// <summary>A program is missing or its linked producer is not made yet.</summary>
    NotReady,

    /// <summary>The driver refused it.</summary>
    Refused,
}
