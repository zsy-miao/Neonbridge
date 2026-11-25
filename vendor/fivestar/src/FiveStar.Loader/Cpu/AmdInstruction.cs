// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>
/// One decoded AMD-only instruction.
/// </summary>
/// <param name="Kind">Which instruction.</param>
/// <param name="Length">Its length in bytes, prefixes included.</param>
/// <param name="Destination">EXTRQ/INSERTQ: the destination XMM register (0-15).</param>
/// <param name="Source">INSERTQ and the register form of EXTRQ: the source XMM register.</param>
/// <param name="Field">The immediate forms' field length and index; null for the register forms, which read them from the source.</param>
/// <param name="AddressSize32">CLZERO and MONITORX with the 67h prefix: the address is EAX, not RAX.</param>
internal readonly record struct AmdInstruction(
    AmdInstructionKind Kind,
    int Length,
    int Destination,
    int Source,
    BitFieldSpec? Field,
    bool AddressSize32);
