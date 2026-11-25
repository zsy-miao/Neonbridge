// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Bvh;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--raytracing-gpu: compute pipelines asked for while the BVH builder's constructor runs are noted and answered, others are not touched.</summary>
public sealed unsafe class BvhBuilderInventoryTests
{
    [Fact]
    public void Only_pipelines_asked_for_during_the_constructor_are_captured()
    {
        using var test = TestRuntime.Create();
        nint* desc = stackalloc nint[3] { 0x1111, 0x2222, 0x330 };
        nint* result = stackalloc nint[1] { -1 };

        Assert.False(BvhBuilderInventory.Capture((byte*)desc, result));
        Assert.Equal(-1, *result);

        var pipelines = BvhBuilderInventory.During(() => Assert.True(BvhBuilderInventory.Capture((byte*)desc, result)));

        var single = Assert.Single(pipelines);
        Assert.Equal((0x2222, 0x330, 0x1111), (single.Bytecode, single.Length, single.RootSignature));
        Assert.False(BvhBuilderInventory.Capture((byte*)desc, result));
    }
}
