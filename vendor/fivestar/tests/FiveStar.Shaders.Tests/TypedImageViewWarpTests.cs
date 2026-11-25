// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>Typed image views on WARP: integer views load, d16-load and store their bits, by dxc and by the DXIL backend.</summary>
public sealed class TypedImageViewWarpTests : IClassFixture<WarpFixture>
{
    private readonly WarpFixture _fixture;

    public TypedImageViewWarpTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    public static TheoryData<string, bool> Cases()
    {
        var data = new TheoryData<string, bool>();

        foreach (var testCase in TypedViewCases.All())
        {
            data.Add(testCase.Name, false);
            data.Add(testCase.Name, true);
        }

        return data;
    }

    [WarpTheory]
    [MemberData(nameof(Cases))]
    public void An_integer_view_reads_and_writes_its_bits(string name, bool direct)
    {
        var testCase = TypedViewCases.All().Single(entry => entry.Name == name);

        var mismatches = new ComputeCaseRunner(_fixture).RunTypedViews(testCase, direct);

        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches.Take(8)));
    }
}
