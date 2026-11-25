// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Handwritten gfx1013 pixel-shader programs (VALU, SALU, packed math, cross-lane, EXEC, helper lanes, control flow,
/// memory, exports) translated, run on WARP and compared bit-exactly with the ISA reference model.
/// </summary>
public sealed class PixelShaderTests : IClassFixture<WarpFixture>
{
    private readonly WarpFixture _fixture;

    public PixelShaderTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    public static TheoryData<string> Cases => [.. PixelCases.All().Select(testCase => testCase.Name)];

    [WarpTheory]
    [MemberData(nameof(Cases))]
    public void A_pixel_program_computes_what_the_isa_defines(string name)
    {
        var mismatches = new PixelCaseRunner(_fixture).Run(PixelCases.Find(name));

        Assert.True(mismatches.Count == 0, $"{mismatches.Count} mismatches:\n{string.Join("\n", mismatches.Take(16))}");
    }

    [WarpTheory]
    [MemberData(nameof(Cases))]
    public void The_dxil_backend_shades_what_the_isa_defines(string name)
    {
        var mismatches = new PixelCaseRunner(_fixture).Run(PixelCases.Find(name), direct: true);

        Assert.True(mismatches.Count == 0, $"{mismatches.Count} mismatches:\n{string.Join("\n", mismatches.Take(16))}");
    }
}
