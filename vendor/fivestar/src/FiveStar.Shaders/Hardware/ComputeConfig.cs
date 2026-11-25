// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Hardware;

/// <summary>Compute fields of an FSSM header: COMPUTE_PGM_RSRC2, numthreads and where the group and thread ids land.</summary>
public sealed record ComputeConfig
{
    public required uint Rsrc2 { get; init; }

    public required bool ScratchEnable { get; init; }

    public required bool GroupSizeEnable { get; init; }

    /// <summary>TIDIG_COMP_CNT: thread id components loaded into v0 (x), v1 (y), v2 (z).</summary>
    public required int ThreadIdComponentCount { get; init; }

    /// <summary>LDS_SIZE in 512-byte granules.</summary>
    public required int LdsBlocks { get; init; }

    public required ThreadGroupSize NumThreads { get; init; }

    /// <summary>Enabled group-id components and their SGPRs, in x, y, z order.</summary>
    public required IReadOnlyList<GroupIdSgpr> GroupIdSgprs { get; init; }

    public int? GroupSizeSgpr { get; init; }
}
