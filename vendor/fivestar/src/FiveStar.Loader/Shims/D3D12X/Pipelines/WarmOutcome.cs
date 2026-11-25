// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>What the warm-up did with one pipeline.</summary>
internal enum WarmOutcome
{
    /// <summary>The driver built it and the cache stored it.</summary>
    Built,

    /// <summary>The cache already had it, an earlier run saw it refused, or another record makes the same pipeline.</summary>
    Skipped,

    /// <summary>A shader has no translated program (the game's creation fails the same way).</summary>
    Untranslated,

    /// <summary>The driver refused it.</summary>
    Refused,

    /// <summary>A heavy pipeline left out for lack of memory; the next run tries again.</summary>
    Deferred,
}
