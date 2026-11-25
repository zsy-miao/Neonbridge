// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>Compiles translated HLSL to DXIL.</summary>
public interface IShaderCompiler
{
    DxcOptions Options { get; }

    /// <summary>A hash of everything besides the request that decides the output: the compiler build and its fixed arguments.</summary>
    string Identity { get; }

    /// <summary>Compiles one source and writes <see cref="CompileRequest.DxilPath"/> when it succeeds.</summary>
    CompileOutcome Compile(CompileRequest request, TimeSpan timeout);
}
