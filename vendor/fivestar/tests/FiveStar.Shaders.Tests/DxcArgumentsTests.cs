// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// dxc compiles the stages that write SV_Position with IEEE strictness (-Gis), so a depth prepass and a pass testing
/// EQUAL round the same position the same; other stages keep fast math.
/// </summary>
public sealed class DxcArgumentsTests
{
    [Theory]
    [InlineData("vs", true)]
    [InlineData("ms", true)]
    [InlineData("gs", true)]
    [InlineData("ps", false)]
    [InlineData("cs", false)]
    public void Position_stages_compile_without_fast_math(string profile, bool strict)
    {
        var arguments = DxcCompiler.Arguments(new CompileRequest(Path.Combine(Path.GetTempPath(), "program.hlsl"), string.Empty, profile, DisableOptimizations: false));

        Assert.Equal(strict, arguments.Contains("-Gis"));
        Assert.EndsWith("program.hlsl", arguments[^1], StringComparison.Ordinal);
    }
}
