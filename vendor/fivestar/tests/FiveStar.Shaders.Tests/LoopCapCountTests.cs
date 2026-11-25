// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The loop cap only keeps the device alive; reaching it is a bug to fix (a wrong input or translation). A program
/// whose loops spend the cap counts it in its slot of the cap table (dword 3, once per wave), which the loader reports
/// by name; one that stays under the cap counts nothing.
/// </summary>
public sealed class LoopCapCountTests : IClassFixture<WarpFixture>
{
    private const uint Id = 0x2A7;
    private const int LoopDword = 3;

    private readonly WarpFixture _fixture;

    public LoopCapCountTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void A_program_that_reaches_the_loop_cap_is_counted_and_one_under_it_is_not()
    {
        var runner = new ComputeCaseRunner(_fixture);
        var nested = ComputeCases.All().Single(testCase => testCase.Name == "cs_loop_cap");

        // Two nested loops of 1,000 iterations: past the compute cap of 65,536 in every lane.
        var (mismatches, slot) = runner.RunCapCounted(nested, Id);
        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches.Take(8)));
        Assert.Equal(Id, slot[0]);
        Assert.True(slot[LoopDword] > 0, $"no wave counted past the loop cap: {string.Join(", ", slot)}");
        Assert.Equal(0u, slot[1]);
        Assert.Equal(0u, slot[2]);

        // Bounds of 10: 100 iterations, nothing counted.
        var (few, quiet) = runner.RunCapCounted(nested with { User = [10], Expect = _ => [10 * 10] }, Id);
        Assert.True(few.Count == 0, string.Join("\n", few.Take(8)));
        Assert.Equal([0u, 0u, 0u, 0u], quiet);
    }
}
