// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Translation.Core;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The loads of an S#'s dword 0 read the sampler heap's records (prelude SamplerPtrLoad): Scaleform's FTexTG programs
/// load their T# through a table and their S# at a root constant + index * 16, both 32-bit pointers.
/// </summary>
public sealed class SamplerLoadsTests
{
    private const int Size = 8;

    [Fact]
    public void Only_the_dword_an_image_instruction_samples_with_reads_the_sampler_records()
    {
        Instruction[] program =
        [
            new(0x00, Size, "s_load_dwordx8 s[4:11], s[0:1], 0x0"),
            new(0x08, Size, "s_lshl_b32 s14, s2, 4"),
            new(0x10, Size, "s_load_dwordx4 s[16:19], s[12:13], s14"),
            new(0x18, Size, "s_waitcnt lgkmcnt(0)"),

            // The deferred passes set the round mode between the load and the sample: its hwreg operand is read, not written.
            new(0x1c, Size, "s_setreg_imm32_b32 hwreg(HW_REG_MODE, 2, 2), 3"),
            new(0x20, Size, "image_sample v[0:3], v[0:1], s[4:11], s[16:19] dmask:0xf dim:SQ_RSRC_IMG_2D"),

            // A literal S#: no load. A T# and an S# in one load: only the S#'s first dword.
            new(0x28, Size, "s_mov_b32 s24, 0x400"),
            new(0x30, Size, "image_sample v[4:7], v[0:1], s[4:11], s[24:27] dmask:0xf dim:SQ_RSRC_IMG_2D"),
            new(0x38, Size, "s_load_dwordx16 s[32:47], s[0:1], 0x40"),
            new(0x40, Size, "s_waitcnt lgkmcnt(0)"),
            new(0x48, Size, "image_gather4 v[8:11], v[0:1], s[32:39], s[40:43] dmask:0x1 dim:SQ_RSRC_IMG_2D"),
        ];

        Assert.Equal([(0x10, 0), (0x38, 8)], SamplerLoads.Find(program).Order());
    }
}
