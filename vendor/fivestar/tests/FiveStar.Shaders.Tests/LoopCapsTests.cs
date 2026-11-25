// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation;
using FiveStar.Shaders.Translation.Core;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>The loop caps a translation runs with, by stage, and how they are named on a command line.</summary>
public sealed class LoopCapsTests
{
    [Fact]
    public void Victory_caps_bound_each_stage_and_the_dispatch_form()
    {
        var caps = LoopCaps.Parse("victory")!;

        Assert.Equal(4096, caps.Iterations(ShaderStage.Compute));
        Assert.Equal(256, caps.Iterations(ShaderStage.Mesh));
        Assert.Equal(256, caps.Iterations(ShaderStage.Vertex));
        Assert.Equal(128, caps.Iterations(ShaderStage.Pixel));
        Assert.Equal(256 * 256, caps.DispatchSteps(ShaderStage.Mesh));
        Assert.Equal(256, caps.Rounds(ShaderStage.Mesh));

        // The default caps bound a mesh program's dispatch blocks at 2^20, its superstep blocks at 2^13 and its group rounds at 2^12; compute keeps 2^24 blocks.
        Assert.Equal(1L << 20, LoopCaps.Default.DispatchSteps(ShaderStage.Mesh));
        Assert.Equal(1L << 12, LoopCaps.Default.Rounds(ShaderStage.Mesh));
        Assert.Equal(1L << 13, LoopCaps.Default.SuperstepSteps(ShaderStage.Mesh));
        Assert.Equal(LoopGuard.DispatchSteps, LoopCaps.Default.DispatchSteps(ShaderStage.Compute));
    }

    [Fact]
    public void Caps_are_three_positive_numbers_or_a_preset()
    {
        Assert.Equal(new LoopCaps(1000, 200, 50, 256), LoopCaps.Parse("1000,200,50"));
        Assert.Same(LoopCaps.Default, LoopCaps.Parse("default"));
        Assert.Null(LoopCaps.Parse("1000,200"));
        Assert.Null(LoopCaps.Parse("1000,0,50"));
    }
}
