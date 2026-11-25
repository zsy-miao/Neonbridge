// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The group-wave variant of a 64-thread group (one console wave, two 32-lane hardware waves) orders an LDS read after
/// an LDS store with a group barrier outside any lane condition; a wave operation orders it already, atomics after
/// atomics need none, and a group of 32 threads (one hardware wave) gets none.
/// </summary>
public sealed class GroupLdsBarrierTests
{
    private const string Barrier = "GroupMemoryBarrierWithGroupSync();   // LDS written by the other hardware wave";

    private const string Program = """
        groupshared uint g_lds[64];
        [WaveSize(64)]
        [numthreads(THREADS, 1, 1)]
        void main(uint3 in_tid : SV_GroupThreadID) {
          bool active = true;
          if (active) g_lds[in_tid.x] = 1u;
          if (active) v0 = g_lds[(in_tid.x + 1u) & 63u];
          if (active) g_lds[5] = 2u;
          uint _a = 0; if (active) InterlockedAdd(g_lds[0], 1u, _a);
          uint _b = 0; if (active) InterlockedAdd(g_lds[1], 1u, _b);
          if (vw_any(active)) {
            if (active) g_lds[2] = 3u;
          }
          if (active) {
            v1 = g_lds[2];
          }
          if (scc) {
            if (active) g_lds[3] = 4u;
          } else {
            v2 = 0u;
          }
          v3 = g_lds[3];
          v4 = vw_read(v3, 1u);
          v5 = g_lds[4];
        }
        """;

    [Fact]
    public void Reads_after_stores_of_the_other_hardware_wave_get_a_barrier_outside_lane_conditions()
    {
        var lines = HlslTranslator.GroupWaveVariant(Program.Replace("THREADS", "64", StringComparison.Ordinal)).Split('\n').Select(line => line.Trim()).ToList();

        // Before the read of the neighbour's store, before the first atomic after a store (not the second), before the lane
        // block that reads the branch's store (not inside it), before the read after the if/else; none after vw_read.
        Assert.Equal(4, lines.Count(line => line == Barrier));

        Assert.Equal(Barrier, lines[lines.FindIndex(line => line.StartsWith("if (active) v0", StringComparison.Ordinal)) - 1]);
        Assert.Equal(Barrier, lines[lines.FindIndex(line => line.StartsWith("uint _a", StringComparison.Ordinal)) - 1]);
        Assert.NotEqual(Barrier, lines[lines.FindIndex(line => line.StartsWith("uint _b", StringComparison.Ordinal)) - 1]);
        Assert.Equal(Barrier, lines[lines.FindIndex(line => line == "if (active) {") - 1]);
        Assert.Equal(Barrier, lines[lines.FindIndex(line => line.StartsWith("v3 = g_lds", StringComparison.Ordinal)) - 1]);
        Assert.NotEqual(Barrier, lines[lines.FindIndex(line => line.StartsWith("v5 = g_lds", StringComparison.Ordinal)) - 1]);
    }

    [Fact]
    public void A_group_of_one_hardware_wave_needs_none()
    {
        Assert.DoesNotContain(Barrier, HlslTranslator.GroupWaveVariant(Program.Replace("THREADS", "32", StringComparison.Ordinal)), StringComparison.Ordinal);
    }
}
