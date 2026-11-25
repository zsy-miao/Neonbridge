// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using FiveStar.Shaders.Translation.Waves;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// One group round serves every ballot, any and count of the mask the last round took, until something that mask
/// reads is written: a ballot's .y after its .x was a round of its own. The WARP cases of the superstep form (compute,
/// mesh, probe) check that the programs still compute what the ISA model says.
/// </summary>
public sealed class SuperstepSplitterTests
{
    [Fact]
    public void A_ballot_reuses_the_last_round_of_its_mask_until_the_mask_is_written()
    {
        List<string> lines =
        [
            "  uint _t1 = vw_ballot(m_s4).x;",
            "  s4 = _t1;",
            "  uint _t2 = vw_ballot(m_s4).y;",
            "  s5 = _t2;",
            "  scc = vw_any((m_s4));",
            "  m_s4 = active && lane_bit(s4, s5);",
            "  uint _t3 = vw_ballot(m_s4).x;",
            "  s6 = _t3;",
            "  pc = PC_END;",
        ];
        var blocks = new List<CodeBlock> { new(0, lines, new BlockExit(BlockExitKind.End, null, null, null)) };

        var split = new SuperstepSplitter().Split(blocks);

        // One round for m_s4's .x, .y and any; a second after m_s4 is written.
        Assert.Equal(3, split.Count);
        Assert.Equal(2, split.SelectMany(block => block.Lines).Count(line => line.Contains("vw_ss_put", StringComparison.Ordinal)));
    }
}
