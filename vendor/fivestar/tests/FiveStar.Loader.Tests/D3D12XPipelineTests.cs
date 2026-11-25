// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Programs;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Cache;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The pipeline descriptions of the D3D12.X device slots, built to the layout the game's code writes.</summary>
public sealed unsafe class D3D12XPipelineTests
{
    [Fact]
    public void A_pipeline_stream_laid_out_like_the_game_reads_into_desktop_state()
    {
        using var memory = new NativeBlocks();
        var stream = GameStream(memory);

        var description = PipelineStreamParser.Parse(stream, 0x2B0);

        Assert.Equal((nint)0x1234, description.GameRootSignature);
        Assert.Equal(["d1", "d2d2"], description.Shaders.Select(shader => Convert.ToHexStringLower(shader.Bytes)));
        Assert.All(description.Shaders, shader => Assert.Equal(ShaderSourceKind.Container, shader.Kind));
        Assert.Equal((1, 5), (description.Blend[0x08], description.Blend[0x10]));
        Assert.Equal((2, 7), (description.Rasterizer[0x04], description.DepthStencil[0x08]));

        Assert.Equal(0xFFFFFFFFu, description.SampleMask);
        Assert.Equal(["POSITION", "TEXCOORD"], description.InputLayout.Select(element => element.Semantic));
        Assert.Equal((12, 1), (description.InputLayout[1].Offset, description.InputLayout[1].InputSlot));
        Assert.Equal((3, 28, 40, 1), (description.TopologyType, description.RenderTargetFormats.Single(), description.DepthStencilFormat, description.SampleCount));
    }

    [Fact]
    public void Dual_source_blending_and_a_forced_sample_count_with_depth_become_desktop_states()
    {
        // The game repeats target 0's SRC1_COLOR blend on all eight targets with independent blending on, and asks
        // for 16 forced samples with depth on; the desktop runtime refuses both (debug-layer errors 112 and 672).
        var blend = DesktopStateBytes.DefaultBlend();
        DesktopStateBytes.Write(blend, DesktopStateBytes.IndependentBlendField, true);

        for (var target = 0; target < 8; target++)
        {
            DesktopStateBytes.WriteTarget(blend, target, true, [2, 16, 1, 2, 16, 1], 0x0F);
        }

        var rasterizer = DesktopStateBytes.DefaultRasterizer();
        rasterizer[DesktopStateBytes.ForcedSampleCountField] = 16;

        var desktopBlend = DesktopStateRules.Blend(blend);
        var desktopRasterizer = DesktopStateRules.Rasterizer(rasterizer, DesktopStateBytes.DefaultDepthStencil());

        Assert.Equal((0, 16, 0), (desktopBlend[DesktopStateBytes.IndependentBlendField], desktopBlend[0x08 + DesktopStateBytes.DestBlendField], desktopBlend[0x08 + 0x28]));
        Assert.Equal(1, desktopBlend[0x08 + 0x28 + DesktopStateBytes.DestBlendField]);
        Assert.Equal(0, desktopRasterizer[DesktopStateBytes.ForcedSampleCountField]);
        Assert.Same(rasterizer, DesktopStateRules.Rasterizer(rasterizer, new byte[0x34 + 8]));
    }

    [Fact]
    public void Derive_overrides_are_read_at_their_stride_with_the_desc_after_the_kind()
    {
        using var memory = new NativeBlocks();
        var entries = memory.Alloc(2 * 0x14C);
        *(int*)(entries + 4 + 4) = 2;
        *(int*)(entries + 0x14C) = 2;
        *(entries + 0x14C + 4 + 8) = 1;

        var overrides = PipelineDescReader.Overrides(2, entries);

        Assert.Equal([0, 2], overrides.Select(entry => entry.Kind));
        Assert.Equal((2, 1), (overrides[0].Payload[4], overrides[1].Payload[8]));
    }

    [Fact]
    public void Derive_kind_0_is_the_depth_stencil_and_kind_1_the_rasterizer_as_the_game_writes_them()
    {
        // 0x142479f40: kind 0 = {DepthEnable 1, write mask ALL, LESS_EQUAL}, kind 1 = {SOLID, CULL_FRONT}; read the
        // other way round they were FillMode 1, CullMode 1 and a depth-write mask of 2 (the debug layer's errors).
        using var memory = new NativeBlocks();
        var entries = memory.Alloc(2 * 0x14C);
        *(int*)(entries + 4) = 1;
        *(int*)(entries + 8) = 1;
        *(int*)(entries + 12) = 4;

        *(int*)(entries + 0x14C) = 1;
        *(int*)(entries + 0x14C + 4) = 3;
        *(int*)(entries + 0x14C + 8) = 2;

        var kinds = PipelineDescReader.Overrides(2, entries).Select(entry => (OverrideKind)entry.Kind);

        Assert.Equal([OverrideKind.DepthStencil, OverrideKind.Rasterizer], kinds);
    }

    [Fact]
    public void The_console_rectangle_topology_type_and_undefined_become_triangle()
    {
        using var memory = new NativeBlocks();
        var stream = GameStream(memory);
        *(int*)(stream + 0x27C) = TopologyTypes.ConsoleRectangle;

        Assert.Equal(3, PipelineStreamParser.Parse(stream, 0x2B0).TopologyType);
        int[] types = [0, 1, 2, 4];
        Assert.Equal([3, 1, 2, 4], types.Select(TopologyTypes.Desktop));
    }

    [Fact]
    public void A_cached_pipeline_takes_its_records_output_formats_over_the_register_guess()
    {
        // Reverse-Z depth with stencil on, one exported target the registers read as R8G8B8A8_UNORM.
        uint[] blend = [0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x80000000, 0x80000000];
        var registers = new GraphicsStateRegisters(
            0x4005D808, false, 2, 0x4, 0xF, 0, 0x00201A42, 0f, 0f, 0f, 0x007007E7, 0x00030030,
            0x01FFFF00, 0x01FFFF00, 0xF, blend, 0x5, 0x1, 0, 0x00100000);
        var state = FixedFunctionDecoder.Decode(registers);

        var guessed = CachedStateMapping.Describe(state, null, 0, [], []);
        var stored = CachedStateMapping.Describe(state, new PipelineTargets([29, 10], 118, 3, 1, uint.MaxValue), 0, [], []);
        var depthOnly = CachedStateMapping.Describe(state, new PipelineTargets([], 55, 3, 1, uint.MaxValue), 0, [], []);

        Assert.Equal([28], guessed.RenderTargetFormats);
        Assert.Equal([29, 10], stored.RenderTargetFormats);
        Assert.Empty(depthOnly.RenderTargetFormats);

        // The console's D16_UNORM_S8_UINT (118) becomes D32_FLOAT_S8X24_UINT; a stored D16_UNORM stays.
        Assert.Equal((20, 20, 55), (guessed.DepthStencilFormat, stored.DepthStencilFormat, depthOnly.DepthStencilFormat));
    }

    [Fact]
    public void A_wave64_pinned_compute_program_loads_its_wave32_variant_on_a_32_lane_adapter()
    {
        var manifest = new ShaderManifest(
        [
            new ProgramVariants("a", "cs", "a.dxil", null, null, 64, "a.w32.dxil", null),
            new ProgramVariants("b", "cs", "b.dxil", null, null, 64, null, null),
            new ProgramVariants("c", "ps", "c.dxil", 64, 4, null, null, null),
        ]);
        var adapter = new AdapterInfo(0, "rtx", false, "6.8", 32, 32, "Tier1");
        var picker = new ProgramVariantPicker(manifest, adapter, new OnceLog(new LoaderLog(TextWriter.Null, null, false)));

        string[] names = ["a", "b", "c", "d"];
        Assert.Equal(["a.w32.dxil", null, "c.dxil", "d.dxil"], names.Select(name => picker.File(name, name + ".dxil")));
    }

    /// <summary>The 0x2B0-byte stream 0x14086f090 builds: fixed subobject offsets, empty DS/HS/GS slots.</summary>
    private static byte* GameStream(NativeBlocks memory)
    {
        var stream = memory.Alloc(0x2B0);
        (int Offset, int Type)[] types =
        [
            (0x00, 0), (0x10, 1), (0x28, 2), (0x40, 3), (0x58, 4), (0x70, 5), (0x88, 8), (0x1D8, 9), (0x1E0, 10),
            (0x210, 21), (0x250, 17), (0x260, 12), (0x278, 14), (0x280, 15), (0x2A8, 16),
        ];

        foreach (var (offset, type) in types)
        {
            *(int*)(stream + offset) = type;
        }

        *(nint*)(stream + 0x08) = 0x1234;
        memory.Range(stream + 0x18, [0xD1]);
        memory.Range(stream + 0x30, [0xD2, 0xD2]);

        *(int*)(stream + 0x8C + 0x08) = 1;
        *(int*)(stream + 0x8C + 0x10) = 5;
        *(uint*)(stream + 0x1DC) = uint.MaxValue;
        *(int*)(stream + 0x1E4 + 0x04) = 2;
        *(int*)(stream + 0x214 + 0x08) = 7;
        *(int*)(stream + 0x254) = 1;

        *(nint*)(stream + 0x268) = (nint)Elements(memory, ["POSITION", "TEXCOORD"]);
        *(int*)(stream + 0x270) = 2;
        *(int*)(stream + 0x27C) = 3;
        *(int*)(stream + 0x284) = 28;
        *(int*)(stream + 0x2A4) = 1;
        *(int*)(stream + 0x2AC) = 40;

        return stream;
    }

    /// <summary>D3D12_INPUT_ELEMENT_DESCs, element n in slot n at offset 12n.</summary>
    private static byte* Elements(NativeBlocks memory, string[] names)
    {
        var elements = memory.Alloc(names.Length * 0x20);

        for (var index = 0; index < names.Length; index++)
        {
            var element = elements + (index * 0x20);
            *(nint*)element = (nint)memory.Ansi(names[index]);
            *(int*)(element + 0x10) = index;
            *(int*)(element + 0x14) = index * 12;
        }

        return elements;
    }
}
