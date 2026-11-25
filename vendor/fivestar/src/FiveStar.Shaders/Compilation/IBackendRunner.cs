// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// The DXIL backend's part of a compile (lower, validate, write <see cref="CompileRequest.DxilPath"/>), in this
/// process (<see cref="InProcessBackend"/>) or another; <see cref="DxilBackendCompiler"/> sends the rest to dxc.
/// </summary>
public interface IBackendRunner
{
    /// <summary>Whether dxil.dll's validator checks every container.</summary>
    bool Validates { get; }

    /// <summary>Compiles one request; the DXIL is written only when the verdict is <see cref="BackendVerdict.Compiled"/>.</summary>
    BackendResult Run(CompileRequest request);
}
