// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Typed image views: a load, gather or store picks a uint4 or int4 declaration of the descriptor by the class the loader
/// writes in T# dword 3 (the stencil plane is a UINT view), and a d16 load of an integer view keeps the low 16 bits.
/// </summary>
public sealed class TypedImageViewsTests
{
    private static readonly string[] Program =
    [
        "image_load v0, v[2:3], s[0:7] dmask:0x2 dim:SQ_RSRC_IMG_2D",
        "image_load v4, v[2:3], s[0:7] dmask:0x3 dim:SQ_RSRC_IMG_2D d16",
        "image_store v[5:8], v[2:3], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D",
        "image_sample v9, v[2:3], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D",
        "s_endpgm",
    ];

    [Fact]
    public void Integer_views_are_read_and_written_through_integer_declarations_chosen_by_the_class_in_dword_3()
    {
        string typed = Translate(typedViews: true);
        string plain = Translate(typedViews: false);

        Assert.Contains("Texture2D<uint4> _tex", typed, StringComparison.Ordinal);
        Assert.Contains("Texture2D<int4> _tex", typed, StringComparison.Ordinal);
        Assert.Contains("& 3u);", typed, StringComparison.Ordinal);
        Assert.Contains("== 1u) _img", typed, StringComparison.Ordinal);
        Assert.Contains("& 0xffffu, f32tof16", typed, StringComparison.Ordinal);
        Assert.Contains("RWTexture2D<uint4> _uav", typed, StringComparison.Ordinal);

        // A sample filters: it stays float4, and without the option nothing changes.
        Assert.Equal(2, System.Text.RegularExpressions.Regex.Count(typed, "Texture2D<uint4> _tex"));
        Assert.DoesNotContain("<uint4>", plain, StringComparison.Ordinal);
        Assert.DoesNotContain("<int4>", plain, StringComparison.Ordinal);
    }

    private static string Translate(bool typedViews)
    {
        var instructions = Program.Select((text, index) => new Instruction(index * 8, 8, text)).ToList();
        var compute = new ComputeConfig { Rsrc2 = 0, ScratchEnable = false, GroupSizeEnable = false, ThreadIdComponentCount = 1, LdsBlocks = 0, NumThreads = new ThreadGroupSize(64, 1, 1), GroupIdSgprs = [] };
        var config = new ShaderConfig { Stage = HardwareStage.Compute, WaveSize = 64, UserSgprs = 12, Compute = compute };
        var program = new ShaderProgram(ShaderStage.Compute, instructions, new byte[instructions.Count * 8], ShaderSignatures.None, config, null);
        var options = new TranslationOptions(StructuredControlFlow: true, PinWaveSize: true, EmulateWaveSize: true) { TypedImageViews = typedViews };

        return HlslTranslator.Translate(program, options).Hlsl;
    }
}
