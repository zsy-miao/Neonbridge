// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// How a make in the compile host went (<see cref="ProgramProducer.MakeInHost"/>). Not <paramref name="Hosted"/>: the
/// loader makes the program itself, with dxc alone when <paramref name="DxcOnly"/> (the host ended on it). Hosted: made,
/// unless <paramref name="Failure"/> (kept by the host) or <paramref name="Dxc"/>, a form for dxc to compile.
/// </summary>
internal sealed record HostedMake(bool Hosted, bool DxcOnly, string? Failure, ProgramForm? Dxc)
{
    /// <summary>No host: the loader makes the program as before.</summary>
    public static HostedMake Here { get; } = new(false, false, null, null);
}
