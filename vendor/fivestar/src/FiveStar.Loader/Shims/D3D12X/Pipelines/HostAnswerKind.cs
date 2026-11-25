// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>What the compile host made of a request; the first three are the backend's verdicts (<see cref="FiveStar.Shaders.Compilation.BackendVerdict"/>).</summary>
internal enum HostAnswerKind
{
    /// <summary>Compiled: the DXIL is in place.</summary>
    Compiled = 0,

    /// <summary>The backend could not lower it: dxc must compile it.</summary>
    NotLowered = 1,

    /// <summary>The validator rejected the backend's container: dxc must compile it.</summary>
    Rejected = 2,

    /// <summary>A make the translator refused or no form of which loads: kept as a failure.</summary>
    Refused = 3,

    /// <summary>An error in the host: the loader does the work itself.</summary>
    Failed = 4,
}
