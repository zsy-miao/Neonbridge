// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// A compute program whose control flow cannot be structured runs in the dispatch form, where each thread runs the
/// switch on its own; an s_barrier there keeps only its memory fence, since a thread that ended or sits in another
/// block never reaches it and the group would hang.
/// </summary>
public sealed class DispatchBarrierTests : IClassFixture<WarpFixture>
{
    // A loop entered at two blocks (irreducible), with an s_barrier in one of them.
    private const string Irreducible = """
        s_cmp_eq_u32 s0, 0
        s_cbranch_scc1 .Lsecond
        .Lfirst:
        v_add_nc_u32_e32 v0, 1, v0
        s_barrier
        .Lsecond:
        v_add_nc_u32_e32 v1, 1, v1
        s_cmp_lg_u32 s1, 0
        s_cbranch_scc1 .Lfirst
        s_endpgm
        """;

    private readonly WarpFixture _fixture;

    public DispatchBarrierTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpFact]
    public void A_barrier_in_the_dispatch_switch_keeps_only_its_fence()
    {
        var assembled = _fixture.Workbench.Assemble("cs_irreducible_barrier", Irreducible, 64);
        var compute = new ComputeConfig { Rsrc2 = 0, ScratchEnable = false, GroupSizeEnable = false, ThreadIdComponentCount = 1, LdsBlocks = 0, NumThreads = new ThreadGroupSize(128, 1, 1), GroupIdSgprs = [] };
        var config = new ShaderConfig { Stage = HardwareStage.Compute, WaveSize = 64, UserSgprs = 4, Compute = compute };
        var program = new ShaderProgram(ShaderStage.Compute, assembled.Instructions, assembled.Code, ShaderSignatures.None, config, null);

        string hlsl = ShaderWorkbench.Translate(program);

        Assert.Contains("control flow: dispatch loop", hlsl, StringComparison.Ordinal);
        Assert.Contains("GroupMemoryBarrier();   // dispatch switch: fence only", hlsl, StringComparison.Ordinal);
        Assert.DoesNotContain("GroupMemoryBarrierWithGroupSync();\n", hlsl[hlsl.IndexOf("switch (pc)", StringComparison.Ordinal)..], StringComparison.Ordinal);
    }
}
