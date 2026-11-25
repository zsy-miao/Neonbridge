// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Shader;
using FiveStar.Cli.Tests.Support;
using FiveStar.Shaders.Cache;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>The fixed-function state of the cache's graphics pipelines decodes to D3D12 values.</summary>
public sealed class PipelineStateTests
{
    [Fact]
    public void Blend_depth_and_cull_registers_decode_to_d3d12_values()
    {
        // Premultiplied-style blend on target 0 (SRC_ALPHA, INV_SRC_ALPHA; alpha ONE, INV_SRC_ALPHA), write RGB;
        // reverse-Z GEQUAL with depth write and stencil REPLACE on pass; cull back; depth clip off.
        uint[] blend = [0xE5010504, 0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x80000000];
        var registers = new GraphicsStateRegisters(
            0x4005D808, false, 2, 0x4, 0xF, 0x0C080000, 0x00201A42, 0.003f, 29f, 1f, 0x007007E7, 0x00030030,
            0x01FFFF00, 0x01FFFF00, 0x7, blend, 0x5, 0x1, 0, 0x00100000);

        var state = FixedFunctionDecoder.Decode(registers);

        Assert.Equal(new BlendTarget(true, 5, 6, 1, 2, 6, 1, 7), state.Blend[0]);
        Assert.False(state.Blend[1].Enable);
        Assert.Equal(new[] { new RenderTargetSlot(0, 28, 5, 4) }, state.RenderTargets);

        Assert.Equal((true, true, 7, true), (state.DepthStencil.DepthEnable, state.DepthStencil.DepthWrite, state.DepthStencil.DepthFunc, state.DepthStencil.StencilEnable));
        Assert.Equal(new StencilFace(1, 1, 3, 8), state.DepthStencil.Front);
        Assert.Equal(new RasterizerState(3, 3, true, 1, 0.003f, 29f / 16, false), state.Rasterizer);
        Assert.Equal((3, 0), (state.TopologyType, state.ForcedSampleCount));
    }

    [DumpFact]
    public void Every_graphics_state_decodes_and_agrees_with_the_shaders_it_runs()
    {
        var cache = PipelineCacheSource.Load(Path.Combine(DumpFactAttribute.Folder!, "shaders.rpf"));
        var states = cache.Graphics.Select(pipeline => FixedFunctionDecoder.Decode(cache.State(pipeline)!)).ToList();

        Assert.Equal(cache.Graphics.Count, states.Count);
        Assert.Equal((13534, 11875, 62), (states.Count(state => state.Rasterizer.CullMode == 1), states.Count(state => state.Rasterizer.CullMode == 3), states.Count(state => state.Rasterizer.CullMode == 2)));
        Assert.Equal((10098, 1937, 960), (states.Count(state => state.Blend[0].Enable), states.Count(state => state.DualSource), states.Count(state => !state.Rasterizer.DepthClipEnable)));

        // Pipelines without a pixel shader have no colour targets.
        var noPixel = cache.Graphics.Where(pipeline => !pipeline.Stage(PipelineSlot.Pixel).HasValue).ToList();
        Assert.All(noPixel, pipeline => Assert.Equal(0, states[pipeline.Index].RenderTargetCount));

        if (DumpFactAttribute.ShaderFolder is null)
        {
            return;
        }

        // Depth clipping is off for shadow-cascade passes; no "Opaque" pixel shader blends.
        var programs = CacheProgramMap.Build(cache, ProgramIndex.Load(DumpFactAttribute.ShaderFolder));
        var names = cache.Graphics.Select(pipeline => string.Join(' ', pipeline.Stages.SelectMany(link => programs.Programs(link.Shader)))).ToList();
        var clipOff = Enumerable.Range(0, states.Count).Where(index => !states[index].Rasterizer.DepthClipEnable && names[index].Length > 0).ToList();

        Assert.Equal(945, clipOff.Count);
        Assert.Equal(939, clipOff.Count(index => names[index].Contains("Shadow", StringComparison.OrdinalIgnoreCase) || names[index].Contains("Cascade", StringComparison.Ordinal)));
        Assert.DoesNotContain(Enumerable.Range(0, states.Count), index => states[index].Blend[0].Enable && Pixel(cache, programs, index).Contains("Opaque", StringComparison.Ordinal));
    }

    private static string Pixel(PipelineCache cache, CacheProgramMap programs, int index)
    {
        var link = cache.Graphics[index].Stage(PipelineSlot.Pixel);

        if (!link.HasValue)
        {
            return string.Empty;
        }

        return string.Join(' ', programs.Programs(link.Value.Shader));
    }
}
