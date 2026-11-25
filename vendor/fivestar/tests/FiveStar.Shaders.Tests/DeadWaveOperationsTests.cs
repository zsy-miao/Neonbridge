// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using FiveStar.Shaders.Translation.Waves;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The superstep form drops the wave operations whose result nothing reads (every wave operation there is a group
/// round): a ballot into an SGPR half overwritten before any read, and an scc write overwritten or never read. The
/// ones a later block or branch reads stay. The WARP cases of the superstep form (compute, mesh, probe) check that the
/// programs still compute what the ISA model says.
/// </summary>
public sealed class DeadWaveOperationsTests
{
    [Fact]
    public void Only_the_wave_operations_a_later_read_needs_stay()
    {
        List<string> first =
        [
            "  uint _t1 = vw_ballot(m_s4).x;",
            "  s4 = _t1;",
            "  uint _t2 = vw_ballot(m_s4).y;",
            "  s5 = _t2;",
            "  scc = vw_any(m_s4);",
            "  uint _t3 = vw_ballot(m_s6).x;",
            "  s6 = _t3;",
            "  scc = vw_any(lane_bit(s6, s7));",
            "  s4 = 0x0u;",
            "  pc = 0x10;",
        ];
        List<string> second =
        [
            "  v1 = select(active, s5 + s4, v1);",
            "  pc = (scc) ? 0x20 : PC_END;",
        ];
        List<string> third = ["  pc = PC_END;"];
        var blocks = new List<CodeBlock>
        {
            new(0, first, new BlockExit(BlockExitKind.Jump, 0x10, null, null)),
            new(0x10, second, new BlockExit(BlockExitKind.Conditional, 0x20, 0x20, "scc")),
            new(0x20, third, new BlockExit(BlockExitKind.End, null, null, null)),
        };

        int removed = DeadWaveOperations.Remove(blocks, new HashSet<string>());

        // s4's ballot is overwritten; the first scc by the second; s6's ballot is read by the second scc, which the branch reads.
        Assert.Equal(3, removed);
        Assert.Equal(
            [
                "  uint _t2 = vw_ballot(m_s4).y;",
                "  s5 = _t2;",
                "  uint _t3 = vw_ballot(m_s6).x;",
                "  s6 = _t3;",
                "  scc = vw_any(lane_bit(s6, s7));",
                "  s4 = 0x0u;",
                "  pc = 0x10;",
            ],
            first);
    }

    [Fact]
    public void A_lanes_own_bit_of_a_ballot_is_read_from_the_mask_bool_until_either_is_written()
    {
        List<string> lines =
        [
            "  uint _t1 = vw_ballot(m_s4).x;",
            "  s4 = _t1;",
            "  uint _t2 = vw_ballot(m_s4).y;",
            "  s5 = _t2;",
            "  active = active && lane_bit(s4, s5);",
            "  m_s4 = active;",
            "  v1 = select(lane_bit(s4, s5), 1u, v1);",
            "  pc = PC_END;",
        ];
        var blocks = new List<CodeBlock> { new(0, lines, new BlockExit(BlockExitKind.End, null, null, null)) };

        Assert.Equal(1, BallotBits.Substitute(blocks));
        Assert.Equal("  active = active && (m_s4);", lines[4]);
        Assert.Equal("  v1 = select(lane_bit(s4, s5), 1u, v1);", lines[6]);
    }
}
