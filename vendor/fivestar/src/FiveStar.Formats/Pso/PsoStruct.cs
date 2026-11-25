// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>A struct schema: name hash, signature hash, flags, size and members.</summary>
/// <param name="Name">Struct name hash (or a synthesized id 0x100.. for map nodes).</param>
/// <param name="Signature">Signature hash.</param>
/// <param name="Flags">Flags; byte 1 holds log2 of the alignment.</param>
/// <param name="Size">Size in bytes.</param>
/// <param name="Members">The member records in schema order.</param>
public sealed record PsoStruct(uint Name, uint Signature, uint Flags, uint Size, IReadOnlyList<PsoMember> Members)
{
    private const int AlignmentShift = 8;
    private const uint AlignmentMask = 0xF;

    /// <summary>Gets the alignment in bytes.</summary>
    public int Alignment => 1 << (int)((Flags >> AlignmentShift) & AlignmentMask);
}
