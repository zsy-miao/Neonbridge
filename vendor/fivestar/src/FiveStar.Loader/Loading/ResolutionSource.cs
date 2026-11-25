// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>Where an import was bound.</summary>
internal enum ResolutionSource
{
    /// <summary>A shim of this loader (including not-implemented stubs).</summary>
    Shim,

    /// <summary>An export of an image this loader mapped.</summary>
    Image,

    /// <summary>An export of a System32 module loaded by the OS.</summary>
    System,

    /// <summary>Nothing: an unresolved-import stub.</summary>
    Unresolved,
}
