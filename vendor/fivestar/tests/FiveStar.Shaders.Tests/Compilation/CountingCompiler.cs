// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Shaders.Tests.Compilation;

/// <summary>A stand-in for dxc that counts its compiles: sources containing "bad" fail, others become their own bytes.</summary>
public sealed class CountingCompiler : IShaderCompiler
{
    private int _compiles;

    public DxcOptions Options { get; } = DxcOptions.Defaults("dxc", retry: false);

    public string Identity => "counting";

    public int Compiles => _compiles;

    public CompileOutcome Compile(CompileRequest request, TimeSpan timeout)
    {
        Interlocked.Increment(ref _compiles);

        if (request.Hlsl.Contains("bad", StringComparison.Ordinal))
        {
            return CompileOutcome.Failure("x.hlsl:1:1: error: bad");
        }

        File.WriteAllBytes(request.DxilPath, Encoding.UTF8.GetBytes(request.Hlsl));

        return CompileOutcome.Success;
    }
}
