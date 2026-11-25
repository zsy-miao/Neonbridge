// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>
/// One member record of a struct schema (16 bytes: u32 name, u32 offset, u32 type, u32 ref).
/// The type word holds the type code in its low byte, a subtype in byte 1 and an auxiliary value in its high half.
/// </summary>
/// <param name="Name">Member name hash; <see cref="AnonymousElement"/> marks an array or map element descriptor.</param>
/// <param name="Offset">Byte offset inside the struct.</param>
/// <param name="Type">The type word.</param>
/// <param name="Ref">Struct hash, enum hash, element count or key/value member indices, by type.</param>
/// <param name="Index">Position in the struct's member list.</param>
public sealed record PsoMember(uint Name, uint Offset, uint Type, uint Ref, int Index)
{
    /// <summary>The name of the element descriptor of an array or map.</summary>
    public const uint AnonymousElement = 0x100;

    private const uint CodeMask = 0xFF;
    private const int AuxShift = 16;

    /// <summary>Gets the type code (low byte of the type word).</summary>
    public PsoTypeCode Code => (PsoTypeCode)(Type & CodeMask);

    /// <summary>Gets the high half of the type word: element member index (arrays, maps) or bit count (bitsets).</summary>
    public int Aux => (int)(Type >> AuxShift);
}
