// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation.Core;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// A console loop around an s_barrier in a mesh program runs one group round per iteration, in the two-phase and in the
/// superstep form. A few iterations draw the program's image; a garbage count (2^31) ends at the round bound
/// (LoopCaps.Rounds, here 64 x 1) instead of holding the group, which before only ss_steps (2^24 blocks) or nothing
/// bounded: w31's DispatchMesh(4, 1, 1) of MSNC_VS_LinearDepthSkin never finished.
/// </summary>
public sealed class MeshRoundBoundTests : IClassFixture<WarpFixture>
{
    private static readonly LoopCaps Small = new(64, 64, 64, 256);

    private readonly WarpFixture _fixture;

    public MeshRoundBoundTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpTheory]
    [InlineData(false)]
    [InlineData(true)]
    public void A_barrier_loop_draws_with_few_iterations_and_ends_at_the_round_bound_with_many(bool supersteps)
    {
        var runner = new MeshStageRunner(_fixture);
        var few = runner.RunBarrierLoop(supersteps, 3, Small);

        Assert.True(few.Count == 0, string.Join("\n", few.Take(8)));

        // Returns at all: the dispatch finished within the bound. Its exports come after the loop, so it draws nothing.
        Assert.NotEmpty(runner.RunBarrierLoop(supersteps, 0x80000000u, Small));
    }

    [WarpFact]
    public void A_superstep_loop_without_a_wave_operation_ends_at_the_block_bound_not_the_dispatch_forms()
    {
        // A loop that never reaches a group step is bounded only by the blocks a thread may run: 2 per allowed
        // iteration (here 64 x 2), not the dispatch form's 256 (w39/w40: MSNC_VS_LinearDepthSkin's constant-buffer count).
        var runner = new MeshStageRunner(_fixture);

        string spinning = runner.TranslateBarrierLoop(true, Small, spin: true);
        Assert.Contains("if (++ss_steps > 0x80u) { ss_cap_hit(0u); pc = PC_END; break; }", spinning, StringComparison.Ordinal);
        Assert.Contains("if (++ss_rounds > SS_ROUNDS) { if (g_ss_gi == 0u) ss_cap_hit(1u); break; }", spinning, StringComparison.Ordinal);
        Assert.Contains("if (++ss_steps > 0x2000u)", runner.TranslateBarrierLoop(true, LoopCaps.Default, spin: true), StringComparison.Ordinal);

        var few = runner.RunBarrierLoop(true, 3, Small, spin: true);
        Assert.True(few.Count == 0, string.Join("\n", few.Take(8)));
        Assert.NotEmpty(runner.RunBarrierLoop(true, 0x80000000u, Small, spin: true));
    }

    [WarpFact]
    public void A_superstep_program_names_its_stops_and_a_per_stop_budget_bounds_its_rounds()
    {
        var runner = new MeshStageRunner(_fixture);
        string bounded = runner.TranslateBarrierLoop(true, LoopCaps.Default, roundsPerStop: 4);

        // A few stops: the budget's floor of 256 rounds, below the caps' 4,096.
        Assert.Matches(@"// superstep: \d+ stops, round bound 256\n", bounded);
        Assert.Contains("const uint SS_ROUNDS = 0x100u;", bounded, StringComparison.Ordinal);
        Assert.Contains("round bound 4096", runner.TranslateBarrierLoop(true, LoopCaps.Default), StringComparison.Ordinal);

        // Every primitive starts culled after SetMeshOutputCounts, and an export's indices stay inside the vertex count.
        foreach (string form in new[] { bounded, runner.TranslateBarrierLoop(false, LoopCaps.Default) })
        {
            int counts = form.IndexOf("SetMeshOutputCounts(", StringComparison.Ordinal);
            Assert.Contains("prims[in_gi].cull = true;", form[counts..(counts + 300)], StringComparison.Ordinal);
            Assert.Contains("tris[in_gi] = min(_i, _n - 1u); prims[in_gi].cull = (_p >> 31) != 0 || any(_i >= _n);", form, StringComparison.Ordinal);
        }

        // With the default caps and the budget, a garbage count still ends (in 256 rounds).
        Assert.NotEmpty(runner.RunBarrierLoop(true, 0x80000000u, LoopCaps.Default, roundsPerStop: 4));
    }
}
