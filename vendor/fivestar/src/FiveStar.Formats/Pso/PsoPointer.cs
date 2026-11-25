// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>
/// A pointer inside L-format PSO data: a u64 whose low 20 bits are the 1-based block id (0 = null) and whose upper
/// 44 bits are the byte offset inside that block.
/// </summary>
/// <param name="Block">1-based block id; 0 for null.</param>
/// <param name="Offset">Byte offset inside the block.</param>
public readonly record struct PsoPointer(uint Block, ulong Offset)
{
    private const int BlockBits = 20;
    private const ulong BlockMask = (1UL << BlockBits) - 1;

    public static PsoPointer Decode(ulong raw)
    {
        return new PsoPointer((uint)(raw & BlockMask), raw >> BlockBits);
    }
}
