// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>Decodes the NGG block of an FSSM header (hardware stage 7).</summary>
internal static class NggBlock
{
    // Merged-shader layout: s0..s7 are system SGPRs, user data starts at s8.
    private const int UserSgprBase = 8;
    private const int Config55CField = 0x55C;
    private const int Config560Field = 0x560;

    public static NggConfig Decode(ReadOnlySpan<byte> program)
    {
        return new NggConfig(UserSgprBase, FssmHeader.U32(program, Config55CField), FssmHeader.U32(program, Config560Field));
    }
}
