// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Mods;

/// <summary>What happened to a patch when its module was mapped.</summary>
internal enum PatchStatus
{
    Applied,

    /// <summary>Disabled, or the module's SHA-256 differs; nothing was written.</summary>
    Skipped,

    /// <summary>The pattern was not found or matched a different number of times, or the target is invalid; nothing was written.</summary>
    Failed,
}
