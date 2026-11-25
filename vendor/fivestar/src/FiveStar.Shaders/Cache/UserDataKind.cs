// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>How a root-signature binding reaches the program (bits 8..15 of a tag 0x41 entry's u32 at +4).</summary>
public enum UserDataKind
{
    /// <summary>Root constants: the value is in a user SGPR.</summary>
    RootConstants = 0,

    /// <summary>Root descriptor: a 64-bit address in a user SGPR pair.</summary>
    RootDescriptor = 1,

    /// <summary>Descriptor table entry: a user SGPR holds the table's 32-bit address, the descriptor sits at an offset in it.</summary>
    TableEntry = 3,

    /// <summary>Static sampler: the S# is a literal in the code, no user SGPR.</summary>
    StaticSampler = 4,
}
