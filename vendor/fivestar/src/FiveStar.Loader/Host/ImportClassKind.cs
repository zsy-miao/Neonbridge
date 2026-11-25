// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Host;

/// <summary>How the loader would resolve an import, from the shim table and the module facts (no image mapped).</summary>
internal enum ImportClassKind
{
    /// <summary>A shim of the loader answers the function (a forward to a PC DLL is one of these).</summary>
    Shim,

    /// <summary>An Xbox-only module with no shim for the function: a logged stub returning a fixed value.</summary>
    XboxStub,

    /// <summary>A module of the package folder that the loader maps itself (the shipped CRT, middleware DLLs).</summary>
    Package,

    /// <summary>A module the PC host has (a System32 file or an implemented API set).</summary>
    Host,

    /// <summary>No shim, not in the package and not on this PC: an Xbox system module the loader does not know yet.</summary>
    Missing,
}
