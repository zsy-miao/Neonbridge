// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>What happens to an import nothing resolves.</summary>
internal enum UnresolvedMode
{
    /// <summary>Bind it to a stub that logs the first call and returns 0 (or the module's unresolved value).</summary>
    Stub,

    /// <summary>Bind it to a stub that logs the call and terminates the process.</summary>
    Trap,

    /// <summary>Fail the load.</summary>
    Error,
}
