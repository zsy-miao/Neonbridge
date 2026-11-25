// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The form of a program the adapter runs, translated and ready to compile (<see cref="ProgramProducer"/>), or why there
/// is none ("cause: detail").
/// </summary>
internal sealed record ProgramForm(string Name, CompileRequest? Request, string? Failure)
{
    /// <summary>Source length from which a compile runs on the heavy thread: dxc takes up to minutes for these.</summary>
    public const int HeavySourceLength = 2_000_000;

    /// <summary>Whether the compile goes to the heavy thread.</summary>
    public bool Heavy => Request is not null && Request.Hlsl.Length >= HeavySourceLength;
}
