// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;

namespace FiveStar.Shaders.Hardware;

/// <summary>Decodes the compute block of an FSSM header (hardware stage 6).</summary>
internal static class ComputeBlock
{
    private const int Rsrc2Field = 0x138;
    private const int NumThreadsXField = 0x13C;
    private const int NumThreadsYField = 0x140;
    private const int NumThreadsZField = 0x144;

    // COMPUTE_PGM_RSRC2: bit 0 SCRATCH_EN, bits 1..5 USER_SGPR, bits 7/8/9 TGID_X/Y/Z_EN, bit 10 TG_SIZE_EN,
    // bits 11..12 TIDIG_COMP_CNT, bits 15..23 LDS_SIZE.
    private const int UserSgprShift = 1;
    private const uint UserSgprMask = 0x1F;
    private const int GroupIdXShift = 7;
    private const int GroupSizeShift = 10;
    private const int ThreadIdShift = 11;
    private const uint ThreadIdMask = 0x3;
    private const int LdsShift = 15;
    private const uint LdsMask = 0x1FF;
    private const string Axes = "xyz";

    public static ShaderConfig Apply(ShaderConfig config, ReadOnlySpan<byte> program)
    {
        var rsrc2 = FssmHeader.U32(program, Rsrc2Field);
        var userSgprs = (int)((rsrc2 >> UserSgprShift) & UserSgprMask);
        var groupSize = ((rsrc2 >> GroupSizeShift) & 1) != 0;

        // System SGPRs follow the user data: enabled group-id components in x, y, z order, then the group size.
        var groupIds = new List<GroupIdSgpr>();
        var next = userSgprs;

        for (var axis = 0; axis < Axes.Length; axis++)
        {
            if (((rsrc2 >> (GroupIdXShift + axis)) & 1) != 0)
            {
                groupIds.Add(new GroupIdSgpr(Axes[axis], next));
                next++;
            }
        }

        int? groupSizeSgpr = groupSize ? next : null;
        var compute = new ComputeConfig
        {
            Rsrc2 = rsrc2,
            ScratchEnable = (rsrc2 & 1) != 0,
            GroupSizeEnable = groupSize,
            ThreadIdComponentCount = (int)((rsrc2 >> ThreadIdShift) & ThreadIdMask),
            LdsBlocks = (int)((rsrc2 >> LdsShift) & LdsMask),
            NumThreads = new ThreadGroupSize(
                (int)FssmHeader.U32(program, NumThreadsXField),
                (int)FssmHeader.U32(program, NumThreadsYField),
                (int)FssmHeader.U32(program, NumThreadsZField)),
            GroupIdSgprs = groupIds,
            GroupSizeSgpr = groupSizeSgpr,
        };

        return config with { UserSgprs = userSgprs, Compute = compute };
    }
}
